//! Panel window: FrameLayout{SurfaceView, EditText} + WindowManager.LayoutParams.

use jni::{Env, JValue, jni_sig, jni_str, objects::JObject};

const TYPE_APPLICATION_PANEL: i32 = 1000;
const FLAG_NOT_FOCUSABLE: i32 = 0x8;
const FLAG_NOT_TOUCH_MODAL: i32 = 0x20;
const PIXEL_FORMAT_TRANSLUCENT: i32 = -3;

/// Overlay window size in px. Derived from the fixed UI size so the Surface
/// always matches egui: `DEFAULT = UI_SIZE * PIXELS_PER_POINT`.
pub const DEFAULT_W: i32 = (crate::ui::UI_WIDTH * crate::ui::PIXELS_PER_POINT) as i32;
pub const DEFAULT_H: i32 = (crate::ui::UI_HEIGHT * crate::ui::PIXELS_PER_POINT) as i32;
/// Window height in px when collapsed to just the title bar.
pub const COLLAPSED_H: i32 = 80;

/// `new SurfaceView(activity)`
pub fn create_surface_view<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let cls = env.find_class(jni_str!("android/view/SurfaceView"))?;

    env.new_object(
        cls,
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(activity)],
    )
}

/// `new EditText(activity)` styled as an invisible, multi-line input relay.
///
/// It sits above the surface (sibling in the FrameLayout) and owns the IME
/// connection for both input tabs (Lua script / Settings rules); it is
/// repositioned per-tab via `ViewGroup.updateViewLayout`. Its text is read via
/// [`crate::ui::android::lua_input_text`].
pub fn create_edit_text<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let cls = env.find_class(jni_str!("android/widget/EditText"))?;
    let et = env.new_object(
        cls,
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(activity)],
    )?;

    // Transparent so the egui box shows through; light text on the dark panel.
    env.call_method(
        &et,
        jni_str!("setBackgroundColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x00000000)],
    )?;
    env.call_method(
        &et,
        jni_str!("setTextColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0xFFE6E6E6u32 as i32)],
    )?;
    env.call_method(
        &et,
        jni_str!("setHintTextColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0xFF707070u32 as i32)],
    )?;

    // TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_MULTI_LINE
    env.call_method(
        &et,
        jni_str!("setInputType"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x00020001)],
    )?;
    env.call_method(
        &et,
        jni_str!("setSingleLine"),
        jni_sig!("(Z)V"),
        &[JValue::Bool(false)],
    )?;

    // Gravity.TOP | Gravity.START
    env.call_method(
        &et,
        jni_str!("setGravity"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x30 | 0x800003)],
    )?;
    env.call_method(
        &et,
        jni_str!("setTextSize"),
        jni_sig!("(F)V"),
        &[JValue::Float(13.0)],
    )?;

    // Typeface.MONOSPACE
    let tf = env
        .get_static_field(
            jni_str!("android/graphics/Typeface"),
            jni_str!("MONOSPACE"),
            jni_sig!("Landroid/graphics/Typeface;"),
        )?
        .l()?;
    env.call_method(
        &et,
        jni_str!("setTypeface"),
        jni_sig!("(Landroid/graphics/Typeface;)V"),
        &[JValue::Object(&tf)],
    )?;

    // Hint shown while empty.
    let hint = env.new_string("sle.log('hello world')")?;
    env.call_method(
        &et,
        jni_str!("setHint"),
        jni_sig!("(Ljava/lang/CharSequence;)V"),
        &[JValue::Object(&hint)],
    )?;

    // Hidden (GONE) until the Lua tab asks for input; otherwise it would
    // swallow touches in its region on every tab.
    env.call_method(
        &et,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)],
    )?;

    Ok(et)
}

/// `new EditText(activity)` styled like the Lua relay, for the Settings
/// domain-rules box (multi-line, hidden until the Settings tab is focused).
/// Position is fixed at attach time — the EditTexts are never moved at
/// runtime (updateViewLayout on a child stalls the game's main thread).
pub fn create_rule_edit<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let cls = env.find_class(jni_str!("android/widget/EditText"))?;
    let et = env.new_object(
        cls,
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(activity)],
    )?;

    // Transparent so the egui box shows through; light text on the dark panel.
    env.call_method(
        &et,
        jni_str!("setBackgroundColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x00000000)],
    )?;
    env.call_method(
        &et,
        jni_str!("setTextColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0xFFE6E6E6u32 as i32)],
    )?;
    env.call_method(
        &et,
        jni_str!("setHintTextColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0xFF707070u32 as i32)],
    )?;

    // TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_MULTI_LINE
    env.call_method(
        &et,
        jni_str!("setInputType"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x00020001)],
    )?;
    env.call_method(
        &et,
        jni_str!("setSingleLine"),
        jni_sig!("(Z)V"),
        &[JValue::Bool(false)],
    )?;

    // Gravity.TOP | Gravity.START
    env.call_method(
        &et,
        jni_str!("setGravity"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x30 | 0x800003)],
    )?;
    env.call_method(
        &et,
        jni_str!("setTextSize"),
        jni_sig!("(F)V"),
        &[JValue::Float(13.0)],
    )?;

    // Typeface.MONOSPACE
    let tf = env
        .get_static_field(
            jni_str!("android/graphics/Typeface"),
            jni_str!("MONOSPACE"),
            jni_sig!("Landroid/graphics/Typeface;"),
        )?
        .l()?;
    env.call_method(
        &et,
        jni_str!("setTypeface"),
        jni_sig!("(Landroid/graphics/Typeface;)V"),
        &[JValue::Object(&tf)],
    )?;

    // Hint shown while empty.
    let hint = env.new_string("origin[:port] -> target[:port], comma separated")?;
    env.call_method(
        &et,
        jni_str!("setHint"),
        jni_sig!("(Ljava/lang/CharSequence;)V"),
        &[JValue::Object(&hint)],
    )?;

    // Hidden (GONE) until the Settings tab asks for input.
    env.call_method(
        &et,
        jni_str!("setVisibility"),
        jni_sig!("(I)V"),
        &[JValue::Int(8)],
    )?;

    Ok(et)
}

/// `new FrameLayout(activity)` with the SurfaceView (fill) and the two
/// EditText relays (Lua script + Settings rules) as children. Returns the
/// frame layout.
pub fn wrap_in_frame_layout<'local>(
    env: &mut Env<'local>,
    activity: &JObject<'local>,
    surface_view: &JObject<'local>,
    edit_text: &JObject<'local>,
    rule_edit: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let fl_cls = env.find_class(jni_str!("android/widget/FrameLayout"))?;
    let frame = env.new_object(
        &fl_cls,
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(activity)],
    )?;

    // SurfaceView: fill the window.
    let lp_cls = env.find_class(jni_str!("android/widget/FrameLayout$LayoutParams"))?;
    let fill = env.new_object(
        &lp_cls,
        jni_sig!("(II)V"),
        &[JValue::Int(-1), JValue::Int(-1)],
    )?;
    env.call_method(
        &frame,
        jni_str!("addView"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(surface_view), JValue::Object(&fill)],
    )?;

    // EditText: MATCH_PARENT width, positioned over the Lua input box.
    let (left, top, w, h) = crate::ui::lua::input_rect_px();
    let et_lp = env.new_object(
        &lp_cls,
        jni_sig!("(II)V"),
        &[JValue::Int(-1), JValue::Int(h)],
    )?;
    env.set_field(
        &et_lp,
        jni_str!("leftMargin"),
        jni_sig!("I"),
        JValue::Int(left),
    )?;
    env.set_field(
        &et_lp,
        jni_str!("rightMargin"),
        jni_sig!("I"),
        JValue::Int(left),
    )?;
    env.set_field(
        &et_lp,
        jni_str!("topMargin"),
        jni_sig!("I"),
        JValue::Int(top),
    )?;
    // The LayoutParams width is set below (MATCH_PARENT already implies
    // parent-minus-margins, so leave it).
    let _ = w;
    env.call_method(
        &frame,
        jni_str!("addView"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(edit_text), JValue::Object(&et_lp)],
    )?;

    // Rule EditText: a tiny (1x1) box that owns the IME connection for the
    // Settings tab. The typed text is displayed in the egui TextEdit instead,
    // so it must be effectively invisible yet stay VISIBLE — showSoftInput
    // rejects non-VISIBLE views (isShown). Never moved at runtime.
    let rule_lp = env.new_object(
        &lp_cls,
        jni_sig!("(II)V"),
        &[JValue::Int(1), JValue::Int(1)],
    )?;
    env.call_method(
        &frame,
        jni_str!("addView"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(rule_edit), JValue::Object(&rule_lp)],
    )?;

    Ok(frame)
}

/// `new WindowManager.LayoutParams()` with panel settings.
///
/// Flags must include `FLAG_NOT_FOCUSABLE | FLAG_NOT_TOUCH_MODAL`:
/// - without NOT_FOCUSABLE the panel steals focus from the game (Unity stops
///   handling input);
/// - without NOT_TOUCH_MODAL the panel is touch-modal and swallows all touches
///   outside its bounds.
/// (While an input tab is editing, NOT_FOCUSABLE is temporarily cleared — the
/// IME does not open on NOT_FOCUSABLE windows on this device; the flag is
/// restored when the tab is left.)
pub fn create_layout_params<'local>(
    env: &mut Env<'local>,
    token: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let cls = env.find_class(jni_str!("android/view/WindowManager$LayoutParams"))?;

    let params = env.new_object(cls, jni_sig!("()V"), &[])?;

    env.set_field(
        &params,
        jni_str!("width"),
        jni_sig!("I"),
        JValue::Int(DEFAULT_W),
    )?;
    env.set_field(
        &params,
        jni_str!("height"),
        jni_sig!("I"),
        JValue::Int(DEFAULT_H),
    )?;
    env.set_field(
        &params,
        jni_str!("type"),
        jni_sig!("I"),
        JValue::Int(TYPE_APPLICATION_PANEL),
    )?;
    env.set_field(
        &params,
        jni_str!("flags"),
        jni_sig!("I"),
        JValue::Int(FLAG_NOT_FOCUSABLE | FLAG_NOT_TOUCH_MODAL),
    )?;
    env.set_field(
        &params,
        jni_str!("format"),
        jni_sig!("I"),
        JValue::Int(PIXEL_FORMAT_TRANSLUCENT),
    )?;
    env.set_field(&params, jni_str!("x"), jni_sig!("I"), JValue::Int(100))?;
    env.set_field(&params, jni_str!("y"), jni_sig!("I"), JValue::Int(200))?;
    env.set_field(
        &params,
        jni_str!("token"),
        jni_sig!("Landroid/os/IBinder;"),
        JValue::Object(token),
    )?;

    Ok(params)
}

/// `wm.addView(view, params)` — must run on the main thread.
pub fn add_view<'local>(
    env: &mut Env<'local>,
    wm: &JObject<'local>,
    view: &JObject<'local>,
    params: &JObject<'local>,
) -> jni::errors::Result<()> {
    env.call_method(
        wm,
        jni_str!("addView"),
        jni_sig!("(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V"),
        &[JValue::Object(view), JValue::Object(params)],
    )?;

    Ok(())
}

/// Make the view background fully transparent.
///
/// The SurfaceView view background renders *below* the surface; without this,
/// transparent pixels of our alpha=0 clear would show the (black) view
/// background instead of the game.
pub fn set_transparent_background<'local>(
    env: &mut Env<'local>,
    panel: &JObject<'local>,
) -> jni::errors::Result<()> {
    env.call_method(
        panel,
        jni_str!("setBackgroundColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(0x00000000)], // Color.TRANSPARENT
    )?;

    Ok(())
}

/// `holder.setFormat(PixelFormat.TRANSLUCENT)`.
///
/// The SurfaceView surface defaults to OPAQUE; without this, SurfaceFlinger
/// ignores the buffer alpha and alpha=0 pixels render black. Must be called
/// before the surface is created (right after addView).
pub fn set_translucent_format<'local>(
    env: &mut Env<'local>,
    panel: &JObject<'local>,
) -> jni::errors::Result<()> {
    let holder = env
        .call_method(
            panel,
            jni_str!("getHolder"),
            jni_sig!("()Landroid/view/SurfaceHolder;"),
            &[],
        )?
        .l()?;

    env.call_method(
        &holder,
        jni_str!("setFormat"),
        jni_sig!("(I)V"),
        &[JValue::Int(PIXEL_FORMAT_TRANSLUCENT)],
    )?;

    Ok(())
}
