//! Settings → Rundown: the show's running order.

use super::*;
use crate::config::Segment;

/// Edit a list of cue commands (for scenes and rundown segments): pick from
/// what this show has, or type any command.
pub(super) fn cues_editor(ui: &mut egui::Ui, id: &str, cues: &mut Vec<String>, suggestions: &[(String, String)]) {
    commands_editor(ui, id, cues, suggestions, false);
}

/// Edit a list of commands; `scenes`: scene changes are allowed (macros).
pub(super) fn commands_editor(ui: &mut egui::Ui, id: &str, cues: &mut Vec<String>, suggestions: &[(String, String)], scenes: bool) {
    let label_of = |c: &str| suggestions.iter().find(|(cmd, _)| cmd.eq_ignore_ascii_case(c)).map(|(_, l)| l.clone()).or_else(|| crate::control::Action::parse(c).ok().map(|a| a.describe())).unwrap_or_else(|| c.to_string());
    let mut remove = None;
    let mut swap = None;
    let n = cues.len();
    for (i, c) in cues.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if scenes {
                if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                    swap = Some(i - 1);
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                    swap = Some(i);
                }
            }
            ui.add(egui::TextEdit::singleline(c).desired_width(200.0).font(egui::TextStyle::Monospace));
            let parsed = crate::control::Action::parse(c);
            match parsed {
                Ok(_) => {
                    ui.label(RichText::new(label_of(c)).color(TEXT_DIM).size(13.0));
                }
                Err(e) => {
                    ui.label(RichText::new(format!("⚠ {e}")).color(AMBER).size(13.0));
                }
            }
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        cues.remove(i);
    }
    if let Some(i) = swap {
        cues.swap(i, i + 1);
    }
    let add = if scenes { "＋ Add a step…" } else { "＋ Add a cue…" };
    egui::ComboBox::from_id_salt(("add_cue", id)).selected_text(add).width(240.0).height(360.0).show_ui(ui, |ui| {
        let skip = |c: &str| !scenes && (["scene:", "cut:", "preview:", "source:", "rundown:"].iter().any(|p| c.starts_with(p)) || c == "take" || c == "cut");
        for (secs, l) in [("0.5", "Wait half a second"), ("1", "Wait 1 second"), ("2", "Wait 2 seconds"), ("5", "Wait 5 seconds")] {
            if ui.selectable_label(false, l).on_hover_text(format!("wait:{secs}")).clicked() {
                cues.push(format!("wait:{secs}"));
            }
        }
        ui.separator();
        for (cmd, l) in suggestions.iter().filter(|(c, _)| !skip(c)) {
            if ui.selectable_label(false, l).on_hover_text(cmd).clicked() {
                cues.push(cmd.clone());
            }
        }
        if ui.selectable_label(false, "Other (type a command)").clicked() {
            cues.push(String::new());
        }
    });
}

fn fmt_len(secs: u32) -> String {
    if secs == 0 {
        "open".into()
    } else {
        crate::engine::timers::fmt_span(std::time::Duration::from_secs(secs as u64), false)
    }
}

impl SettingsUi {
    pub(super) fn rundown(&mut self, ui: &mut egui::Ui) {
        title(ui, "Rundown", "The show's running order: each segment's length, scene and cues. Step through it with N, the main window, the Stream Deck or the remote.");
        let n = self.draft.rundown.segments.len();
        if self.sel_segment >= n {
            self.sel_segment = n.saturating_sub(1);
        }
        let total = crate::control::rundown::planned_total(&self.draft.rundown.segments);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(240.0);
                for i in 0..n {
                    let s = &self.draft.rundown.segments[i];
                    let b = egui::Button::selectable(self.sel_segment == i, RichText::new(format!("{}. {}\n{}", i + 1, s.name, fmt_len(s.seconds))).size(14.0)).min_size(vec2(240.0, 40.0));
                    if ui.add(b).clicked() {
                        self.sel_segment = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("＋ Segment").clicked() {
                        let name = unique(self.draft.rundown.segments.iter().map(|s| s.name.clone()).collect(), if n == 0 { "Intro" } else { "Segment" });
                        let at = if n == 0 { 0 } else { self.sel_segment + 1 };
                        self.draft.rundown.segments.insert(at, Segment { name, ..Default::default() });
                        self.sel_segment = at;
                    }
                    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
                        self.draft.rundown.segments.remove(self.sel_segment);
                    }
                    if ui.add_enabled(n > 0 && self.sel_segment > 0, egui::Button::new("↑")).clicked() {
                        self.draft.rundown.segments.swap(self.sel_segment, self.sel_segment - 1);
                        self.sel_segment -= 1;
                    }
                    if ui.add_enabled(n > 0 && self.sel_segment + 1 < n, egui::Button::new("↓")).clicked() {
                        self.draft.rundown.segments.swap(self.sel_segment, self.sel_segment + 1);
                        self.sel_segment += 1;
                    }
                });
                if n > 0 {
                    ui.add_space(6.0);
                    hint(ui, &format!("Planned: {}", crate::engine::timers::fmt_span(std::time::Duration::from_secs(total), false)));
                }
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let scenes: Vec<String> = self.draft.effective_scenes().iter().map(|s| s.name.clone()).collect();
                let suggestions = crate::control::suggestions(&self.draft);
                let Some(s) = self.draft.rundown.segments.get_mut(self.sel_segment) else {
                    hint(ui, "No segments yet. Add one for each part of the show — intro, interview, ad break, outro…");
                    return;
                };
                egui::Grid::new("segment").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    label(ui, "Name");
                    ui.add_sized([260.0, 22.0], egui::TextEdit::singleline(&mut s.name));
                    ui.end_row();
                    label(ui, "Length");
                    ui.horizontal(|ui| {
                        let mut m = s.seconds / 60;
                        let mut sec = s.seconds % 60;
                        let a = ui.add(egui::DragValue::new(&mut m).range(0..=600).suffix(" min"));
                        let b = ui.add(egui::DragValue::new(&mut sec).range(0..=59).suffix(" s"));
                        if a.changed() || b.changed() {
                            s.seconds = m * 60 + sec;
                        }
                        hint(ui, if s.seconds == 0 { "open-ended" } else { "" });
                    });
                    ui.end_row();
                    label(ui, "Scene");
                    let shown = if s.scene.is_empty() { "stay on the current scene".to_string() } else { s.scene.clone() };
                    egui::ComboBox::from_id_salt("seg_scene").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.scene, String::new(), "stay on the current scene");
                        for name in &scenes {
                            ui.selectable_value(&mut s.scene, name.clone(), name);
                        }
                    });
                    ui.end_row();
                    label(ui, "Cues");
                    ui.vertical(|ui| cues_editor(ui, &format!("seg{}", self.sel_segment), &mut s.cues, &suggestions));
                    ui.end_row();
                    label(ui, "Notes");
                    ui.add(egui::TextEdit::multiline(&mut s.notes).desired_width(340.0).desired_rows(4).hint_text("talking points, who's on, what to set up"));
                    ui.end_row();
                });
            });
        });
        ui.add_space(12.0);
        egui::Grid::new("rundown_marks").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Chapters");
            ui.checkbox(&mut self.draft.rundown.markers, "mark each segment in the recording");
            ui.end_row();
        });
        hint(ui, "For a countdown on the program, add a clock graphic showing \"Rundown segment time left\"; for the presenters only, turn on clocks on the second screen.");
    }
}
