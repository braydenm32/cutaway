//! Scenes › Scenes: the scene bank as a list, to put in order, rename,
//! duplicate, delete and start new ones (blank or from a layout). Saves as
//! you go (live.rs). Frames are moved in Show › Edit layout.

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
        ui.label(RichText::new("From a layout").size(TEXT_SMALL).color(TEXT_DIM));
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

/// Rename the scene at `i` to `text` (trimmed), and what picks it by name
/// with it. Ok(false) if that's its name already; Err if it's empty or
/// another scene has it.
pub(super) fn rename_scene(cfg: &mut Config, i: usize, text: &str) -> Result<bool, String> {
    let text = text.trim();
    let Some(now) = cfg.scenes.get(i).map(|s| s.name.clone()) else { return Ok(false) };
    if text.is_empty() {
        return Err("A scene needs a name".into());
    }
    if cfg.scenes.iter().enumerate().any(|(j, s)| j != i && s.name.eq_ignore_ascii_case(text)) {
        return Err("Another scene has that name".into());
    }
    if text == now {
        return Ok(false);
    }
    let names = |list: &[Scene]| list.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    let before = names(&cfg.scenes);
    cfg.scenes[i].name = text.to_string();
    let after = names(&cfg.scenes);
    crate::control::follow_scenes(cfg, &before, &after);
    Ok(true)
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
        p.text(r.min + vec2(5.0, 4.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(TEXT_SMALL), Color32::WHITE);
    }
    if scene.layers.is_empty() {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "Blank: add sources with Edit layout", egui::FontId::proportional(TEXT_BODY), TEXT_DIM);
    }
}

impl SettingsUi {
    /// A new name typed for a scene that would rename it (the box not left yet).
    pub(super) fn scene_rename_pending(&self) -> bool {
        self.scene_rename.as_ref().is_some_and(|(i, t)| {
            let mut c = self.draft.clone();
            rename_scene(&mut c, *i, t) == Ok(true)
        })
    }

    /// Rename the scene whose new name is being typed, if it can be.
    pub(super) fn commit_scene_rename(&mut self) {
        if let Some((i, t)) = self.scene_rename.take() {
            let _ = rename_scene(&mut self.draft, i, &t);
        }
    }

    pub(super) fn scenes(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Scenes", "The scene buttons on Show, in order: keys 1–9 take the first nine. Put them in order, rename, copy or delete them, or start a new one. Changes save as you make them.");
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
                    let b = egui::Button::selectable(self.sel_scene == i, RichText::new(format!("{key}   {}", s.name)).size(TEXT_STATE)).min_size(vec2(250.0, 30.0));
                    let r = ui.add(b);
                    // The one on air: a red dot (red only means on air).
                    let on_air = info.on_air_scene.as_deref() == Some(s.name.as_str());
                    if on_air {
                        ui.painter().circle_filled(r.rect.right_center() - vec2(14.0, 0.0), 4.5, ON_AIR);
                    }
                    let key = if i < 9 { format!("Key {}", i + 1) } else { "No number key (keys 1–9 take the first nine)".into() };
                    if r.on_hover_text(if on_air { format!("{key} · on air now") } else { key }).clicked() {
                        self.commit_scene_rename();
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
                            if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(ERROR)).clicked() {
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
                egui::Grid::new("scene").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
                        if let Some((_, t)) = self.scene_rename.as_ref().filter(|(i, _)| *i == sel) {
                            if let Err(e) = rename_scene(&mut self.draft.clone(), sel, t) {
                                ui.label(RichText::new(e).color(AMBER).size(TEXT_BODY));
                            }
                        }
                        // Renamed when the box is left (or on Apply / OK), so a half-typed name never clashes.
                        if r.lost_focus() {
                            self.commit_scene_rename();
                        }
                    });
                    ui.end_row();
                    label(ui, "Number key");
                    ui.label(if sel < 9 { format!("{}", sel + 1) } else { "None: only the first nine have one (move it up with ↑)".into() });
                    ui.end_row();
                    label(ui, "Shows");
                    ui.vertical(|ui| {
                        sketch(ui, &scene, &sources, vec2(240.0, 135.0));
                        // Its frames are moved on Show › Edit layout: in
                        // preview with Preview on; on air only if it's on air.
                        let on_air = info.on_air_scene.as_deref() == Some(scene.name.as_str());
                        let preview = self.original.transition.preview_mode;
                        let saved = self.original.scenes.iter().any(|s| s.name == scene.name);
                        if !saved {
                            hint(ui, "Edit its layout once its name is saved (leave the name box).");
                        } else if preview || on_air {
                            let tip = if preview { "Puts it in preview and opens Show › Edit layout: the program stays as it is until you Take" } else { "It's on air, so changes show in the recording and stream at once" };
                            if ui.button("Edit its layout ›").on_hover_text(tip).clicked() {
                                self.pending = Some(SettingsAction::EditLayout(scene.name.clone()));
                            }
                            if on_air && !preview {
                                ui.label(RichText::new("● On air: changes show at once").color(ON_AIR).size(TEXT_SMALL));
                            }
                        } else {
                            hint(ui, "It isn't on air, so its frames are moved in preview first:");
                            if ui.button("Turn on Preview").on_hover_text("Pick the next scene in a preview monitor, then Take it (Scenes › Switching). Then Edit its layout here puts it in preview").clicked() {
                                self.draft.transition.preview_mode = true;
                            }
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_checks_the_name_and_takes_references_along() {
        let mut cfg = Config::default();
        cfg.scenes = ["Wide", "Host"].iter().map(|n| Scene { name: n.to_string(), ..Default::default() }).collect();
        cfg.rundown.segments.push(crate::config::Segment { scene: "Host".into(), ..Default::default() });
        assert_eq!(rename_scene(&mut cfg, 1, "  "), Err("A scene needs a name".into()));
        assert!(rename_scene(&mut cfg, 1, "wide").is_err(), "another scene's name");
        assert_eq!(rename_scene(&mut cfg, 1, "Host"), Ok(false));
        assert_eq!(rename_scene(&mut cfg, 1, " Presenter "), Ok(true));
        assert_eq!(cfg.scenes[1].name, "Presenter");
        assert_eq!(cfg.rundown.segments[0].scene, "Presenter");
    }

    #[test]
    fn a_name_still_being_typed_is_applied() {
        let mut cfg = Config::default();
        cfg.scenes = vec![Scene { name: "Wide".into(), ..Default::default() }];
        let mut win = SettingsUi::new(cfg).open_at(Section::Scenes);
        assert!(!win.dirty());
        win.scene_rename = Some((0, "Wide shot".into()));
        assert!(win.dirty(), "leaving now hands it over (take_live)");
        win.commit_scene_rename();
        assert_eq!(win.draft.scenes[0].name, "Wide shot");
        assert!(win.scene_rename.is_none());
        // Saved as you go: nothing waits for an Apply.
        assert!(!win.dirty());
        assert!(matches!(win.live_step(&egui::Context::default()), Some(SettingsAction::Live { .. })));
        // A name that can't be used isn't a change.
        win.scene_rename = Some((0, "".into()));
        assert!(!win.dirty());
    }
}
