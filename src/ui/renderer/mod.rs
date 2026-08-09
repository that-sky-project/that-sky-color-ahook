//! Renderer: owns the wgpu context and the egui painter, runs the render loop
//! on a dedicated thread.

pub mod egui;
pub mod wgpu;

use crate::ui::android::surface::{self, NativeWindow};
use crate::{log_error, log_info, log_warn};

/// Outcome of one render attempt.
enum FrameResult {
    /// Frame acquired and presented.
    Presented,
    /// Transient failure (timeout/outdated): keep the loop, back off.
    Skipped,
    /// Surface lost: hand back to the worker for re-acquisition.
    Lost,
}

pub struct Renderer {
    /// Holds an ANativeWindow refcount so the `Surface<'static>` cannot dangle.
    _window: NativeWindow,

    wgpu: wgpu::WgpuContext,
    egui: egui::EguiPainter,

    /// Pending size change (debounced: applied only after two stable frames).
    pending_size: Option<(u32, u32)>,
}

impl Renderer {
    /// Blocking entry point for the render thread; returns when the surface is
    /// destroyed (the overlay worker then re-acquires and restarts).
    pub fn run_until_surface_lost(window: NativeWindow) {
        let (width, height) = surface::window_size(&window);
        log_info!("[rust] surface size: {}x{}", width, height);

        match Self::new(window, width, height) {
            Ok(mut renderer) => {
                log_info!("[rust] wgpu renderer ready");
                renderer.render_loop();
            }
            Err(e) => log_error!("[rust] wgpu init FAILED: {:?}", e),
        }
    }

    pub fn new(window: NativeWindow, width: u32, height: u32) -> Result<Self, wgpu::WgpuError> {
        let wgpu = wgpu::WgpuContext::new(&window, width, height)?;
        let egui = egui::EguiPainter::new(wgpu.device(), wgpu.config.format);

        Ok(Self {
            _window: window,
            wgpu,
            egui,
            pending_size: None,
        })
    }

    fn render_loop(&mut self) {
        let mut consecutive_skips = 0u32;

        loop {
            // Stop when the SurfaceView surface is gone (the overlay worker
            // re-acquires and restarts us). `surface_alive` is both the
            // holder callback flag and a self-heal mark after acquisition.
            if !surface::surface_alive() {
                log_warn!("[rust] surface destroyed, stopping renderer");
                return;
            }

            let frame_start = std::time::Instant::now();
            match self.render_frame() {
                FrameResult::Presented => {
                    consecutive_skips = 0;
                    // Cap at ~120fps: Mailbox present mode does not block, so
                    // without this the loop spins at hundreds of fps and
                    // saturates the GPU/CPU (device-wide jank).
                    let elapsed = frame_start.elapsed();
                    if elapsed < std::time::Duration::from_micros(8333) {
                        std::thread::sleep(std::time::Duration::from_micros(8333) - elapsed);
                    }
                }
                FrameResult::Skipped => {
                    consecutive_skips += 1;
                    // Capped backoff so a transient failure (resize, IME
                    // relayout) recovers quickly instead of feeling dead.
                    let delay_ms = if consecutive_skips > 60 { 100 } else { 16 };
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
                FrameResult::Lost => {
                    log_error!("[rust] surface lost, restarting renderer");
                    return;
                }
            }
        }
    }

    /// Render one frame.
    fn render_frame(&mut self) -> FrameResult {
        // egui size tracks the surface size: reconfig on real size changes.
        self.sync_surface_size();

        match self.wgpu.acquire() {
            wgpu::AcquireResult::Frame(frame) => {
                self.render_to(frame);
                FrameResult::Presented
            }
            wgpu::AcquireResult::Skip => FrameResult::Skipped,
            wgpu::AcquireResult::Outdated => {
                log_warn!("[rust] surface outdated, reconfiguring");
                self.wgpu.reconfigure();
                FrameResult::Skipped
            }
            wgpu::AcquireResult::Lost => FrameResult::Lost,
        }
    }

    fn render_to(&mut self, frame: ::wgpu::SurfaceTexture) {
        let view = frame
            .texture
            .create_view(&::wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .wgpu
            .device()
            .create_command_encoder(&::wgpu::CommandEncoderDescriptor::default());

        let size = [self.wgpu.config.width, self.wgpu.config.height];
        let egui_frame = self
            .egui
            .run_frame(self.wgpu.device(), self.wgpu.queue(), size);
        self.egui.paint(
            self.wgpu.device(),
            self.wgpu.queue(),
            &mut encoder,
            &view,
            &egui_frame,
        );

        let command_buffer = encoder.finish();
        self.wgpu.queue().submit([command_buffer]);
        self.wgpu.present(frame);
    }

    /// Debounced surface-size sync: window relayouts can transiently change the
    /// reported size; only reconfigure after two stable frames (avoids swapchain
    /// churn that flickers the UI while dragging).
    fn sync_surface_size(&mut self) {
        let Some((w, h)) = surface::window_size_checked(&self._window) else {
            self.pending_size = None;
            return;
        };

        if (w, h) == (self.wgpu.config.width, self.wgpu.config.height) {
            self.pending_size = None;
            return;
        }

        if self.pending_size == Some((w, h)) {
            self.pending_size = None;
            log_warn!(
                "[rust] surface resized: {}x{} -> {}x{}",
                self.wgpu.config.width,
                self.wgpu.config.height,
                w,
                h
            );
            self.wgpu.config.width = w;
            self.wgpu.config.height = h;
            self.wgpu.reconfigure();
        } else {
            self.pending_size = Some((w, h));
        }
    }
}
