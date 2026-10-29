//! Settings → Teleprompter.

use super::*;

impl SettingsUi {
    pub(super) fn prompter(&mut self, ui: &mut egui::Ui) {
        title(ui, "Teleprompter", "A script that scrolls for the presenters, on the second screen or a tablet. Roll it from the main window, the Stream Deck, the remote or MIDI.");
        let has_rundown = !self.draft.rundown.segments.is_empty();
        let p = &mut self.draft.prompter;
        egui::Grid::new("prompter").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Script");
            ui.vertical(|ui| {
                ui.add(egui::TextEdit::multiline(&mut p.script).desired_width(460.0).desired_rows(12).hint_text("Paste or type the script. A blank line starts a new paragraph (◀ ▶▶ jump between them)."));
                ui.horizontal(|ui| {
                    if ui.button("Load a text file…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Text", &["txt", "md"]).pick_file() {
                            if let Ok(t) = std::fs::read_to_string(&f) {
                                p.script = t;
                            }
                        }
                    }
                    let words = p.script.split_whitespace().count();
                    if words > 0 {
                        hint(ui, &format!("{words} words · about {:.0} min at {:.0} words a minute", words as f32 / p.words_per_minute.max(20.0), p.words_per_minute));
                    }
                });
            });
            ui.end_row();
            label(ui, "Rundown");
            ui.add_enabled(has_rundown, egui::Checkbox::new(&mut p.use_rundown, "follow it, with each segment's notes"));
            ui.end_row();
            label(ui, "Speed");
            ui.add(egui::Slider::new(&mut p.words_per_minute, 60.0..=260.0).step_by(5.0).suffix(" words a minute"));
            ui.end_row();
            label(ui, "Text size");
            ui.add(egui::Slider::new(&mut p.text_size, 0.03..=0.15).custom_formatter(|v, _| format!("{:.0}% of the screen's height", v * 100.0)));
            ui.end_row();
            label(ui, "Mirror");
            ui.checkbox(&mut p.mirror, "flip left to right");
            ui.end_row();
            label(ui, "Colours");
            ui.horizontal(|ui| {
                for (txt, value, fallback) in [("text", &mut p.text_color, [255u8, 255, 255]), ("background", &mut p.background, [0, 0, 0])] {
                    let mut rgb = crate::config::parse_color(value).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or(fallback);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    }
                    ui.label(txt);
                }
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        if self.draft.remote.enabled {
            let ip = crate::engine::netsrc::local_ip().unwrap_or_else(|| "this-computer".into());
            let url = format!("http://{ip}:{}/prompter", self.draft.remote.port);
            ui.horizontal_wrapped(|ui| {
                ui.label("On a tablet or phone on the same Wi-Fi, open");
                ui.label(RichText::new(&url).monospace().color(ACCENT));
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
                ui.label("(tap to roll or pause).");
            });
            hint(ui, "On the second screen: choose \"The teleprompter\" in Screens & webcam.");
        } else {
            hint(ui, "To use it on a tablet or phone, turn on Remote control (Settings → Remote & MIDI) first. On the second screen: choose \"The teleprompter\" in Screens & webcam.");
        }
    }
}
