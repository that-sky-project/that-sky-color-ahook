//! egui context + egui-wgpu painter, and touch -> egui event mapping.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ui::android::input::{TouchAction, TouchEvent, drain_touches};
use crate::ui::show_overlay;
use crate::ui::{PIXELS_PER_POINT, UI_HEIGHT, UI_WIDTH};

pub struct EguiPainter {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    /// Logical scale; the Surface window is sized to `UI_SIZE * ppp`.
    pixels_per_point: f32,
}

/// One frame of egui output ready for painting.
pub struct EguiFrame {
    pub paint_jobs: Vec<egui::ClippedPrimitive>,
    pub screen_descriptor: egui_wgpu::ScreenDescriptor,
}

/// Touch gesture id tracked across frames (stable per finger-down-to-up).
static NEXT_TOUCH_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TOUCH_ID: AtomicU64 = AtomicU64::new(0);

impl EguiPainter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // egui-wgpu 0.36 takes RendererOptions (msaa/dithering booleans are gone).
        let renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());

        let ctx = egui::Context::default();
        // Touch-friendlier click detection: finger taps drift and linger more
        // than mouse clicks. Never let hold duration disqualify a click (a
        // resting finger would otherwise produce a long-press, not a tap).
        ctx.memory_mut(|m| {
            let o = &mut m.options.input_options;
            o.max_click_dist = 12.0; // points (default 6.0)
            o.max_click_duration = f64::INFINITY; // any hold still clicks
        });

        Self {
            ctx,
            renderer,
            pixels_per_point: PIXELS_PER_POINT,
        }
    }

    /// Run one egui frame: drain touches, build RawInput, run the UI, update
    /// textures and tessellate. `size_px` is the surface size.
    pub fn run_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size_px: [u32; 2],
    ) -> EguiFrame {
        let ppp = self.pixels_per_point;

        let screen_points = [UI_WIDTH, UI_HEIGHT];

        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(screen_points[0], screen_points[1]),
            )),
            events: touch_to_egui_events(drain_touches(), ppp),
            ..Default::default()
        };
        if let Some(viewport) = raw_input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }

        // egui 0.36 renamed Context::run to run_ui (callback gets &mut Ui).
        let full_output = self.ctx.run_ui(raw_input, |ui| {
            show_overlay(ui.ctx());
        });

        // Textures: egui-wgpu 0.36 updates per (TextureId, ImageDelta).
        let textures_delta = full_output.textures_delta;
        for (tex_id, deltas) in &textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *tex_id, delta);
            }
        }

        let paint_jobs = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for tex_id in &textures_delta.free {
            self.renderer.free_texture(tex_id);
        }

        EguiFrame {
            paint_jobs,
            screen_descriptor: egui_wgpu::ScreenDescriptor {
                size_in_pixels: size_px,
                pixels_per_point: ppp,
            },
        }
    }

    /// Upload buffers and draw the egui pass into `view`.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &EguiFrame,
    ) {
        self.renderer.update_buffers(
            device,
            queue,
            encoder,
            &frame.paint_jobs,
            &frame.screen_descriptor,
        );

        let rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        // egui-wgpu 0.36 requires RenderPass<'static>; wgpu 30 provides
        // forget_lifetime for exactly this.
        self.renderer.render(
            &mut rp.forget_lifetime(),
            &frame.paint_jobs,
            &frame.screen_descriptor,
        );
    }
}

/// Clear color: alpha=0 so the game shows through outside the panel
/// (window format is TRANSLUCENT; SurfaceFlinger composites the buffer alpha).
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// Map raw touches to egui events (px / ppp = points).
///
/// Emits pointer events (interaction) AND `Event::Touch` (marks the screen as
/// a touch screen, which enables egui's drag-to-scroll in ScrollAreas).
fn touch_to_egui_events(touches: Vec<TouchEvent>, ppp: f32) -> Vec<egui::Event> {
    let mut events = Vec::with_capacity(touches.len() * 2);

    for touch in touches {
        let pos = egui::pos2(touch.x_px / ppp, touch.y_px / ppp);

        // Touch events: stable gesture id across frames.
        let (phase, touch_id) = match touch.action {
            TouchAction::Down => {
                // A previous gesture may never have ended (its Up/Cancel was
                // dropped while the window relaid out, e.g. IME open/close).
                // Close it first, or egui would treat this press as a second
                // finger and enter multi-touch mode, ignoring taps.
                let prev = ACTIVE_TOUCH_ID.swap(0, Ordering::Relaxed);
                if prev != 0 {
                    events.push(egui::Event::Touch {
                        device_id: egui::TouchDeviceId(0),
                        id: egui::TouchId(prev),
                        phase: egui::TouchPhase::End,
                        pos,
                        force: None,
                    });
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    });
                    events.push(egui::Event::PointerGone);
                }
                let id = NEXT_TOUCH_ID.fetch_add(1, Ordering::Relaxed);
                ACTIVE_TOUCH_ID.store(id, Ordering::Relaxed);
                (egui::TouchPhase::Start, id)
            }
            TouchAction::Move => (
                egui::TouchPhase::Move,
                ACTIVE_TOUCH_ID.load(Ordering::Relaxed),
            ),
            TouchAction::Up | TouchAction::Cancel => (
                egui::TouchPhase::End,
                ACTIVE_TOUCH_ID.swap(0, Ordering::Relaxed),
            ),
        };
        if touch_id != 0 {
            events.push(egui::Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(touch_id),
                phase,
                pos,
                force: None,
            });
        }

        // Pointer events drive widgets.
        match touch.action {
            TouchAction::Down => events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }),
            TouchAction::Move => events.push(egui::Event::PointerMoved(pos)),
            TouchAction::Up | TouchAction::Cancel => {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
                // egui recommends PointerGone after release to clear hover.
                events.push(egui::Event::PointerGone);
            }
        }
    }

    events
}
