//! Settings → Graphics → Taking turns: graphics that come on one after
//! another every few minutes (sponsors, social handles, reminders).

use super::*;
use crate::config::{Rotation, RotationStart};

impl SettingsUi {
    pub(super) fn rotations(&mut self, ui: &mut egui::Ui) {
        let n = self.draft.rotations.len();
        let state = if n == 0 { "none".to_string() } else { self.draft.rotations.iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(", ") };
        let names: Vec<String> = self.draft.graphics.iter().map(|g| g.name.clone()).collect();
        part(ui, "rotations", "Taking turns", &state, n > 0, |ui| {
            hint(ui, "Graphics that come on one after another by themselves: sponsors, your social handles, a reminder to subscribe. Each still has its own button too.");
            let mut remove = None;
            for (ri, r) in self.draft.rotations.iter_mut().enumerate() {
                ui.add_space(6.0);
                egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    egui::Grid::new(("rotation", ri)).num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                        label(ui, "Group name");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(180.0));
                            if ui.small_button("Remove").clicked() {
                                remove = Some(ri);
                            }
                        });
                        ui.end_row();

                        label(ui, "Graphics");
                        ui.vertical(|ui| {
                            let mut drop = None;
                            let mut up = None;
                            let len = r.graphics.len();
                            for (gi, g) in r.graphics.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}.", gi + 1));
                                    if names.contains(g) {
                                        ui.label(RichText::new(g).strong());
                                    } else {
                                        ui.label(RichText::new(format!("⚠ {g}: there's no graphic called that now")).color(AMBER));
                                    }
                                    match row_buttons(ui, gi, len, true) {
                                        Some(RowEdit::Up) => up = Some(gi),
                                        Some(RowEdit::Down) => up = Some(gi + 1),
                                        Some(RowEdit::Remove) => drop = Some(gi),
                                        None => {}
                                    }
                                });
                            }
                            if let Some(i) = up {
                                r.graphics.swap(i, i - 1);
                            }
                            if let Some(i) = drop {
                                r.graphics.remove(i);
                            }
                            let left: Vec<&String> = names.iter().filter(|n| !r.graphics.contains(n)).collect();
                            if left.is_empty() && names.is_empty() {
                                hint(ui, "Add a graphic above first.");
                            } else if !left.is_empty() {
                                egui::ComboBox::from_id_salt(("rotation_add", ri)).selected_text("＋ Add a graphic").width(200.0).show_ui(ui, |ui| {
                                    for n in left {
                                        if ui.selectable_label(false, n).clicked() {
                                            r.graphics.push(n.clone());
                                        }
                                    }
                                });
                            }
                        });
                        ui.end_row();

                        label(ui, "How often");
                        ui.horizontal(|ui| {
                            ui.label("one every");
                            ui.add(egui::DragValue::new(&mut r.every_minutes).range(0.1..=240.0).speed(0.5).max_decimals(1).suffix(" min"));
                            ui.label("for");
                            ui.add(egui::DragValue::new(&mut r.show_seconds).range(1..=3600).speed(1.0).suffix(" s"));
                        });
                        ui.end_row();

                        label(ui, "Runs");
                        egui::ComboBox::from_id_salt(("rotation_start", ri)).selected_text(r.start.label()).width(220.0).show_ui(ui, |ui| {
                            for s in RotationStart::ALL {
                                ui.selectable_value(&mut r.start, s, s.label());
                            }
                        });
                        ui.end_row();
                    });
                    let round = r.graphics.len() as f32 * r.every_minutes.max(r.show_seconds as f32 / 60.0);
                    if r.graphics.len() > 1 {
                        hint(ui, &format!("Each one comes back about every {}.", if round < 1.5 { format!("{:.0} seconds", round * 60.0) } else { format!("{round:.0} minutes") }));
                    }
                });
            }
            if let Some(i) = remove {
                self.draft.rotations.remove(i);
            }
            ui.add_space(6.0);
            if ui.button("＋ Add graphics that take turns").clicked() {
                let taken: Vec<String> = self.draft.rotations.iter().map(|r| r.name.clone()).collect();
                self.draft.rotations.push(Rotation { name: unique(taken, "Sponsors"), ..Default::default() });
            }
            hint(ui, "Each take's folder gets Graphics on air.csv: every graphic that was on screen, how many times and for how long. Handy for sponsors.");
        });
    }
}
