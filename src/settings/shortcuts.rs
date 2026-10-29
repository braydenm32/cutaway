//! Settings → Shortcuts: move a built-in shortcut, turn one off, or add
//! your own key for any command.

use super::*;
use crate::config::CustomKey;
use crate::control::shortcuts::{builtin_keys, cheat_sheet, is_modifier, Builtin, KeyCombo, Keymap};

/// The shortcut waiting for a key press.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum KeyCapture {
    Builtin(Builtin),
    Custom(usize),
}

impl SettingsUi {
    pub(super) fn shortcuts(&mut self, ui: &mut egui::Ui) {
        title(ui, "Shortcuts", "Keyboard control while you direct. Move any shortcut to another key, turn one off, or add keys of your own for any command.");
        // Recording a key: the next key pressed (with its modifiers) is it.
        // Esc gives up. The key is used up here so it doesn't also press a
        // button or close Settings.
        if let Some(target) = self.key_capture {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            let got = ui.input_mut(|i| {
                let mut got = None;
                i.events.retain(|e| match e {
                    egui::Event::Key { key, pressed, modifiers, .. } => {
                        if *pressed && got.is_none() && !is_modifier(*key) {
                            got = Some(if *key == egui::Key::Escape { None } else { Some(KeyCombo::new(*key, *modifiers)) });
                        }
                        false
                    }
                    egui::Event::Text(_) => false,
                    _ => true,
                });
                got
            });
            if let Some(combo) = got {
                self.key_capture = None;
                if let Some(c) = combo {
                    match target {
                        KeyCapture::Builtin(b) => {
                            if b.defaults() == vec![c] {
                                self.draft.keys.builtin.remove(b.id());
                            } else {
                                self.draft.keys.builtin.insert(b.id().to_string(), c.code());
                            }
                        }
                        KeyCapture::Custom(i) => {
                            if let Some(k) = self.draft.keys.custom.get_mut(i) {
                                k.key = c.code();
                            }
                        }
                    }
                }
            }
        }
        let map = Keymap::new(&self.draft.keys);
        for p in &map.problems {
            ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
        }

        subtitle(ui, "Built in");
        let mut capture = None;
        egui::Grid::new("builtin_keys").num_columns(3).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
            for b in Builtin::ALL {
                ui.label(b.describe());
                let keys = builtin_keys(&self.draft.keys, b);
                let shown = if keys.is_empty() { "off".to_string() } else { keys.iter().map(|k| k.label()).collect::<Vec<_>>().join(" / ") };
                let waiting = self.key_capture == Some(KeyCapture::Builtin(b));
                let text = if waiting { "press a key…".to_string() } else { shown };
                let rich = if keys.is_empty() && !waiting { RichText::new(text).monospace().color(TEXT_DIM) } else { RichText::new(text).monospace().strong() };
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Button::new(rich).selected(waiting).min_size(vec2(110.0, 0.0))).on_hover_text("Click, then press the key (with ⌘/Ctrl, Shift or Alt if you like). Esc to give up.");
                    if r.clicked() {
                        capture = Some(if waiting { None } else { Some(KeyCapture::Builtin(b)) });
                        r.surrender_focus();
                    }
                    let changed = self.draft.keys.builtin.contains_key(b.id());
                    if changed && ui.small_button("Reset").on_hover_text(format!("Back to {}", b.defaults().iter().map(|k| k.label()).collect::<Vec<_>>().join(" / "))).clicked() {
                        self.draft.keys.builtin.remove(b.id());
                    }
                    if !keys.is_empty() && ui.small_button("Off").on_hover_text("No key for this").clicked() {
                        self.draft.keys.builtin.insert(b.id().to_string(), String::new());
                    }
                });
                ui.end_row();
            }
        });
        hint(ui, "Going live has no key unless you give it one, so a stray key can't start or end a broadcast.");

        subtitle(ui, "Your own");
        let commands = crate::control::suggestions(&self.draft);
        let mut remove = None;
        let mut add = false;
        if self.draft.keys.custom.is_empty() {
            hint(ui, "None yet. Add one to put any command on a key: a scene, a graphic, a sound pad, a macro, a camera move, a lower third…");
        }
        egui::Grid::new("custom_keys").num_columns(3).spacing(TABLE_GAP).show(ui, |ui| {
            for (i, k) in self.draft.keys.custom.iter_mut().enumerate() {
                let waiting = self.key_capture == Some(KeyCapture::Custom(i));
                let shown = if waiting {
                    "press a key…".to_string()
                } else {
                    KeyCombo::parse(&k.key).map(|c| c.label()).unwrap_or_else(|| if k.key.trim().is_empty() { "choose a key".into() } else { k.key.clone() })
                };
                let r = ui.add(egui::Button::new(RichText::new(shown).monospace().strong()).selected(waiting).min_size(vec2(110.0, 0.0))).on_hover_text("Click, then press the key. Esc to give up.");
                if r.clicked() {
                    capture = Some(if waiting { None } else { Some(KeyCapture::Custom(i)) });
                    r.surrender_focus();
                }
                let shown = commands.iter().find(|c| c.0 == k.command).map(|c| c.1.clone()).unwrap_or_else(|| if k.command.is_empty() { "choose…".into() } else { k.command.clone() });
                egui::ComboBox::from_id_salt(("key_action", i)).selected_text(shown).width(280.0).show_ui(ui, |ui| {
                    for (code, text) in &commands {
                        ui.selectable_value(&mut k.command, code.clone(), text);
                    }
                });
                if row_buttons(ui, i, 0, false) == Some(RowEdit::Remove) {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(c) = capture {
            self.key_capture = c;
        }
        if let Some(i) = remove {
            self.draft.keys.custom.remove(i);
            self.key_capture = None;
        }
        ui.horizontal(|ui| {
            if ui.button("＋ Add a shortcut").clicked() {
                add = true;
            }
            if ui.button("🖨 Print a cheat sheet").on_hover_text("Opens a page with every shortcut, ready to print and keep by the keyboard").clicked() {
                let path = std::env::temp_dir().join("Cutaway shortcuts.html");
                let opened = std::fs::write(&path, cheat_sheet(&self.draft)).and_then(|_| crate::util::open_with_system(&path));
                if let Err(e) = opened {
                    self.message = Some((format!("Couldn't open the cheat sheet: {e}"), true));
                }
            }
        });
        if add {
            self.draft.keys.custom.push(CustomKey::default());
            self.key_capture = Some(KeyCapture::Custom(self.draft.keys.custom.len() - 1));
        }
        hint(ui, "A key you pick here wins over a built-in one on the same key. Sound pads and talkback set here work while the key is held.");

        subtitle(ui, "Always");
        let rows: Vec<(&str, &str)> = vec![
            ("1 – 9", "Take scene 1–9 (using the current transition)"),
            ("Shift + 1 – 9", "Cut straight to scene 1–9"),
            (if cfg!(target_os = "macos") { "Option + 1 – 9" } else { "Alt + 1 – 9" }, "Show / hide graphic 1–9"),
            ("F1 – F12", "Sound pads 1–12 (hold for \"while held\" pads)"),
            ("Click a source", "Put that source full screen (Shift+click cuts)"),
            ("Right-click a media tile", "Pick from the media player's playlist"),
            ("Esc", "Close Settings"),
            ("F / Esc", "On the second-screen window: full screen / leave full screen"),
        ];
        egui::Grid::new("fixed_keys").num_columns(2).spacing(FORM_GAP).striped(true).show(ui, |ui| {
            for (k, what) in rows {
                ui.label(RichText::new(k).monospace().strong());
                ui.label(what);
                ui.end_row();
            }
        });
    }
}
