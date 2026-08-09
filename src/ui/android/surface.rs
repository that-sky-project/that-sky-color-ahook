//! SurfaceView -> Surface -> ANativeWindow, and surface lifecycle state.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use jni::{Env, EnvUnowned, jni_sig, jni_str, objects::JObject};

/// Opaque ANativeWindow type.
#[repr(C)]
pub struct ANativeWindow {
    _private: [u8; 0],
}

// Linked from libandroid (minSdk 21 sysroot); the `nativewindow` feature is
// unavailable below API 26 so we declare the FFI ourselves with jni's sys
// types (no casts needed).
#[link(name = "android")]
unsafe extern "C" {
    /// Returns an owned reference (must be released with `ANativeWindow_release`).
    pub fn ANativeWindow_fromSurface(
        env: *mut jni::sys::JNIEnv,
        surface: jni::sys::jobject,
    ) -> *mut ANativeWindow;
    pub fn ANativeWindow_acquire(window: *mut ANativeWindow);
    pub fn ANativeWindow_release(window: *mut ANativeWindow);
    pub fn ANativeWindow_getWidth(window: *mut ANativeWindow) -> i32;
    pub fn ANativeWindow_getHeight(window: *mut ANativeWindow) -> i32;
}

/// RAII wrapper over an owned `ANativeWindow*` (release on drop).
pub struct NativeWindow {
    ptr: *mut ANativeWindow,
}

// ANativeWindow methods are thread-safe per NDK docs.
unsafe impl Send for NativeWindow {}
unsafe impl Sync for NativeWindow {}

impl NativeWindow {
    /// # Safety
    /// `ptr` must be a valid, unreleased `ANativeWindow*` not owned elsewhere.
    pub unsafe fn from_raw(ptr: *mut ANativeWindow) -> Option<Self> {
        (!ptr.is_null()).then_some(Self { ptr })
    }

    pub fn as_ptr(&self) -> *mut ANativeWindow {
        self.ptr
    }
}

impl Clone for NativeWindow {
    fn clone(&self) -> Self {
        unsafe { ANativeWindow_acquire(self.ptr) };
        Self { ptr: self.ptr }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe { ANativeWindow_release(self.ptr) };
    }
}

// ============================================================
// Surface lifecycle state (updated by SurfaceHolder.Callback)
// ============================================================

static SURFACE_ALIVE: AtomicBool = AtomicBool::new(false);
static SURFACE_W: AtomicU32 = AtomicU32::new(0);
static SURFACE_H: AtomicU32 = AtomicU32::new(0);

/// Whether the SurfaceView surface currently exists.
pub fn surface_alive() -> bool {
    SURFACE_ALIVE.load(Ordering::Relaxed)
}

/// Mark the surface alive after a successful ANativeWindow acquisition.
///
/// The SurfaceHolder callback can miss a recreate (e.g. after a view
/// re-attach), leaving `SURFACE_ALIVE` stuck false even though the surface
/// was just validated — which would make the renderer exit instantly.
pub fn mark_alive() {
    SURFACE_ALIVE.store(true, Ordering::Relaxed);
}

/// Last known surface size from `surfaceChanged` (px).
pub fn surface_size() -> (u32, u32) {
    (
        SURFACE_W.load(Ordering::Relaxed),
        SURFACE_H.load(Ordering::Relaxed),
    )
}

/// `SurfaceCallback.surfaceCreated` — runs on the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn native_surface_created(
    mut _env: EnvUnowned<'_>,
    _this: JObject<'_>,
    _holder: JObject<'_>,
) {
    SURFACE_ALIVE.store(true, Ordering::Relaxed);
    crate::log_info!("[rust] surface created");
}

/// `SurfaceCallback.surfaceChanged` — runs on the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn native_surface_changed(
    mut _env: EnvUnowned<'_>,
    _this: JObject<'_>,
    _holder: JObject<'_>,
    _format: i32,
    width: i32,
    height: i32,
) {
    SURFACE_ALIVE.store(true, Ordering::Relaxed);
    SURFACE_W.store(width.max(0) as u32, Ordering::Relaxed);
    SURFACE_H.store(height.max(0) as u32, Ordering::Relaxed);
    crate::log_info!("[rust] surface changed: {}x{}", width, height);
}

/// `SurfaceCallback.surfaceDestroyed` — runs on the main thread.
///
/// The render thread polls this and stops rendering; the overlay worker then
/// re-acquires a fresh ANativeWindow once the surface is recreated.
#[unsafe(no_mangle)]
pub extern "system" fn native_surface_destroyed(
    mut _env: EnvUnowned<'_>,
    _this: JObject<'_>,
    _holder: JObject<'_>,
) {
    SURFACE_ALIVE.store(false, Ordering::Relaxed);
    crate::log_warn!("[rust] surface destroyed");
}

/// Current window size in px, falling back to the last `surfaceChanged` size
/// (or the default window size) while the native window reports 0.
pub fn window_size(window: &NativeWindow) -> (u32, u32) {
    if let Some(size) = window_size_checked(window) {
        return size;
    }
    let (w, h) = surface_size();
    if w > 0 && h > 0 {
        (w, h)
    } else {
        (
            crate::ui::android::window::DEFAULT_W as u32,
            crate::ui::android::window::DEFAULT_H as u32,
        )
    }
}

/// Current window size in px, or `None` while invalid (e.g. during relayout).
pub fn window_size_checked(window: &NativeWindow) -> Option<(u32, u32)> {
    let w = unsafe { ANativeWindow_getWidth(window.as_ptr()) };
    let h = unsafe { ANativeWindow_getHeight(window.as_ptr()) };
    (w > 0 && h > 0).then_some((w as u32, h as u32))
}

// ============================================================
// Surface acquisition (JNI)
// ============================================================

/// `surfaceView.getHolder().getSurface()`
pub fn get_surface<'local>(
    env: &mut Env<'local>,
    surface_view: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let holder = env
        .call_method(
            surface_view,
            jni_str!("getHolder"),
            jni_sig!("()Landroid/view/SurfaceHolder;"),
            &[],
        )?
        .l()?;

    if holder.is_null() {
        return Err(jni::errors::Error::NullPtr("SurfaceHolder is null"));
    }

    env.call_method(
        &holder,
        jni_str!("getSurface"),
        jni_sig!("()Landroid/view/Surface;"),
        &[],
    )
    .and_then(|v| v.l())
}

/// `surface.isValid()`
pub fn surface_is_valid<'local>(
    env: &mut Env<'local>,
    surface: &JObject<'local>,
) -> jni::errors::Result<bool> {
    if surface.is_null() {
        return Ok(false);
    }
    env.call_method(surface, jni_str!("isValid"), jni_sig!("()Z"), &[])
        .map(|v| v.z().unwrap_or(false))
}

/// Poll until the surface is valid (it is created asynchronously after
/// addView), bounded by `timeout`. Poll from a background thread — the main
/// thread must stay free to run layout.
pub fn wait_for_valid_surface<'local>(
    env: &mut Env<'local>,
    surface_view: &JObject<'local>,
    timeout: std::time::Duration,
) -> jni::errors::Result<JObject<'local>> {
    let deadline = std::time::Instant::now() + timeout;

    loop {
        let surface = get_surface(env, surface_view)?;
        if surface_is_valid(env, &surface)? {
            return Ok(surface);
        }
        if std::time::Instant::now() >= deadline {
            return Err(jni::errors::Error::NullPtr(
                "Surface not valid within timeout",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
