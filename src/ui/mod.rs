//! Overlay UI: Surface-sized window shell with three tabs
//! (About | Console | Settings) and `ui::log!` console logging.

pub mod android;
pub mod console;
pub mod lua;
pub mod renderer;
pub mod settings;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Align, Color32, Context, CornerRadius, Frame, Layout, Margin, RichText, Sense};

use crate::ui::android::input;

/// Fixed logical UI size in points. The Surface window is always sized to
/// `UI_SIZE * PIXELS_PER_POINT`, so egui and the Surface stay identical.
pub const UI_WIDTH: f32 = 600.0;
pub const UI_HEIGHT: f32 = 400.0;
pub const PIXELS_PER_POINT: f32 = 1.6;

/// `ui::log!("...", args)` — append a line to the on-screen Console tab.
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::ui::console::push(format!($($arg)*))
    };
}
pub(crate) use log;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    About,
    Console,
    Settings,
    Lua,
}

static TAB: Mutex<Tab> = Mutex::new(Tab::About);

fn current_tab() -> Tab {
    *TAB.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether the whole window is collapsed to just the title bar.
static COLLAPSED: AtomicBool = AtomicBool::new(false);

pub fn is_collapsed() -> bool {
    COLLAPSED.load(Ordering::Relaxed)
}

/// Reset the collapsed state (window recreated at full size by re-setup).
pub fn reset_collapsed() {
    COLLAPSED.store(false, Ordering::Relaxed);
}

/// Window drag state: absolute positioning using screen coords.
///
/// `target = window_pos_at_press + (current_raw - raw_at_press)`. Raw screen
/// coords are unaffected by window movement, so there is no feedback loop.
#[derive(Debug, Clone, Copy)]
struct DragState {
    start_raw: (f32, f32),
    start_window: (i32, i32),
}

static DRAG: Mutex<Option<DragState>> = Mutex::new(None);

pub fn show_overlay(ctx: &Context) {
    let collapsed = is_collapsed();

    // Fill the whole surface with the panel; the rounded corners stay
    // translucent so the game shows through at the corners.
    let screen = ctx.content_rect();
    let frame = Frame::window(&ctx.style_of(ctx.theme()))
        .fill(Color32::from_rgba_unmultiplied(16, 16, 16, 220))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::same(12));

    egui::Area::new(egui::Id::new("color_panel"))
        .fixed_pos(screen.min)
        .default_size(screen.size())
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());
            // Larger touch targets for all interactive widgets.
            ui.style_mut().spacing.interact_size.y = 26.0;
            frame.show(ui, |ui| {
                let title = ui.horizontal(|ui| {
                    ui.label(RichText::new("Color Panel").strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        toggle_button(ui);
                    });
                });
                handle_title_drag(title.response.interact(Sense::drag()));

                if collapsed {
                    return;
                }

                ui.separator();
                show_tabs(ui);
                ui.separator();
                match current_tab() {
                    Tab::About => tab_body(ui, about_tab),
                    Tab::Console => tab_body(ui, console::show),
                    Tab::Settings => settings::show(ui),
                    Tab::Lua => lua::show(ui),
                }
            });
        });
}

/// Close the keyboard before collapsing the window, restore on expand.
fn set_tab_input_active(tab: Tab, active: bool) {
    match tab {
        Tab::Lua => crate::ui::android::set_lua_input_active(active),
        Tab::Settings => crate::ui::android::set_settings_input_active(active),
        _ => {}
    }
}

/// Collapse/expand toggle: shrinks the whole SurfaceView window to the title
/// bar (and back). The window resize happens on the Android side, so the
/// Surface itself shrinks, not just the drawn UI.
fn toggle_button(ui: &mut egui::Ui) {
    let collapsed = is_collapsed();

    // Painted triangle (the default fonts have no reliable geometric glyphs).
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 22.0), Sense::click());
    let visuals = ui.style().interact(&response);
    let color = visuals.fg_stroke.color;
    let tri = rect.shrink(5.0);
    let points = if collapsed {
        // Pointing up (expand).
        vec![tri.left_bottom(), tri.right_bottom(), tri.center_top()]
    } else {
        // Pointing down (collapse).
        vec![tri.left_top(), tri.right_top(), tri.center_bottom()]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));

    if response.clicked() {
        let next = !collapsed;
        COLLAPSED.store(next, Ordering::Relaxed);
        let tab = current_tab();
        // Close the keyboard before shrinking the window.
        if next {
            set_tab_input_active(tab, false);
            crate::ui::android::resize_surface(
                android::window::DEFAULT_W,
                android::window::COLLAPSED_H,
            );
        } else {
            crate::ui::android::resize_surface(
                android::window::DEFAULT_W,
                android::window::DEFAULT_H,
            );
            // Restore keyboard access if we are still on an input tab.
            set_tab_input_active(tab, true);
        }
    }
}

/// Tab bar: About | Console | Settings | Lua.
///
/// Entering an input tab (Lua/Settings) makes the window focusable and opens
/// the Android keyboard for its native input box; leaving restores
/// `NOT_FOCUSABLE`.
fn show_tabs(ui: &mut egui::Ui) {
    let mut tab = current_tab();

    ui.horizontal(|ui| {
        for (t, label) in [
            (Tab::About, "About"),
            (Tab::Console, "Console"),
            (Tab::Settings, "Settings"),
            (Tab::Lua, "Lua"),
        ] {
            let selected = tab == t;
            // Fixed-size targets: the default selectable label is only as tall
            // as the text, which is too small for a finger.
            if ui
                .add_sized(
                    egui::vec2(68.0, 28.0),
                    egui::Button::selectable(selected, label),
                )
                .clicked()
            {
                tab = t;
            }
        }
    });

    if tab != current_tab() {
        let prev = current_tab();
        // Give/restore keyboard access when switching to/from input tabs.
        set_tab_input_active(prev, false);
        set_tab_input_active(tab, true);
        *TAB.lock().unwrap_or_else(|p| p.into_inner()) = tab;
    }
}

/// Wrap tab content in a field that fills the remaining panel height, so
/// every tab has the same footprint below the tab bar. Transparent fill —
/// the panel's own translucent background stays uniform.
fn tab_body(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let h = ui.available_height().max(80.0);
    egui::Frame::NONE
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_height(h - 20.0);
            add_contents(ui);
        });
}

/// About tab: overlay & info.
fn about_tab(ui: &mut egui::Ui) {
    ui.label(RichText::new("that-sky-ahook overlay").strong());
    ui.label(RichText::new("egui + wgpu on a SurfaceView (TYPE_APPLICATION_PANEL)").weak());

    ui.add_space(4.0);

    ui.hyperlink("https://github.com/flakes-ink/that-sky-ahook");
    ui.label(RichText::new("Author: ColorSkyFun <i@colorsky.fun>").weak());

    ui.add_space(4.0);
}

/// Title-bar drag: record the press, then absolutely position the window.
fn handle_title_drag(response: egui::Response) {
    if response.drag_started() {
        if let Some(raw) = input::latest_raw_position() {
            *DRAG.lock().unwrap_or_else(|p| p.into_inner()) = Some(DragState {
                start_raw: raw,
                start_window: crate::ui::android::window_pos(),
            });
        }
    }

    if response.dragged() {
        let drag = DRAG.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(state) = drag.as_ref() {
            if let Some(raw) = input::latest_raw_position() {
                let x = state.start_window.0 + (raw.0 - state.start_raw.0).round() as i32;
                let y = state.start_window.1 + (raw.1 - state.start_raw.1).round() as i32;
                crate::ui::android::move_surface_to(x, y);
            }
        }
    }

    if response.drag_stopped() {
        *DRAG.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}
