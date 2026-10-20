//! Settings → Macros: buttons that run a list of commands.

use super::rundown::commands_editor;
use super::sound::color_button;
use super::*;
use crate::config::Macro;

impl SettingsUi {
    pub(super) fn macros(&mut self, ui: &mut egui::Ui) {
        title(ui, "Macros", "One press runs a list of commands: take a scene, wait, show a title, play a pad, start a countdown… Each is a button in the main window, the Stream Deck and the remote.");
        let n = self.draft.macros.len();
        if self.sel_macro >= n {
            self.sel_macro = n.saturating_sub(1);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for i in 0..n {
                    let m = &self.draft.macros[i];
                    let b = egui::Button::selectable(self.sel_macro == i, RichText::new(format!("{}. {}", i + 1, m.name)).size(13.0)).min_size(vec2(220.0, 30.0));
                    if ui.add(b).clicked() {
                        self.sel_macro = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("＋ Macro").clicked() {
                        let name = unique(self.draft.macros.iter().map(|m| m.name.clone()).collect(), "Macro");
                        self.draft.macros.push(Macro { name, ..Default::default() });
                        self.sel_macro = self.draft.macros.len() - 1;
                    }
                    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
                        self.draft.macros.remove(self.sel_macro);
                    }
                    if ui.add_enabled(n > 0 && self.sel_macro > 0, egui::Button::new("↑")).clicked() {
                        self.draft.macros.swap(self.sel_macro, self.sel_macro - 1);
                        self.sel_macro -= 1;
                    }
                    if ui.add_enabled(n > 0 && self.sel_macro + 1 < n, egui::Button::new("↓")).clicked() {
                        self.draft.macros.swap(self.sel_macro, self.sel_macro + 1);
                        self.sel_macro += 1;
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let suggestions = crate::control::suggestions(&self.draft);
                let sel = self.sel_macro;
                let Some(m) = self.draft.macros.get_mut(sel) else {
                    hint(ui, "No macros yet. For example: \"Break\" = take the Break scene, wait 1 s, start the music pad, start the \"Back in\" countdown.");
                    return;
                };
                egui::Grid::new("macro").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    label(ui, "Name");
                    ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut m.name));
                    ui.end_row();
                    label(ui, "Colour");
                    color_button(ui, &mut m.color, [106, 79, 201]);
                    ui.end_row();
                    label(ui, "Steps");
                    ui.vertical(|ui| commands_editor(ui, &format!("macro{sel}"), &mut m.commands, &suggestions, true));
                    ui.end_row();
                });

            });
        });
    }
}
