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
                    let b = egui::Button::selectable(self.sel_macro == i, RichText::new(format!("{}. {}", i + 1, m.name)).size(TEXT_STATE)).min_size(vec2(220.0, 30.0));
                    if ui.add(b).clicked() {
                        self.sel_macro = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if let Some(edit) = list_buttons(ui, n, self.sel_macro, Some("＋ Macro")) {
                        apply_list_edit(
                            &mut self.draft.macros,
                            &mut self.sel_macro,
                            edit,
                            false,
                            |list| Some(Macro { name: unique(list.iter().map(|m| m.name.clone()).collect(), "Macro"), ..Default::default() }),
                            |m, list| Macro { name: unique(list.iter().map(|m| m.name.clone()).collect(), &format!("{} copy", m.name)), ..m.clone() },
                        );
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
                egui::Grid::new("macro").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
