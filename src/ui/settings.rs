//! Settings tab: hook toggles + domain rewrite rules.
//!
//! The rules input is a real egui `TextEdit` inside the scroll area, so it
//! moves with the content. egui cannot open the Android soft keyboard itself,
//! so a tiny (1x1, VISIBLE) native `EditText` owns the IME connection:
//! tapping the box focuses it (keyboard opens) and the text is synced back
//! into the egui box at a low frequency while the tab is visible.

use crate::hooks::settings;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Display buffer for the rules text (synced from the native input box).
static RULES_TEXT: Mutex<String> = Mutex::new(String::new());
/// Last synced native cursor (char idx) — detects caret-only moves (arrow keys).
static LAST_CURSOR: Mutex<usize> = Mutex::new(0);
/// Last sync time — the JNI read is throttled, not per-frame.
static LAST_SYNC: Mutex<Option<Instant>> = Mutex::new(None);

/// Render the Settings tab.
pub fn show(ui: &mut egui::Ui) {
    // Pull text + cursor from the hidden native box (~20 Hz). Rules take
    // effect immediately as you type; leaving the tab also commits (fallback).
    let mut sync_changed = false;
    let mut native_cursor = 0usize;
    let sync_due = {
        let mut last = LAST_SYNC.lock().unwrap_or_else(|p| p.into_inner());
        let due = last
            .map(|t| t.elapsed() >= std::time::Duration::from_millis(50))
            .unwrap_or(true);
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if sync_due {
        if let Some((text, cursor)) = crate::ui::android::settings_input_state() {
            let mut buf = RULES_TEXT.lock().unwrap_or_else(|p| p.into_inner());
            let mut last = LAST_CURSOR.lock().unwrap_or_else(|p| p.into_inner());
            if *buf != text {
                // Text changed (typing): update display + commit, follow caret.
                *buf = text.clone();
                crate::hooks::settings::set_rules(crate::hooks::settings::parse_rules(&text));
                *last = cursor;
                sync_changed = true;
                native_cursor = cursor;
            } else if *last != cursor {
                // Caret moved without text change (arrow keys / IME moves).
                *last = cursor;
                sync_changed = true;
                native_cursor = cursor;
            }
        }
    }

    // Everything scrolls (Console-style) — the input box is a real egui
    // TextEdit, so it moves with the content. Add controls without limit.
    egui::ScrollArea::vertical()
        .id_salt("settings_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_toggles(ui);
            ui.add_space(6.0);

            let mut text = RULES_TEXT.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let resp = ui.add(
                egui::TextEdit::multiline(&mut text)
                    .hint_text("origin[:port] -> target[:port], comma or newline separated")
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
            // Keep any egui-side edits (hardware keyboard) in the buffer.
            *RULES_TEXT.lock().unwrap_or_else(|p| p.into_inner()) = text.clone();

            // Text edits come from the hidden native box. When the text moved
            // (typing), follow the native cursor so the caret advances.
            if sync_changed {
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(native_cursor),
                        )));
                    state.store(ui.ctx(), resp.id);
                }
            }

            // Tapping inside the box moves the egui caret (TextEditState is
            // updated by the widget); mirror it into the native box so typing
            // inserts at the tapped spot. Also opens the keyboard.
            if resp.clicked() || resp.gained_focus() {
                crate::ui::android::set_settings_input_active(true);
                if let Some(state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                    if let Some(range) = state.cursor.char_range() {
                        crate::ui::android::set_settings_cursor(range.primary.index.0);
                    }
                }
            }

            ui.add_space(6.0);
            let count = settings::rules().len();
            ui.label(egui::RichText::new(format!("{count} rule(s) active")).weak());
        });
}

/// Hook toggles, inside the scrollable region.
fn show_toggles(ui: &mut egui::Ui) {
    let mut skip_cert = settings::SKIP_CERT_VERIFY.load(Ordering::Relaxed);
    if ui.checkbox(&mut skip_cert, "Skip cert verify").changed() {
        settings::SKIP_CERT_VERIFY.store(skip_cert, Ordering::Relaxed);
    }

    let mut force_http = settings::FORCE_HTTP.load(Ordering::Relaxed);
    if ui.checkbox(&mut force_http, "Force HTTP").changed() {
        settings::FORCE_HTTP.store(force_http, Ordering::Relaxed);
    }

    let mut rewrite = settings::REWRITE_DOMAIN.load(Ordering::Relaxed);
    if ui.checkbox(&mut rewrite, "Rewrite domain").changed() {
        settings::REWRITE_DOMAIN.store(rewrite, Ordering::Relaxed);
    }
}
