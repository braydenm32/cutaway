//! Sources › People & names: who's in each picture, for the name titles
//! that follow whoever is on air, and each camera's name. The one place
//! names are edited (forensics settings #4); the first-show guide shows the
//! same rows, which behave the same way. Like every area page, changes save
//! as you go, and ↶ Undo (⌘Z) takes them back (see settings/live.rs): who's
//! in a picture as it's typed (nothing reopens), a camera's new name when
//! you press Enter or click elsewhere (it restarts that camera).

use super::theme::*;
use super::{undo, App};
use eframe::egui::{self, vec2, Color32, RichText};

/// What a row asked for.
enum RowEdit {
    Rename(usize, String),
    Who(usize, String, String),
}

impl App {
    /// The sources people can be in (not media players).
    pub(super) fn people_sources(&self) -> Vec<usize> {
        self.cfg.sources.iter().enumerate().filter(|(_, s)| !s.device.trim().eq_ignore_ascii_case("media")).map(|(i, _)| i).collect()
    }

    /// Is there a title that names whoever is on air?
    pub(super) fn has_name_title(&self) -> bool {
        self.cfg.graphics.iter().any(|g| g.follow)
    }

    /// One row per source: its name, who's in it and their role. Saved when
    /// a box is left (Enter or a click elsewhere), straight away, as one
    /// Undo step. `max`: rows shown (the guide shows a few).
    pub(super) fn people_rows(&mut self, ui: &mut egui::Ui, max: usize, wide: bool) {
        let rows = self.people_sources();
        let busy = self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming());
        let (name_w, who_w, role_w) = if wide { (160.0, 200.0, 170.0) } else { (110.0, 130.0, 110.0) };
        let mut edit = None;
        for &i in rows.iter().take(max) {
            let s = &self.cfg.sources[i];
            let guest = s.guest_id().is_some();
            // A new name is kept beside the row until the box is left, so
            // the camera doesn't reopen at every letter.
            let key = |what: &str| ui.scope_id().with(("people", what, s.uid, i));
            let (kn, kp, kr) = (key("name"), key("person"), key("role"));
            let mut name: String = ui.data_mut(|d| d.get_temp(kn)).unwrap_or_else(|| s.name.clone());
            let mut person: String = ui.data_mut(|d| d.get_temp(kp)).unwrap_or_else(|| s.person.clone());
            let mut role: String = ui.data_mut(|d| d.get_temp(kr)).unwrap_or_else(|| s.role.clone());
            ui.horizontal(|ui| {
                let r = ui.add_enabled(!busy, egui::TextEdit::singleline(&mut name).desired_width(name_w).background_color(RAISED).hint_text("camera name"));
                let r = r.on_disabled_hover_text("Stop recording and streaming to rename a camera: a new name restarts it for a moment");
                let r = if busy { r } else { r.on_hover_text("The source's name, in scenes and the multiview. A new name restarts the camera for a moment") };
                // Saved once the box is left (also by going elsewhere mid-word).
                if !r.has_focus() && name.trim() != s.name.trim() {
                    edit = Some(RowEdit::Rename(i, name.trim().to_string()));
                    name = s.name.clone();
                }
                let a = ui.add(egui::TextEdit::singleline(&mut person).desired_width(who_w).background_color(RAISED).hint_text(if guest { "the name they type" } else { "who's in it" }));
                let b = ui.add(egui::TextEdit::singleline(&mut role).desired_width(role_w).background_color(RAISED).hint_text("role or handle"));
                // Who's in it changes live (nothing reopens), so it's saved as typed.
                if (a.changed() || b.changed()) && (person != s.person || role != s.role) {
                    edit = Some(RowEdit::Who(i, person.clone(), role.clone()));
                }
            });
            for (k, v, now) in [(kn, &name, &s.name), (kp, &person, &s.person), (kr, &role, &s.role)] {
                if v == now {
                    ui.data_mut(|d| d.remove::<String>(k));
                } else {
                    ui.data_mut(|d| d.insert_temp(k, v.clone()));
                }
            }
        }
        if busy && wide {
            ui.label(RichText::new("Camera names can be changed once recording and streaming stop (a new name restarts the camera).").size(TEXT_SMALL).color(TEXT_DIM));
        }
        if rows.len() > max {
            ui.label(RichText::new(format!("and {} more in Sources › People & names", rows.len() - max)).color(TEXT_DIM));
        }
        match edit {
            Some(RowEdit::Rename(i, new)) => self.rename_camera(i, &new, ui.ctx()),
            Some(RowEdit::Who(i, person, role)) => self.set_who(i, person, role),
            None => {}
        }
    }

    /// Who's in a picture: changes live (nothing reopens), as one Undo step.
    fn set_who(&mut self, i: usize, person: String, role: String) {
        let Some(name) = self.cfg.sources.get(i).map(|s| s.name.clone()) else { return };
        self.remember(undo::Kind::Settings, format!("Who's in {name}"));
        let s = &mut self.cfg.sources[i];
        (s.person, s.role) = (person, role);
        self.cfg_dirty = true;
        if let Some(e) = &self.engine {
            let _ = e.update_config(&self.cfg);
        }
    }

    /// A camera's new name, applied now (close-ups cut from it and sound
    /// taken from it follow), as one Undo step.
    fn rename_camera(&mut self, i: usize, new: &str, ctx: &egui::Context) {
        let Some((uid, old)) = self.cfg.sources.get(i).map(|s| (s.uid, s.name.clone())) else { return };
        let mut cfg = self.live_settings();
        if let Err(e) = cfg.rename_source(i, new) {
            self.toast(e, true);
            return;
        }
        let before = self.snapshot(undo::Kind::Settings, format!("Rename {old}"));
        self.quiet_save = true;
        let result = self.apply_settings(cfg, ctx);
        self.quiet_save = false;
        match result {
            Ok(()) => {
                self.history.record(before);
                if let Some(win) = self.settings.as_mut() {
                    win.follow_rename(uid, &old, new.trim());
                }
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// Sources › People & names.
    pub(super) fn people_page(&mut self, ui: &mut egui::Ui) {
        let rows = self.people_sources();
        let named = rows.iter().filter(|&&i| !self.cfg.sources[i].person.trim().is_empty()).count();
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 44.0).show(ui, |ui| {
            ui.set_max_width(760.0);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Who's in each picture").size(TEXT_HEADING).color(Color32::WHITE));
                ui.label(RichText::new(format!("{named} of {} named", rows.len())).color(if named == rows.len() { GREEN } else { TEXT_DIM }));
            });
            ui.label(RichText::new("A name title shows the person in the picture that fills most of the program, changing at each cut.").color(TEXT_DIM));
            ui.add_space(8.0);
            // A show with no name title yet: make one here (guide #1).
            if !self.has_name_title() {
                let mut make = false;
                egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("No name title yet: these names show on screen once there is one.").color(TEXT_DIM));
                        let b = egui::Button::new(RichText::new("＋ Make a name title").strong().color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(0.0, 28.0));
                        if ui.add(b).on_hover_text("Adds a title that names whoever is on air (Undo takes it away)").clicked() {
                            make = true;
                        }
                    });
                });
                if make {
                    self.make_name_title(ui.ctx());
                }
                ui.add_space(8.0);
            }
            ui.horizontal(|ui| {
                for (t, w) in [("CAMERA", 160.0), ("WHO'S IN IT", 200.0), ("ROLE OR HANDLE", 170.0)] {
                    ui.allocate_ui_with_layout(vec2(w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w);
                        ui.label(RichText::new(t).size(TEXT_SMALL).color(TEXT_DIM));
                    });
                }
            });
            self.people_rows(ui, usize::MAX, true);
            ui.add_space(12.0);
            // Two people sharing one camera: a close-up each (guide #5).
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Two or three people in one camera? Give each a close-up of their own, then name them here.").color(TEXT_DIM));
                if ui.add(egui::Button::new(RichText::new("Make close-ups ›").color(ACCENT)).frame(false)).clicked() {
                    self.go("sources/closeups/split");
                }
            });
        });
        self.page_footer(ui);
    }

    /// The bar under a page the app draws itself: Undo and Redo, and that
    /// changes save as you go (as settings/live.rs draws it on the others).
    pub(super) fn page_footer(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let (undo, redo) = (self.history.can_undo().map(String::from), self.history.can_redo().map(String::from));
        let typing = ui.ctx().egui_wants_keyboard_input();
        // The key as it's set now (numbers audit #22).
        let key = self.key_label_or(crate::control::shortcuts::Builtin::Undo, "no key");
        ui.horizontal(|ui| {
            let u = ui.add_enabled(undo.is_some(), egui::Button::new("↶ Undo")).on_hover_text(match &undo {
                Some(l) => format!("Undo: {l} ({key} when you're not typing)"),
                None => "Nothing to undo".into(),
            });
            if u.clicked() {
                self.undo(ui.ctx());
            }
            let r = ui.add_enabled(redo.is_some(), egui::Button::new("↷ Redo")).on_hover_text(match &redo {
                Some(l) => format!("Redo: {l}"),
                None => "Nothing to redo".into(),
            });
            if r.clicked() {
                self.redo(ui.ctx());
            }
            ui.add_space(8.0);
            let text = if typing { "Saves when you press Enter or click elsewhere" } else { "✓ Changes save as you make them" };
            ui.label(RichText::new(text).color(TEXT_DIM));
        });
    }
}
