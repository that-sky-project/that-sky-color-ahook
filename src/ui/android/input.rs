//! Touch input: DEX-based `OnTouchListener` + event queue.
//!
//! Android 10+ ART dropped .class loading (`can't load this type of class
//! file`), so the listener classes are embedded as DEX (javac -> d8) and
//! loaded with `InMemoryDexClassLoader`.

use std::collections::VecDeque;
use std::sync::Mutex;

use jni::objects::{JClass, JObject};
use jni::sys::{JNI_TRUE, jboolean};
use jni::{Env, EnvUnowned, JValue, NativeMethod, Outcome, jni_sig, jni_str};

use crate::log_error;

/// Embedded DEX (d8, dex version 035) with three classes:
///
/// ```java
/// package skyhook;
///
/// public class TouchListener implements android.view.View.OnTouchListener {
///     public native boolean onTouch(android.view.View view, android.view.MotionEvent event);
///     public TouchListener() { super(); }
/// }
///
/// public class MainThreadTask implements Runnable {
///     public native void run();   // executes on the main thread
///     public MainThreadTask() { super(); }
/// }
///
/// public class SurfaceCallback implements android.view.SurfaceHolder.Callback {
///     public native void surfaceCreated(android.view.SurfaceHolder holder);
///     public native void surfaceChanged(android.view.SurfaceHolder holder, int format, int width, int height);
///     public native void surfaceDestroyed(android.view.SurfaceHolder holder);
///     public SurfaceCallback() { super(); }
/// }
/// ```
const TOUCH_LISTENER_DEX_BYTECODE: &[u8] = &[
    100, 101, 120, 10, 48, 51, 53, 0, 72, 253, 123, 15, 204, 4, 103, 21, 168, 54, 165, 130, 88, 81,
    5, 116, 188, 212, 200, 137, 235, 78, 71, 176, 148, 5, 0, 0, 112, 0, 0, 0, 120, 86, 52, 18, 0,
    0, 0, 0, 0, 0, 0, 0, 244, 4, 0, 0, 26, 0, 0, 0, 112, 0, 0, 0, 13, 0, 0, 0, 216, 0, 0, 0, 4, 0,
    0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 60, 1, 0, 0, 3, 0, 0, 0, 132, 1, 0, 0,
    176, 3, 0, 0, 228, 1, 0, 0, 108, 2, 0, 0, 116, 2, 0, 0, 119, 2, 0, 0, 147, 2, 0, 0, 186, 2, 0,
    0, 216, 2, 0, 0, 253, 2, 0, 0, 18, 3, 0, 0, 38, 3, 0, 0, 60, 3, 0, 0, 86, 3, 0, 0, 113, 3, 0,
    0, 138, 3, 0, 0, 159, 3, 0, 0, 181, 3, 0, 0, 201, 3, 0, 0, 204, 3, 0, 0, 208, 3, 0, 0, 215, 3,
    0, 0, 218, 3, 0, 0, 223, 3, 0, 0, 232, 3, 0, 0, 237, 3, 0, 0, 253, 3, 0, 0, 13, 4, 0, 0, 31, 4,
    0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0, 7, 0, 0, 0, 8, 0,
    0, 0, 9, 0, 0, 0, 10, 0, 0, 0, 11, 0, 0, 0, 15, 0, 0, 0, 18, 0, 0, 0, 15, 0, 0, 0, 11, 0, 0, 0,
    0, 0, 0, 0, 16, 0, 0, 0, 11, 0, 0, 0, 80, 2, 0, 0, 17, 0, 0, 0, 11, 0, 0, 0, 88, 2, 0, 0, 19,
    0, 0, 0, 12, 0, 0, 0, 100, 2, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0,
    21, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 9, 0, 2, 0, 22, 0, 0, 0, 9, 0, 1, 0, 23, 0, 0, 0, 9, 0, 1,
    0, 24, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 10, 0, 3, 0, 20, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0, 6,
    0, 0, 0, 56, 2, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 189, 4, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 1, 0, 0,
    0, 6, 0, 0, 0, 64, 2, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0, 203, 4, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 1,
    0, 0, 0, 6, 0, 0, 0, 72, 2, 0, 0, 14, 0, 0, 0, 0, 0, 0, 0, 225, 4, 0, 0, 0, 0, 0, 0, 1, 0, 1,
    0, 1, 0, 0, 0, 44, 2, 0, 0, 4, 0, 0, 0, 112, 16, 0, 0, 0, 0, 14, 0, 1, 0, 1, 0, 1, 0, 0, 0, 49,
    2, 0, 0, 4, 0, 0, 0, 112, 16, 0, 0, 0, 0, 14, 0, 1, 0, 1, 0, 1, 0, 0, 0, 44, 2, 0, 0, 4, 0, 0,
    0, 112, 16, 0, 0, 0, 0, 14, 0, 8, 0, 14, 60, 0, 14, 0, 14, 60, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0,
    1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0,
    0, 0, 0, 0, 2, 0, 0, 0, 5, 0, 1, 0, 6, 60, 105, 110, 105, 116, 62, 0, 1, 73, 0, 26, 76, 97,
    110, 100, 114, 111, 105, 100, 47, 118, 105, 101, 119, 47, 77, 111, 116, 105, 111, 110, 69, 118,
    101, 110, 116, 59, 0, 37, 76, 97, 110, 100, 114, 111, 105, 100, 47, 118, 105, 101, 119, 47, 83,
    117, 114, 102, 97, 99, 101, 72, 111, 108, 100, 101, 114, 36, 67, 97, 108, 108, 98, 97, 99, 107,
    59, 0, 28, 76, 97, 110, 100, 114, 111, 105, 100, 47, 118, 105, 101, 119, 47, 83, 117, 114, 102,
    97, 99, 101, 72, 111, 108, 100, 101, 114, 59, 0, 35, 76, 97, 110, 100, 114, 111, 105, 100, 47,
    118, 105, 101, 119, 47, 86, 105, 101, 119, 36, 79, 110, 84, 111, 117, 99, 104, 76, 105, 115,
    116, 101, 110, 101, 114, 59, 0, 19, 76, 97, 110, 100, 114, 111, 105, 100, 47, 118, 105, 101,
    119, 47, 86, 105, 101, 119, 59, 0, 18, 76, 106, 97, 118, 97, 47, 108, 97, 110, 103, 47, 79, 98,
    106, 101, 99, 116, 59, 0, 20, 76, 106, 97, 118, 97, 47, 108, 97, 110, 103, 47, 82, 117, 110,
    110, 97, 98, 108, 101, 59, 0, 24, 76, 115, 107, 121, 104, 111, 111, 107, 47, 77, 97, 105, 110,
    84, 104, 114, 101, 97, 100, 84, 97, 115, 107, 59, 0, 25, 76, 115, 107, 121, 104, 111, 111, 107,
    47, 83, 117, 114, 102, 97, 99, 101, 67, 97, 108, 108, 98, 97, 99, 107, 59, 0, 23, 76, 115, 107,
    121, 104, 111, 111, 107, 47, 84, 111, 117, 99, 104, 76, 105, 115, 116, 101, 110, 101, 114, 59,
    0, 19, 77, 97, 105, 110, 84, 104, 114, 101, 97, 100, 84, 97, 115, 107, 46, 106, 97, 118, 97, 0,
    20, 83, 117, 114, 102, 97, 99, 101, 67, 97, 108, 108, 98, 97, 99, 107, 46, 106, 97, 118, 97, 0,
    18, 84, 111, 117, 99, 104, 76, 105, 115, 116, 101, 110, 101, 114, 46, 106, 97, 118, 97, 0, 1,
    86, 0, 2, 86, 76, 0, 5, 86, 76, 73, 73, 73, 0, 1, 90, 0, 3, 90, 76, 76, 0, 7, 111, 110, 84,
    111, 117, 99, 104, 0, 3, 114, 117, 110, 0, 14, 115, 117, 114, 102, 97, 99, 101, 67, 104, 97,
    110, 103, 101, 100, 0, 14, 115, 117, 114, 102, 97, 99, 101, 67, 114, 101, 97, 116, 101, 100, 0,
    16, 115, 117, 114, 102, 97, 99, 101, 68, 101, 115, 116, 114, 111, 121, 101, 100, 0, 155, 1,
    126, 126, 68, 56, 123, 34, 98, 97, 99, 107, 101, 110, 100, 34, 58, 34, 100, 101, 120, 34, 44,
    34, 99, 111, 109, 112, 105, 108, 97, 116, 105, 111, 110, 45, 109, 111, 100, 101, 34, 58, 34,
    100, 101, 98, 117, 103, 34, 44, 34, 104, 97, 115, 45, 99, 104, 101, 99, 107, 115, 117, 109,
    115, 34, 58, 102, 97, 108, 115, 101, 44, 34, 109, 105, 110, 45, 97, 112, 105, 34, 58, 49, 44,
    34, 115, 104, 97, 45, 49, 34, 58, 34, 48, 56, 52, 97, 56, 57, 49, 50, 54, 97, 101, 48, 53, 57,
    54, 99, 53, 57, 57, 49, 52, 51, 100, 102, 53, 55, 97, 54, 53, 52, 54, 52, 54, 97, 102, 50, 56,
    51, 49, 51, 34, 44, 34, 118, 101, 114, 115, 105, 111, 110, 34, 58, 34, 57, 46, 50, 46, 52, 45,
    100, 101, 118, 34, 125, 0, 0, 0, 1, 1, 1, 129, 128, 4, 228, 3, 2, 129, 2, 0, 0, 0, 1, 3, 3,
    129, 128, 4, 252, 3, 4, 129, 2, 0, 1, 129, 2, 0, 1, 129, 2, 0, 0, 0, 1, 1, 7, 129, 128, 4, 148,
    4, 8, 129, 2, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
    26, 0, 0, 0, 112, 0, 0, 0, 2, 0, 0, 0, 13, 0, 0, 0, 216, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 12,
    1, 0, 0, 5, 0, 0, 0, 9, 0, 0, 0, 60, 1, 0, 0, 6, 0, 0, 0, 3, 0, 0, 0, 132, 1, 0, 0, 1, 32, 0,
    0, 3, 0, 0, 0, 228, 1, 0, 0, 3, 32, 0, 0, 2, 0, 0, 0, 44, 2, 0, 0, 1, 16, 0, 0, 6, 0, 0, 0, 56,
    2, 0, 0, 2, 32, 0, 0, 26, 0, 0, 0, 108, 2, 0, 0, 0, 32, 0, 0, 3, 0, 0, 0, 189, 4, 0, 0, 3, 16,
    0, 0, 1, 0, 0, 0, 240, 4, 0, 0, 0, 16, 0, 0, 1, 0, 0, 0, 244, 4, 0, 0,
];

/// Touch action (`getActionMasked()`), single-pointer subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    Down,   // ACTION_DOWN
    Up,     // ACTION_UP
    Move,   // ACTION_MOVE
    Cancel, // ACTION_CANCEL
}

/// One raw touch event.
///
/// Coordinates: `x/y` are view coordinates (for egui), `raw_x/raw_y` are
/// screen coordinates (for window dragging — unaffected by window movement,
/// avoiding the feedback-loop jitter).
#[derive(Debug, Clone, Copy)]
pub struct TouchEvent {
    pub action: TouchAction,
    pub x_px: f32,
    pub y_px: f32,
    pub raw_x_px: f32,
    pub raw_y_px: f32,
}

static LATEST_RAW: Mutex<Option<(f32, f32)>> = Mutex::new(None);
static TOUCH_QUEUE: Mutex<VecDeque<TouchEvent>> = Mutex::new(VecDeque::new());
static DEX_LOADER: Mutex<Option<jni::objects::Global<jni::objects::JObject<'static>>>> =
    Mutex::new(None);
static MAIN_THREAD_TASK: Mutex<Option<jni::objects::Global<jni::objects::JObject<'static>>>> =
    Mutex::new(None);

/// Queue one touch event (called from the native onTouch callback, main thread).
pub fn push_touch(action: i32, x_px: f32, y_px: f32, raw_x_px: f32, raw_y_px: f32) {
    let action = match action {
        0 => TouchAction::Down,
        1 => TouchAction::Up,
        2 => TouchAction::Move,
        3 => TouchAction::Cancel,
        _ => return, // ignore multi-pointer events for now
    };

    *LATEST_RAW.lock().unwrap_or_else(|p| p.into_inner()) = Some((raw_x_px, raw_y_px));

    TOUCH_QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(TouchEvent {
            action,
            x_px,
            y_px,
            raw_x_px,
            raw_y_px,
        });
}

/// Latest touch position in screen coordinates (px).
pub fn latest_raw_position() -> Option<(f32, f32)> {
    *LATEST_RAW.lock().unwrap_or_else(|p| p.into_inner())
}

/// Drain queued touch events (called once per render frame).
pub fn drain_touches() -> Vec<TouchEvent> {
    let mut queue = TOUCH_QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    queue.drain(..).collect()
}

/// Raw jobject pointer of the cached `MainThreadTask` (Copy, no borrow).
pub fn main_thread_task() -> Option<jni::sys::jobject> {
    let slot = MAIN_THREAD_TASK.lock().unwrap_or_else(|p| p.into_inner());
    slot.as_ref().map(|g| g.as_obj().as_raw())
}

// ============================================================
// Native onTouch callback
// ============================================================

/// `TouchListener.onTouch(View, MotionEvent) -> boolean`. Runs on the main
/// thread; returns `true` to consume the event.
unsafe extern "system" fn native_on_touch(
    mut unowned_env: EnvUnowned<'_>,
    _this: JObject<'_>,
    _view: JObject<'_>,
    event: JObject<'_>,
) -> jboolean {
    let outcome = unowned_env.with_env::<_, _, jni::errors::Error>(|env| {
        let action = env
            .call_method(&event, jni_str!("getActionMasked"), jni_sig!("()I"), &[])?
            .i()?;
        let x = env
            .call_method(&event, jni_str!("getX"), jni_sig!("()F"), &[])?
            .f()?;
        let y = env
            .call_method(&event, jni_str!("getY"), jni_sig!("()F"), &[])?
            .f()?;
        let raw_x = env
            .call_method(&event, jni_str!("getRawX"), jni_sig!("()F"), &[])?
            .f()?;
        let raw_y = env
            .call_method(&event, jni_str!("getRawY"), jni_sig!("()F"), &[])?
            .f()?;

        push_touch(action, x, y, raw_x, raw_y);
        Ok(())
    });

    match outcome.into_outcome() {
        Outcome::Ok(()) => {}
        Outcome::Err(e) => {
            log_error!("[rust] touch callback error: {:?}", e);
            // Clear any Java exception left on the main thread's env: if it
            // stayed pending, every later touch would fail before reaching
            // push_touch, silently killing overlay input.
            let _ = unowned_env.with_env::<_, _, jni::errors::Error>(|env| {
                if env.exception_check() {
                    env.exception_clear();
                }
                Ok(())
            });
        }
        Outcome::Panic(_) => log_error!("[rust] touch callback panicked"),
    }

    JNI_TRUE
}

// ============================================================
// Install
// ============================================================

/// Define the listener classes (InMemoryDexClassLoader), register natives,
/// attach the touch listener and cache the main-thread task instance.
///
/// `app_loader` is the app's ClassLoader (`activity.getClassLoader()`) used
/// as parent loader. Call on the main thread during overlay init.
pub fn install_touch_listener<'local>(
    env: &mut Env<'local>,
    view: &JObject<'local>,
    app_loader: &JObject<'local>,
) -> jni::errors::Result<()> {
    let loader = load_dex(env, app_loader)?;

    let loader_global = env.new_global_ref(&loader)?;
    *DEX_LOADER.lock().unwrap_or_else(|p| p.into_inner()) = Some(loader_global);

    let touch_cls = load_class(env, &loader, "skyhook.TouchListener")?;
    let task_cls = load_class(env, &loader, "skyhook.MainThreadTask")?;
    let callback_cls = load_class(env, &loader, "skyhook.SurfaceCallback")?;

    // SAFETY: native fn signatures match the JNI descriptors.
    let on_touch_method = unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("onTouch"),
            jni_str!("(Landroid/view/View;Landroid/view/MotionEvent;)Z"),
            native_on_touch as *mut std::ffi::c_void,
        )
    };
    unsafe { env.register_native_methods(&touch_cls, &[on_touch_method])? };

    let run_method = unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("run"),
            jni_str!("()V"),
            crate::ui::android::native_main_thread_task_run as *mut std::ffi::c_void,
        )
    };
    unsafe { env.register_native_methods(&task_cls, &[run_method])? };

    // SurfaceHolder.Callback natives -> surface lifecycle state.
    let created_method = unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("surfaceCreated"),
            jni_str!("(Landroid/view/SurfaceHolder;)V"),
            crate::ui::android::surface::native_surface_created as *mut std::ffi::c_void,
        )
    };
    let changed_method = unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("surfaceChanged"),
            jni_str!("(Landroid/view/SurfaceHolder;III)V"),
            crate::ui::android::surface::native_surface_changed as *mut std::ffi::c_void,
        )
    };
    let destroyed_method = unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("surfaceDestroyed"),
            jni_str!("(Landroid/view/SurfaceHolder;)V"),
            crate::ui::android::surface::native_surface_destroyed as *mut std::ffi::c_void,
        )
    };
    unsafe {
        env.register_native_methods(
            &callback_cls,
            &[created_method, changed_method, destroyed_method],
        )?;
    }

    let listener = env.new_object(touch_cls, jni_sig!("()V"), &[])?;
    let task = env.new_object(task_cls, jni_sig!("()V"), &[])?;
    let callback = env.new_object(callback_cls, jni_sig!("()V"), &[])?;

    let task_global = env.new_global_ref(&task)?;
    *MAIN_THREAD_TASK.lock().unwrap_or_else(|p| p.into_inner()) = Some(task_global);

    env.call_method(
        view,
        jni_str!("setOnTouchListener"),
        jni_sig!("(Landroid/view/View$OnTouchListener;)V"),
        &[JValue::Object(&listener)],
    )?;

    // holder.addCallback(callback) — surface lifecycle notifications.
    let holder = env
        .call_method(
            view,
            jni_str!("getHolder"),
            jni_sig!("()Landroid/view/SurfaceHolder;"),
            &[],
        )?
        .l()?;
    env.call_method(
        &holder,
        jni_str!("addCallback"),
        jni_sig!("(Landroid/view/SurfaceHolder$Callback;)V"),
        &[JValue::Object(&callback)],
    )?;

    Ok(())
}

/// Load the embedded DEX via `InMemoryDexClassLoader` (must be a direct
/// ByteBuffer). Returns the loader object.
fn load_dex<'local>(
    env: &mut Env<'local>,
    app_loader: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let bytebuffer_cls = env.find_class(jni_str!("java/nio/ByteBuffer"))?;
    let imdcl_cls = env.find_class(jni_str!("dalvik/system/InMemoryDexClassLoader"))?;

    let buffer = env
        .call_static_method(
            &bytebuffer_cls,
            jni_str!("allocateDirect"),
            jni_sig!("(I)Ljava/nio/ByteBuffer;"),
            &[JValue::Int(TOUCH_LISTENER_DEX_BYTECODE.len() as i32)],
        )?
        .l()?;

    let byte_array = env.byte_array_from_slice(TOUCH_LISTENER_DEX_BYTECODE)?;

    env.call_method(
        &buffer,
        jni_str!("put"),
        jni_sig!("([B)Ljava/nio/ByteBuffer;"),
        &[JValue::Object(&byte_array)],
    )?;
    env.call_method(
        &buffer,
        jni_str!("flip"),
        jni_sig!("()Ljava/nio/ByteBuffer;"),
        &[],
    )?;

    env.new_object(
        &imdcl_cls,
        jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
        &[JValue::Object(&buffer), JValue::Object(app_loader)],
    )
}

/// `loader.loadClass(name)`
fn load_class<'local>(
    env: &mut Env<'local>,
    loader: &JObject<'local>,
    name: &str,
) -> jni::errors::Result<JClass<'local>> {
    let name_jstr = env.new_string(name)?;
    let class_obj = env
        .call_method(
            loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&name_jstr)],
        )?
        .l()?;

    if class_obj.is_null() {
        return Err(jni::errors::Error::NullPtr("loadClass returned null"));
    }

    env.new_cast_local_ref::<JClass<'local>>(&class_obj)
}
