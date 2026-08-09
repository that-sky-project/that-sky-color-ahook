//! Android glue: JVM access, overlay window setup, lifecycle handling, and
//! cross-thread communication with the main thread.

pub mod activity;
pub mod input;
pub mod surface;
pub mod window;

pub use jni::JavaVM;

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use jni::objects::{Global, JObject, JString, JThrowable};
use jni::sys::jobject;
use jni::{Env, EnvUnowned, JValue, Outcome, jni_sig, jni_str};

use crate::ui::renderer::Renderer;
use crate::{log_error, log_info, log_warn};

static JVM: OnceLock<JavaVM> = OnceLock::new();

/// Store the JavaVM captured in `JNI_OnLoad`.
pub fn init_jvm(vm: JavaVM) -> std::result::Result<(), JavaVM> {
    JVM.set(vm)
}

/// Java handles for the current overlay window. Replaced on Activity re-setup.
struct OverlayRefs {
    activity: Global<JObject<'static>>,
    wm: Global<JObject<'static>>,
    params: Global<JObject<'static>>,
    /// FrameLayout added to the WindowManager (updateViewLayout target).
    view: Global<JObject<'static>>,
    /// Invisible EditText relay for the Lua tab (owns the IME connection).
    /// Positions are fixed at attach time; the boxes are never moved at
    /// runtime (child updateViewLayout stalls the game's main thread).
    edit_text: Global<JObject<'static>>,
    /// Multiline EditText for the Settings domain replacement rules.
    rule_edit: Global<JObject<'static>>,
    /// Main-Looper Handler: posts `MainThreadTask` from any thread.
    handler: Global<JObject<'static>>,
}

static OVERLAY_REFS: Mutex<Option<OverlayRefs>> = Mutex::new(None);

/// Panel handed to the renderer worker. Taken per session (Global is !Clone);
/// `setup_overlay` drops in a fresh ref after each re-setup.
static PANEL_SLOT: Mutex<Option<Global<JObject<'static>>>> = Mutex::new(None);
static PANEL_GEN: AtomicU64 = AtomicU64::new(0);

// Window movement (absolute positioning — see `move_surface_to`).
static TARGET_X: AtomicI32 = AtomicI32::new(100);
static TARGET_Y: AtomicI32 = AtomicI32::new(200);
static WINDOW_X: AtomicI32 = AtomicI32::new(100);
static WINDOW_Y: AtomicI32 = AtomicI32::new(200);
static MOVE_LAST_POST_MS: AtomicU64 = AtomicU64::new(0);

/// Main-thread task ops, coalesced into a single Runnable run().
const OP_MOVE: u32 = 1 << 0;
const OP_REINIT: u32 = 1 << 1;
const OP_RESIZE: u32 = 1 << 2;
const OP_FOCUS_LUA: u32 = 1 << 3;
const OP_UNFOCUS_LUA: u32 = 1 << 4;
const OP_FOCUS_SETTINGS: u32 = 1 << 5;
const OP_UNFOCUS_SETTINGS: u32 = 1 << 6;
/// Move the Settings native box's cursor (char idx in `CURSOR_CHAR_IDX`).
const OP_SET_CURSOR: u32 = 1 << 7;
static PENDING_OPS: AtomicU32 = AtomicU32::new(0);
static TASK_POSTED: AtomicBool = AtomicBool::new(false);
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);

/// Latest egui cursor position (char idx) for the Settings rules box.
static CURSOR_CHAR_IDX: AtomicI32 = AtomicI32::new(0);

static TARGET_W: AtomicI32 = AtomicI32::new(0);
static TARGET_H: AtomicI32 = AtomicI32::new(0);

/// The window's `FLAG_NOT_FOCUSABLE` bit (cleared while an input tab edits).
const FLAG_NOT_FOCUSABLE: i32 = 0x8;

/// Current window position in screen px (maintained by the main thread).
pub fn window_pos() -> (i32, i32) {
    (
        WINDOW_X.load(Ordering::Relaxed),
        WINDOW_Y.load(Ordering::Relaxed),
    )
}

/// Move the whole SurfaceView window to absolute screen coords (x, y).
///
/// `updateViewLayout` must run on the main thread, so this only records the
/// target and posts `MainThreadTask`. Absolute positioning (not incremental)
/// avoids the drag feedback loop: the pointer's view-space position shifts
/// when the window moves, which would otherwise oscillate the window.
/// Posting is throttled (~30/s): every touch MOVE would otherwise flood the
/// main thread with expensive window relayouts and stall it for seconds.
pub fn move_surface_to(x: i32, y: i32) {
    TARGET_X.store(x, Ordering::Relaxed);
    TARGET_Y.store(y, Ordering::Relaxed);

    let now = now_ms();
    let last = MOVE_LAST_POST_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 33 {
        return;
    }
    MOVE_LAST_POST_MS.store(now, Ordering::Relaxed);
    request_main_thread_op(OP_MOVE);
}

/// Monotonic-ish millisecond timestamp (wall clock, fine for throttling).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resize the whole SurfaceView window (px). Applied on the main thread.
pub fn resize_surface(width: i32, height: i32) {
    TARGET_W.store(width, Ordering::Relaxed);
    TARGET_H.store(height, Ordering::Relaxed);
    request_main_thread_op(OP_RESIZE);
}

/// Grant/revoke keyboard access for the Lua tab (window focus + IME).
pub fn set_lua_input_active(active: bool) {
    request_main_thread_op(if active { OP_FOCUS_LUA } else { OP_UNFOCUS_LUA });
}

/// Grant/revoke keyboard access for the Settings tab (same pattern as Lua).
pub fn set_settings_input_active(active: bool) {
    request_main_thread_op(if active {
        OP_FOCUS_SETTINGS
    } else {
        OP_UNFOCUS_SETTINGS
    });
}

/// Move the Settings native box's cursor to `char_idx` (egui char index),
/// so typing inserts at the position tapped in the egui TextEdit.
pub fn set_settings_cursor(char_idx: usize) {
    CURSOR_CHAR_IDX.store(char_idx as i32, Ordering::Relaxed);
    request_main_thread_op(OP_SET_CURSOR);
}

/// Ask the main thread to run one or more ops (at most one post in flight).
fn request_main_thread_op(op: u32) {
    PENDING_OPS.fetch_or(op, Ordering::AcqRel);
    if !TASK_POSTED.swap(true, Ordering::AcqRel) {
        post_main_thread_task();
    }
}

/// Post the cached `MainThreadTask` to the main thread via `handler.post`.
///
/// Uses a main-Looper Handler (not `view.post`) so the task still runs when
/// the view is detached (e.g. after the Activity window was torn down).
fn post_main_thread_task() {
    let Some(vm) = JVM.get() else {
        log_error!("[rust] JVM not initialized");
        return;
    };

    let _ = vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
        let task_raw = input::main_thread_task();
        let Some(task_raw) = task_raw else {
            log_error!("[rust] main thread task not installed");
            return Ok(());
        };

        let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
        let Some(refs) = refs.as_ref() else {
            return Ok(());
        };

        // SAFETY: task_raw is a cached Global reference (still alive).
        let task = unsafe { JObject::from_raw(env, task_raw) };

        env.call_method(
            refs.handler.as_obj(),
            jni_str!("post"),
            jni_sig!("(Ljava/lang/Runnable;)Z"),
            &[JValue::Object(&task)],
        )?;
        Ok(())
    });
}

/// `MainThreadTask.run()` native impl (**main thread**): consume pending ops.
#[unsafe(no_mangle)]
pub extern "system" fn native_main_thread_task_run(
    mut unowned_env: EnvUnowned<'_>,
    _this: JObject<'_>,
) {
    let outcome = unowned_env.with_env::<_, _, jni::errors::Error>(|env| {
        TASK_POSTED.store(false, Ordering::Release);
        let pending = PENDING_OPS.swap(0, Ordering::AcqRel);

        // Each op is isolated: a failure must not abort the remaining ops
        // (e.g. a settings teardown skipping its EditText hides), and any
        // pending exception is cleared right after the op so it can never
        // poison the main thread's JNIEnv — the touch callback runs on this
        // thread, so a stuck exception kills overlay input.
        let run = |env: &mut Env<'_>,
                   mask: u32,
                   name: &str,
                   op: fn(&mut Env<'_>) -> jni::errors::Result<()>| {
            if pending & mask != 0 {
                if let Err(e) = op(env) {
                    log_error!("[rust] main task {name} failed: {e:?}");
                }
                log_and_clear_exception(env);
            }
        };

        run(env, OP_MOVE, "move", apply_window_move);
        run(env, OP_RESIZE, "resize", apply_window_resize);
        // Unfocus before focusing: both change which input box is visible.
        run(env, OP_UNFOCUS_LUA, "unfocus_lua", apply_unfocus_lua);
        run(
            env,
            OP_UNFOCUS_SETTINGS,
            "unfocus_settings",
            apply_unfocus_settings,
        );
        run(env, OP_FOCUS_LUA, "focus_lua", apply_focus_lua);
        run(
            env,
            OP_FOCUS_SETTINGS,
            "focus_settings",
            apply_focus_settings,
        );
        // Cursor moves must follow the focus op (the box is shown there).
        run(env, OP_SET_CURSOR, "set_cursor", apply_set_cursor);
        if pending & OP_REINIT != 0 {
            log_info!("[rust] re-running overlay setup (activity recreated)");
            if let Err(e) = setup_overlay(env) {
                log_error!("[rust] main task reinit failed: {e:?}");
            }
            log_and_clear_exception(env);
        }
        Ok(())
    });

    match outcome.into_outcome() {
        Outcome::Ok(()) => {}
        Outcome::Err(e) => log_error!("[rust] main thread task error: {:?}", e),
        Outcome::Panic(_) => log_error!("[rust] main thread task panicked"),
    }
}

fn apply_window_move<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let x = TARGET_X.load(Ordering::Relaxed);
    let y = TARGET_Y.load(Ordering::Relaxed);

    let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(refs) = refs.as_ref() else {
        return Ok(());
    };

    let params = refs.params.as_obj();
    env.set_field(params, jni_str!("x"), jni_sig!("I"), JValue::Int(x))?;
    env.set_field(params, jni_str!("y"), jni_sig!("I"), JValue::Int(y))?;

    env.call_method(
        refs.wm.as_obj(),
        jni_str!("updateViewLayout"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(refs.view.as_obj()), JValue::Object(params)],
    )?;

    WINDOW_X.store(x, Ordering::Relaxed);
    WINDOW_Y.store(y, Ordering::Relaxed);

    Ok(())
}

/// Apply a pending window resize (width/height) on the main thread.
fn apply_window_resize<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let w = TARGET_W.load(Ordering::Relaxed);
    let h = TARGET_H.load(Ordering::Relaxed);

    let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(refs) = refs.as_ref() else {
        return Ok(());
    };

    let params = refs.params.as_obj();
    env.set_field(params, jni_str!("width"), jni_sig!("I"), JValue::Int(w))?;
    env.set_field(params, jni_str!("height"), jni_sig!("I"), JValue::Int(h))?;

    env.call_method(
        refs.wm.as_obj(),
        jni_str!("updateViewLayout"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(refs.view.as_obj()), JValue::Object(params)],
    )?;

    Ok(())
}

/// Show/hide one input EditText + IME (main thread).
///
/// The IME does not open on NOT_FOCUSABLE windows on this device, so while
/// editing the window is made focusable (same as the working Lua tab); leaving
/// restores NOT_FOCUSABLE so the game gets its input back. EditText positions
/// are fixed at attach time — they are never moved at runtime (a child
/// `updateViewLayout` stalls the game's main thread).
///
/// `shown_visibility` is the visibility to use when showing (`0` = VISIBLE,
/// `4` = INVISIBLE): Lua shows its box, Settings keeps its box hidden and
/// displays the text in egui instead.
fn apply_input_focus<'local>(
    env: &mut Env<'local>,
    focus: bool,
    edit: &JObject<'local>,
    shown_visibility: i32,
) -> jni::errors::Result<()> {
    let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(refs) = refs.as_ref() else {
        return Ok(());
    };

    // Window focusability (window-level updateViewLayout is safe; only child
    // relayouts stall). Best-effort: proceed with the teardown even if the
    // relayout fails, and clear any exception right after.
    let params = refs.params.as_obj();
    let flags = env
        .get_field(params, jni_str!("flags"), jni_sig!("I"))
        .and_then(|v| v.i());
    if let Ok(flags) = flags {
        let new_flags = if focus {
            flags & !FLAG_NOT_FOCUSABLE
        } else {
            flags | FLAG_NOT_FOCUSABLE
        };
        let _ = env.set_field(
            params,
            jni_str!("flags"),
            jni_sig!("I"),
            JValue::Int(new_flags),
        );
        let _ = env.call_method(
            refs.wm.as_obj(),
            jni_str!("updateViewLayout"),
            jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
            &[JValue::Object(refs.view.as_obj()), JValue::Object(params)],
        );
    }
    log_and_clear_exception(env);

    let imm = input_method_manager(env, &refs.activity)?;
    if focus {
        env.call_method(
            edit,
            jni_str!("setVisibility"),
            jni_sig!("(I)V"),
            &[JValue::Int(shown_visibility)],
        )?;
        env.call_method(edit, jni_str!("requestFocus"), jni_sig!("()Z"), &[])?;
        let shown = env
            .call_method(
                &imm,
                jni_str!("showSoftInput"),
                jni_sig!("(Landroid/view/View;I)Z"),
                &[JValue::Object(edit), JValue::Int(0)],
            )?
            .z()?;
        if !shown {
            // The IME connection attaches on a later looper turn; retry
            // shortly from a background thread as a fallback.
            retry_show_soft_input(edit.as_raw());
        }
    } else {
        env.call_method(
            edit,
            jni_str!("setVisibility"),
            jni_sig!("(I)V"),
            &[JValue::Int(8)],
        )?; // GONE
        env.call_method(edit, jni_str!("clearFocus"), jni_sig!("()V"), &[])?;
        let token = env
            .call_method(
                refs.view.as_obj(),
                jni_str!("getWindowToken"),
                jni_sig!("()Landroid/os/IBinder;"),
                &[],
            )?
            .l()?;
        env.call_method(
            &imm,
            jni_str!("hideSoftInputFromWindow"),
            jni_sig!("(Landroid/os/IBinder;I)Z"),
            &[JValue::Object(&token), JValue::Int(0)],
        )?;
    }

    Ok(())
}

/// Raw jobject pointers of the two input EditTexts (Copy; borrow dropped
/// before any nested OVERLAY_REFS lock).
fn edit_raw_refs() -> Option<(jobject, jobject)> {
    let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
    refs.as_ref()
        .map(|r| (r.edit_text.as_obj().as_raw(), r.rule_edit.as_obj().as_raw()))
}

/// Enter the Lua tab (main thread): hide the rules box, show the Lua box,
/// focus + IME. No layout changes — both boxes keep their attach-time
/// positions.
fn apply_focus_lua<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let Some((edit_raw, rule_raw)) = edit_raw_refs() else {
        return Ok(());
    };
    // SAFETY: Global refs held by OVERLAY_REFS for the process lifetime.
    let edit = unsafe { JObject::from_raw(env, edit_raw) };
    let rule = unsafe { JObject::from_raw(env, rule_raw) };
    let _ = env.call_method(
        &rule,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)], // rules box GONE
    );
    apply_input_focus(env, true, &edit, 0) // VISIBLE
}

/// Leave the Lua tab (main thread): hide the boxes, clear focus, hide IME.
fn apply_unfocus_lua<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let Some((edit_raw, rule_raw)) = edit_raw_refs() else {
        return Ok(());
    };
    // SAFETY: Global refs held by OVERLAY_REFS for the process lifetime.
    let edit = unsafe { JObject::from_raw(env, edit_raw) };
    let rule = unsafe { JObject::from_raw(env, rule_raw) };
    let _ = env.call_method(
        &rule,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)],
    );
    apply_input_focus(env, false, &edit, 8)
}

/// Enter the Settings tab (main thread): hide the Lua box, show the rules
/// box, focus + IME.
fn apply_focus_settings<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let Some((edit_raw, rule_raw)) = edit_raw_refs() else {
        return Ok(());
    };
    // SAFETY: Global refs held by OVERLAY_REFS for the process lifetime.
    let edit = unsafe { JObject::from_raw(env, edit_raw) };
    let rule = unsafe { JObject::from_raw(env, rule_raw) };
    let _ = env.call_method(
        &edit,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)], // Lua box GONE
    );
    // VISIBLE (0): the box is 1x1 so it is effectively invisible, but
    // showSoftInput requires a VISIBLE view (isShown) — INVISIBLE fails it.
    apply_input_focus(env, true, &rule, 0)
}

/// Leave the Settings tab (main thread): hide the boxes, clear focus, hide
/// IME, then commit the rules (best-effort, exception-safe).
fn apply_unfocus_settings<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let Some((edit_raw, rule_raw)) = edit_raw_refs() else {
        return Ok(());
    };
    // SAFETY: Global refs held by OVERLAY_REFS for the process lifetime.
    let edit = unsafe { JObject::from_raw(env, edit_raw) };
    let rule = unsafe { JObject::from_raw(env, rule_raw) };
    let _ = env.call_method(
        &edit,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)],
    );
    let _ = apply_input_focus(env, false, &rule, 8);
    if let Ok(text) = read_edit_text(env, &rule) {
        crate::hooks::settings::set_rules(crate::hooks::settings::parse_rules(&text));
    }
    log_and_clear_exception(env);
    Ok(())
}

/// Move the Settings native box's cursor to the position tapped in the egui
/// TextEdit (main thread). `setSelection` takes UTF-16 code-unit indices.
fn apply_set_cursor<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    let Some((_edit_raw, rule_raw)) = edit_raw_refs() else {
        return Ok(());
    };
    // SAFETY: Global refs held by OVERLAY_REFS for the process lifetime.
    let rule = unsafe { JObject::from_raw(env, rule_raw) };
    let char_idx = CURSOR_CHAR_IDX.load(Ordering::Relaxed).max(0) as usize;
    let text = read_edit_text(env, &rule)?;
    let utf16 = char_idx_to_utf16(&text, char_idx);
    env.call_method(
        &rule,
        jni_str!("setSelection"),
        jni_sig!("(II)V"),
        &[JValue::Int(utf16), JValue::Int(utf16)],
    )?;
    Ok(())
}

/// Retry `showSoftInput` after a delay.
///
/// Right after `requestFocus`, a NOT_FOCUSABLE|ALT_FOCUSABLE_IM window may
/// not have its IME connection attached yet, so the first `showSoftInput`
/// returns false. Retrying from a background thread (the call is just a
/// binder message) lets the view hierarchy settle first.
fn retry_show_soft_input(edit_raw: jobject) {
    let Some(vm) = JVM.get() else {
        return;
    };
    // jobject is !Send; carry the address instead and re-cast on use.
    let edit_addr = edit_raw as usize;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        for _attempt in 0..5 {
            let shown = vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
                let refs = OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner());
                let Some(refs) = refs.as_ref() else {
                    return Ok(false);
                };
                let imm = input_method_manager(env, &refs.activity)?;
                // SAFETY: the address points into a Global ref held by OVERLAY_REFS.
                let edit = unsafe { JObject::from_raw(env, edit_addr as jobject) };
                env.call_method(
                    &imm,
                    jni_str!("showSoftInput"),
                    jni_sig!("(Landroid/view/View;I)Z"),
                    &[JValue::Object(&edit), JValue::Int(0)],
                )
                .and_then(|v| v.z())
            });
            match shown {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => log_error!("[rust] retry showSoftInput failed: {e:?}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    });
}

/// `activity.getSystemService(INPUT_METHOD_SERVICE)`
fn input_method_manager<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let service = env.new_string("input_method")?;
    env.call_method(
        activity,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&service.into())],
    )
    .and_then(|v| v.l())
}

/// Current text of the Lua input box (read via JNI; call from any thread).
pub fn lua_input_text() -> Option<String> {
    let vm = JVM.get()?;
    let edit_raw: Option<jobject> = OVERLAY_REFS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|r| r.edit_text.as_obj().as_raw());
    let edit_raw = edit_raw?;

    let result = vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
        // SAFETY: edit_raw is a Global ref held by OVERLAY_REFS.
        let edit = unsafe { JObject::from_raw(env, edit_raw) };
        read_edit_text(env, &edit)
    });

    match result {
        Ok(text) => Some(text),
        Err(e) => {
            log_error!("[rust] lua_input_text failed: {:?}", e);
            None
        }
    }
}

/// Current text + cursor (char idx) of the Settings rules box (read via JNI;
/// call from any thread, throttled by the caller).
pub fn settings_input_state() -> Option<(String, usize)> {
    let vm = JVM.get()?;
    let rule_raw: Option<jobject> = OVERLAY_REFS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|r| r.rule_edit.as_obj().as_raw());
    let rule_raw = rule_raw?;

    let result = vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
        // SAFETY: rule_raw is a Global ref held by OVERLAY_REFS.
        let rule = unsafe { JObject::from_raw(env, rule_raw) };
        let text = read_edit_text(env, &rule)?;
        // getSelectionStart is a UTF-16 code-unit index; egui uses chars.
        let sel = env
            .call_method(&rule, jni_str!("getSelectionStart"), jni_sig!("()I"), &[])?
            .i()
            .unwrap_or(0)
            .max(0) as usize;
        let cursor = utf16_to_char_idx(&text, sel);
        Ok((text, cursor))
    });

    match result {
        Ok(state) => Some(state),
        Err(e) => {
            log_error!("[rust] settings_input_state failed: {:?}", e);
            None
        }
    }
}

/// UTF-16 code-unit index (EditText selection) -> egui char index.
fn utf16_to_char_idx(s: &str, utf16_idx: usize) -> usize {
    let mut utf16_seen = 0usize;
    let mut char_idx = 0usize;
    for c in s.chars() {
        if utf16_seen >= utf16_idx {
            break;
        }
        utf16_seen += c.len_utf16();
        char_idx += 1;
    }
    char_idx
}

/// egui char index -> UTF-16 code-unit index (for `EditText.setSelection`).
fn char_idx_to_utf16(s: &str, char_idx: usize) -> i32 {
    s.chars()
        .take(char_idx)
        .map(|c| c.len_utf16())
        .sum::<usize>() as i32
}

/// `editText.getText().toString()`
fn read_edit_text<'local>(
    env: &mut Env<'local>,
    edit: &JObject<'local>,
) -> jni::errors::Result<String> {
    let editable = env
        .call_method(
            edit,
            jni_str!("getText"),
            jni_sig!("()Landroid/text/Editable;"),
            &[],
        )?
        .l()?;
    let text = env
        .call_method(
            &editable,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    let jstr = env.new_cast_local_ref::<JString<'local>>(&text)?;
    jstr.try_to_string(env)
}

// ============================================================
// Overlay setup (re-entrant)
// ============================================================

/// Initial overlay setup: run `setup_overlay` and start the renderer worker.
///
/// Must run on the main thread (WindowManager.addView requires it).
pub fn init_overlay<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    setup_overlay(env)?;
    ensure_worker()?;
    log_info!("[rust] overlay initialized");
    Ok(())
}

/// Create (or re-create, after the Activity died) the overlay window and
/// publish the new panel for the renderer worker.
///
/// Re-entrant: retires the previous view, discovers the current Activity, and
/// re-attaches a fresh SurfaceView. Runs on the main thread.
pub fn setup_overlay<'local>(env: &mut Env<'local>) -> jni::errors::Result<()> {
    log_info!("[rust] setting up overlay...");

    // Discover + build everything for the new window first, so a failure here
    // leaves the previous window untouched.
    let activity = activity::get_current_activity(env)?;
    let wm = activity::get_window_manager(env, &activity)?;
    let token = activity::get_window_token(env, &activity)?;

    let panel = window::create_surface_view(env, &activity)?;
    let edit_text = window::create_edit_text(env, &activity)?;
    let rule_edit = window::create_rule_edit(env, &activity)?;
    let view = window::wrap_in_frame_layout(env, &activity, &panel, &edit_text, &rule_edit)?;
    let params = window::create_layout_params(env, &token)?;

    // Non-fatal, but log + clear any pending exception (Phase 18).
    if let Err(e) = window::set_transparent_background(env, &panel) {
        log_error!("[rust] set background failed: {:?}", e);
        log_and_clear_exception(env);
    }

    window::add_view(env, &wm, &view, &params)?;

    // The surface defaults to OPAQUE; without TRANSLUCENT format the
    // SurfaceFlinger ignores buffer alpha (alpha=0 renders black).
    window::set_translucent_format(env, &panel)?;

    // Retire the previous window only now that the new one is attached, so a
    // failure above leaves the old window intact and the worker retries.
    if let Some(old) = OVERLAY_REFS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        let _ = env.call_method(
            old.wm.as_obj(),
            jni_str!("removeView"),
            jni_sig!("(Landroid/view/View;)V"),
            &[JValue::Object(old.view.as_obj())],
        );
        log_and_clear_exception(env);
    }

    log_info!("[rust] panel added");

    // Touch: define listener classes from embedded DEX. Non-fatal on failure.
    let app_loader = env
        .call_method(
            &activity,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    input::install_touch_listener(env, &panel, &app_loader).unwrap_or_else(|e| {
        log_error!("[rust] install touch listener FAILED: {:?}", e);
        log_and_clear_exception(env);
    });

    let handler = create_main_handler(env)?;

    *OVERLAY_REFS.lock().unwrap_or_else(|p| p.into_inner()) = Some(OverlayRefs {
        activity: env.new_global_ref(&activity)?,
        wm: env.new_global_ref(&wm)?,
        params: env.new_global_ref(&params)?,
        view: env.new_global_ref(&view)?,
        edit_text: env.new_global_ref(&edit_text)?,
        rule_edit: env.new_global_ref(&rule_edit)?,
        handler,
    });
    *PANEL_SLOT.lock().unwrap_or_else(|p| p.into_inner()) = Some(env.new_global_ref(&panel)?);
    PANEL_GEN.fetch_add(1, Ordering::Relaxed);

    // The window is recreated at full size; drop the collapsed flag.
    crate::ui::reset_collapsed();

    log_info!("[rust] overlay refs published");
    Ok(())
}

/// `new Handler(Looper.getMainLooper())`, cached as a global ref.
fn create_main_handler<'local>(
    env: &mut Env<'local>,
) -> jni::errors::Result<Global<JObject<'static>>> {
    let looper_cls = env.find_class(jni_str!("android/os/Looper"))?;
    let looper = env
        .call_static_method(
            &looper_cls,
            jni_str!("getMainLooper"),
            jni_sig!("()Landroid/os/Looper;"),
            &[],
        )?
        .l()?;

    let handler_cls = env.find_class(jni_str!("android/os/Handler"))?;
    let handler = env.new_object(
        &handler_cls,
        jni_sig!("(Landroid/os/Looper;)V"),
        &[JValue::Object(&looper)],
    )?;

    env.new_global_ref(&handler)
}

/// Spawn the renderer worker once; it self-restarts after a panic.
fn ensure_worker() -> jni::errors::Result<()> {
    if WORKER_STARTED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }

    std::thread::Builder::new()
        .name("overlay-renderer".to_string())
        .spawn(|| {
            loop {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(renderer_worker));
                if result.is_err() {
                    log_error!("[rust] overlay worker panicked; restarting");
                }
            }
        })
        .map_err(|e| {
            log_error!("[rust] spawn overlay-renderer failed: {e}");
            jni::errors::Error::JniCall(jni::errors::JniError::InvalidArguments)
        })?;

    log_info!("[rust] overlay worker spawned");
    Ok(())
}

// ============================================================
// Renderer worker (lifecycle-aware)
// ============================================================

/// Worker thread: acquire the panel's Surface -> render until it is destroyed,
/// then wait for recreation. Re-setups the overlay when the Activity dies.
fn renderer_worker() {
    loop {
        let Some(panel) = take_panel() else {
            std::thread::sleep(std::time::Duration::from_millis(250));
            continue;
        };
        let generation = PANEL_GEN.load(Ordering::Relaxed);

        loop {
            match acquire_native_window(&panel) {
                Some(window) => {
                    log_info!("[rust] ANativeWindow acquired: {:p}", window.as_ptr());
                    // The surface was just validated; the holder callback may
                    // have missed the recreate, so mark it alive explicitly.
                    surface::mark_alive();
                    // Blocks until the surface is destroyed.
                    Renderer::run_until_surface_lost(window);
                    log_warn!("[rust] renderer stopped (surface lost), waiting for recreation...");
                }
                None => {
                    // The surface did not come back within the timeout.
                    if PANEL_GEN.load(Ordering::Relaxed) != generation {
                        log_warn!("[rust] overlay re-set up; swapping panel");
                        break; // drop stale panel, take the fresh one
                    }
                    if !activity_alive() {
                        log_warn!("[rust] activity dead; requesting overlay re-setup");
                        request_main_thread_op(OP_REINIT);
                        break; // drop stale panel, wait for the new one
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
}

/// Take the current panel out of the slot (None while not yet set up).
fn take_panel() -> Option<Global<JObject<'static>>> {
    PANEL_SLOT.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Whether the current overlay Activity is still alive (not destroyed/finishing).
fn activity_alive() -> bool {
    let Some(vm) = JVM.get() else {
        return true;
    };
    let activity_raw: Option<jobject> = OVERLAY_REFS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|r| r.activity.as_obj().as_raw());
    let Some(activity_raw) = activity_raw else {
        return true; // no refs yet — don't request re-setup
    };

    match vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
        // SAFETY: activity_raw is a Global ref held by OVERLAY_REFS.
        let activity = unsafe { JObject::from_raw(env, activity_raw) };
        let destroyed = env
            .call_method(&activity, jni_str!("isDestroyed"), jni_sig!("()Z"), &[])?
            .z()?;
        let finishing = env
            .call_method(&activity, jni_str!("isFinishing"), jni_sig!("()Z"), &[])?
            .z()?;
        Ok(!destroyed && !finishing)
    }) {
        Ok(alive) => alive,
        Err(e) => {
            log_error!("[rust] activity liveness check failed: {:?}", e);
            true
        }
    }
}

/// Poll for a valid Surface (created asynchronously after addView / recreation)
/// and convert it to an owned `NativeWindow`.
fn acquire_native_window(panel: &Global<JObject<'static>>) -> Option<surface::NativeWindow> {
    let vm = JVM.get()?;

    match vm.attach_current_thread::<_, _, jni::errors::Error>(|env| {
        let surface = surface::wait_for_valid_surface(
            env,
            panel.as_obj(),
            std::time::Duration::from_secs(5),
        )?;
        log_info!("[rust] Surface acquired");
        log_info!("[rust] Surface valid");

        let native_ptr =
            unsafe { surface::ANativeWindow_fromSurface(env.get_raw(), surface.as_raw()) };

        match unsafe { surface::NativeWindow::from_raw(native_ptr) } {
            Some(window) => Ok(Some(window)),
            None => {
                log_error!("[rust] ANativeWindow_fromSurface returned null");
                Ok(None)
            }
        }
    }) {
        Ok(window) => window,
        Err(e) => {
            log_error!("[rust] surface acquisition FAILED: {:?}", e);
            None
        }
    }
}

// ============================================================
// Exception diagnostics
// ============================================================

/// Read a Java String into a Rust String.
fn jstring_to_rust_string<'local>(
    env: &mut Env<'local>,
    obj: &JObject<'local>,
) -> jni::errors::Result<String> {
    let jstr = env.new_cast_local_ref::<JString<'local>>(obj)?;
    jstr.try_to_string(env)
}

/// Log the pending Java exception (class + message + cause chain, 4 levels),
/// then describe and clear it.
///
/// jni's `call_method` clears exceptions before returning, so diagnostics are
/// only meaningful when the failure path keeps the exception pending.
pub(crate) fn log_and_clear_exception<'local>(env: &mut Env<'local>) {
    if !env.exception_check() {
        return;
    }

    env.exception_describe();

    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return;
    };
    env.exception_clear();

    let mut depth = 0usize;
    let mut current = Some(throwable);

    while let Some(throwable) = current {
        if depth >= 4 {
            break;
        }

        let class_name = env
            .get_object_class(&throwable)
            .and_then(|c| {
                env.call_method(
                    &c,
                    jni_str!("getName"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
            })
            .and_then(|v| v.l())
            .and_then(|s| jstring_to_rust_string(env, &s))
            .unwrap_or_else(|_| "?".to_string());

        let message = env
            .call_method(
                &throwable,
                jni_str!("getMessage"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(|v| v.l())
            .and_then(|s| jstring_to_rust_string(env, &s))
            .unwrap_or_else(|_| "<no message>".to_string());

        log_error!("[rust] java exception[{depth}]: {class_name}: {message}");

        current = env
            .call_method(
                &throwable,
                jni_str!("getCause"),
                jni_sig!("()Ljava/lang/Throwable;"),
                &[],
            )
            .and_then(|v| v.l())
            .and_then(|cause| env.new_cast_local_ref::<JThrowable<'local>>(&cause))
            .ok();
        depth += 1;
    }
}
