//! Settings → Scenes: the scene bank as a list, to put in order, rename,
//! duplicate, delete and start new ones (blank or from a layout). Frames
//! are moved with Edit layout in the main window.

use super::*;
use crate::scene::Scene;

/// ＋ New scene: blank, or from one of the layouts for these sources. The
/// scene, named so it's the only one with its name.
pub(crate) fn new_scene_menu(ui: &mut egui::Ui, sources: &[String], existing: &[Scene]) -> Option<Scene> {
    let mut out = None;
    ui.menu_button("＋ New scene", |ui| {
        if ui.button("Blank").on_hover_text("No frames yet: add sources with Edit layout").clicked() {
            out = Some(crate::scene::new_scene(None, existing));
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new("From a layout").size(11.5).color(TEXT_DIM));
        for l in crate::scene::presets(sources) {
            let label = if l.layers.len() == 1 { format!("{} full screen", l.name) } else { l.name.clone() };
            if ui.button(label).clicked() {
                out = Some(crate::scene::new_scene(Some(&l), existing));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("A new scene at the end of the bank: blank, or from a layout");
    out
}

/// A scene's frames as boxes with their sources' names.
fn sketch(ui: &mut egui::Ui, scene: &Scene, sources: &[String], size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_rgb(12, 14, 18));
    for l in &scene.layers {
        let r = egui::Rect::from_min_size(rect.min + vec2(l.x * rect.width(), l.y * rect.height()), vec2(l.w * rect.width(), l.h * rect.height())).intersect(rect);
        p.rect_filled(r, 2.0, Color32::from_rgb(44, 52, 66));
        p.rect_stroke(r, 2.0, egui::Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
        let name = sources.get(l.source).cloned().unwrap_or_else(|| format!("Source {}", l.source + 1));
        p.text(r.min + vec2(5.0, 4.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(11.5), Color32::WHITE);
    }
    if scene.layers.is_empty() {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "Blank: add sources with Edit layout", egui::FontId::proportional(12.0), TEXT_DIM);
    }
}

impl SettingsUi {
    pub(super) fn scenes(&mut self, ui: &mut egui::Ui) {
        title(ui, "Scenes", "The buttons along the bottom of the main window, in order: keys 1–9 take the first nine. Put them in order, rename, copy or delete them, or start a new one. Move and resize frames with Edit layout in the main window.");
        if self.draft.scenes.is_empty() {
            self.draft.scenes = self.draft.effective_scenes();
        }
        let n = self.draft.scenes.len();
        if self.sel_scene >= n {
            self.sel_scene = n.saturating_sub(1);
        }
        let sources: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
        let names = |list: &[Scene]| list.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(250.0);
                for i in 0..n {
                    let s = &self.draft.scenes[i];
                    let key = if i < 9 { format!("{}", i + 1) } else { "  ".into() };
                    let b = egui::Button::selectable(self.sel_scene == i, RichText::new(format!("{key}   {}", s.name)).size(14.0)).min_size(vec2(250.0, 30.0));
                    if ui.add(b).on_hover_text(if i < 9 { format!("Key {}", i + 1) } else { "No number key (keys 1–9 take the first nine)".into() }).clicked() {
                        self.sel_scene = i;
                        self.confirm_scene_delete = None;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if let Some(s) = new_scene_menu(ui, &sources, &self.draft.scenes) {
                        self.draft.scenes.push(s);
                        self.sel_scene = self.draft.scenes.len() - 1;
                        self.confirm_scene_delete = None;
                    }
                    if let Some(edit) = list_buttons(ui, n, self.sel_scene, None) {
                        if edit == ListEdit::Remove {
                            // Asked first (below).
                            if n > 1 {
                                self.confirm_scene_delete = Some(self.sel_scene);
                            }
                        } else {
                            let before = names(&self.draft.scenes);
                            apply_list_edit(&mut self.draft.scenes, &mut self.sel_scene, edit, true, |_| None, |s, list| Scene { name: unique(names(list), &format!("{} copy", s.name)), ..s.clone() });
                            let after = names(&self.draft.scenes);
                            crate::control::follow_scenes(&mut self.draft, &before, &after);
                            self.confirm_scene_delete = None;
                        }
                    }
                });
                if let Some(i) = self.confirm_scene_delete.filter(|i| *i < n) {
                    ui.add_space(6.0);
                    let all = names(&self.draft.scenes);
                    let uses = crate::control::scene_uses(&self.draft, &all, i);
                    egui::Frame::new().fill(Color32::from_rgb(52, 30, 34)).corner_radius(6).inner_margin(10).show(ui, |ui| {
                        ui.label(RichText::new(format!("Delete {}?", all[i])).strong().color(Color32::WHITE));
                        if !uses.is_empty() {
                            hint(ui, &format!("Used by: {}. They won't find it any more.", uses.join(", ")));
                        }
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(RED)).clicked() {
                                let before = all.clone();
                                self.draft.scenes.remove(i);
                                self.sel_scene = i.min(self.draft.scenes.len().saturating_sub(1));
                                let after = names(&self.draft.scenes);
                                crate::control::follow_scenes(&mut self.draft, &before, &after);
                                self.confirm_scene_delete = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_scene_delete = None;
                            }
                        });
                    });
                }
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let sel = self.sel_scene;
                let Some(scene) = self.draft.scenes.get(sel).cloned() else {
                    hint(ui, "No scenes yet: start one with ＋ New scene.");
                    return;
                };
                egui::Grid::new("scene").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    label(ui, "Name");
                    ui.vertical(|ui| {
                        let mut text = match &self.scene_rename {
                            Some((i, t)) if *i == sel => t.clone(),
                            _ => scene.name.clone(),
                        };
                        let r = ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut text));
                        if r.changed() {
                            self.scene_rename = Some((sel, text.clone()));
                        }
                        let typed = self.scene_rename.as_ref().filter(|(i, _)| *i == sel).map(|(_, t)| t.trim().to_string());
                        let clash = typed.as_ref().is_some_and(|t| self.draft.scenes.iter().enumerate().any(|(j, s)| j != sel && s.name.eq_ignore_ascii_case(t)));
                        if clash {
                            ui.label(RichText::new("Another scene has that name").color(AMBER).size(12.5));
                        }
                        // Renamed when the box is left, so a half-typed name never clashes.
                        if r.lost_focus() {
                            if let Some(t) = typed.filter(|t| !t.is_empty() && !clash && *t != scene.name) {
                                let before = names(&self.draft.scenes);
                                self.draft.scenes[sel].name = t;
                                let after = names(&self.draft.scenes);
                                crate::control::follow_scenes(&mut self.draft, &before, &after);
                            }
                            self.scene_rename = None;
                        }
                    });
                    ui.end_row();
                    label(ui, "Number key");
                    ui.label(if sel < 9 { format!("{}", sel + 1) } else { "None: only the first nine have one (move it up with ↑)".into() });
                    ui.end_row();
                    label(ui, "Shows");
                    ui.vertical(|ui| {
                        sketch(ui, &scene, &sources, vec2(240.0, 135.0));
                    });
                    ui.end_row();
                    label(ui, "Sound and cues");
                    let sound = scene.audio_on.len() + scene.audio_off.len();
                    ui.label(match (sound, scene.cues.len()) {
                        (0, 0) => "None".to_string(),
                        (a, c) => format!("{a} input{} turned on or off, {c} cue{}", if a == 1 { "" } else { "s" }, if c == 1 { "" } else { "s" }),
                    });
                    ui.end_row();
                });
                ui.add_space(8.0);
                hint(ui, "Moving a scene keeps what's set up for it: macros, MIDI, your own keys and cues that take it by number get its new number, and the rundown and \"if it drops, cut to\" go by its name. Number keys, Stream Deck keys and the remote's buttons go by place: key 3 takes whatever is third.");
            });
        });
    }
}
