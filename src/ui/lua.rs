//! Lua tab: script input + execute.
//!
//! Text entry happens in a native (invisible) Android `EditText` overlaid on
//! this tab's input box — it owns the IME connection, so tapping the Lua tab
//! pops the keyboard and typing flows into the game's Lua VM via the
//! `hooks::lua` queue (executed on the game thread).

use crate::ui::{PIXELS_PER_POINT, UI_HEIGHT};

/// Native `EditText` overlay placement, in points from the panel top-left.
/// Must stay in sync with the egui box drawn in [`show`].
pub const INPUT_TOP: f32 = 72.0;
pub const INPUT_BOTTOM: f32 = UI_HEIGHT - 60.0;

/// Render the Lua tab.
pub fn show(ui: &mut egui::Ui) {
    // Pin the input box to the fixed region that the native EditText covers.
    let cursor_y = ui.cursor().top();
    if cursor_y < INPUT_TOP {
        ui.add_space(INPUT_TOP - cursor_y);
    }
    let box_h = (INPUT_BOTTOM - INPUT_TOP - 16.0).max(60.0);
    // Border only (transparent fill) so no darker block shows over the panel.
    // Fill the full content width so the border matches the native EditText.
    egui::Frame::group(ui.style())
        .fill(egui::Color32::TRANSPARENT)
        // .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(ui.available_width(), box_h - 16.0));
        });

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let execute_btn = egui::Button::new(egui::RichText::new("▶ Execute").size(18.0).strong())
            .min_size(egui::vec2(160.0, 36.0));
        if ui.add(execute_btn).clicked() {
            execute();
        }
    });
}

/// Read the script from the native input box and queue it in the game VM.
fn execute() {
    let script = crate::ui::android::lua_input_text().unwrap_or_default();
    if script.trim().is_empty() {
        crate::ui::log!("lua: empty script, nothing queued");
        return;
    }

    crate::hooks::queue_script(&script);
}

/// Px size of the native EditText overlay (matches the egui input box).
pub fn input_rect_px() -> (i32, i32, i32, i32) {
    // (left, top, width, height) — insets match the panel frame margin (12pt).
    let margin = (12.0 * PIXELS_PER_POINT).round() as i32;
    let w = (crate::ui::UI_WIDTH * PIXELS_PER_POINT).round() as i32 - margin * 2;
    let top = (INPUT_TOP * PIXELS_PER_POINT - 8.0).round() as i32;
    let h = ((INPUT_BOTTOM - INPUT_TOP) * PIXELS_PER_POINT + 16.0).round() as i32;
    (margin, top, w, h)
}
