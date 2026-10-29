//! Settings → Vertical version: a 9:16 version of the show for Shorts,
//! Reels and TikTok, with a tall layout per scene and a framing per source.

use super::*;
use crate::config::{VerticalFraming, VerticalLayout, VerticalMode};
use crate::engine::vertical;

impl SettingsUi {
    pub(super) fn vertical(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Vertical version",
            "A 9:16 version of the show for Shorts, Reels and TikTok, made alongside the program: a tall layout for each scene, and a framing for each source.",
        );
        let v = &mut self.draft.vertical;
        egui::Grid::new("vertical").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Make it");
            ui.checkbox(&mut v.enabled, "shown beside the program monitor");
            ui.end_row();
            label(ui, "Size");
            ui.horizontal(|ui| {
                for (w, h) in [(1080u32, 1920u32), (720, 1280)] {
                    if ui.selectable_label((v.width, v.height) == (w, h), format!("{w} × {h}")).clicked() {
                        (v.width, v.height) = (w, h);
                    }
                }
            });
            ui.end_row();
            label(ui, "Record");
            ui.checkbox(&mut v.record, "into each take");
            ui.end_row();
            label(ui, "Stream");
            hint(ui, "choose it for a destination in Streaming");
            ui.end_row();
            label(ui, "Graphics");
            ui.checkbox(&mut v.graphics, "the program's titles, logos and tickers");
            ui.end_row();
        });
        if !v.enabled {
            return;
        }
        let (w, h) = v.size();
        let aspect = w as f32 / h as f32;
        let names: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
        let scenes = self.draft.effective_scenes();

        ui.add_space(14.0);
        subtitle(ui, "Scenes");
        hint(ui, "Automatic: a full-screen source fills it, picture in picture keeps its inset (top right), and anything else is stacked, top to bottom.");
        egui::Grid::new("vertical_scenes").num_columns(3).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
            for scene in &scenes {
                let mut l = self.draft.vertical.layout_of(&scene.name);
                let before = l.clone();
                ui.label(RichText::new(&scene.name).strong());
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("vmode", &scene.name)).selected_text(l.mode.label()).width(130.0).show_ui(ui, |ui| {
                        for m in VerticalMode::ALL {
                            ui.selectable_value(&mut l.mode, m, m.label());
                        }
                    });
                    if l.mode == VerticalMode::One {
                        let current = if l.source.is_empty() { "choose…".to_string() } else { l.source.clone() };
                        egui::ComboBox::from_id_salt(("vsrc", &scene.name)).selected_text(current).width(130.0).show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut l.source, n.clone(), n);
                            }
                        });
                    }
                });
                let one = names.iter().position(|n| n.eq_ignore_ascii_case(&l.source));
                ui.label(RichText::new(describe(&vertical::arrange(scene, l.mode, one, aspect), &names)).color(TEXT_DIM).size(TEXT_BODY));
                ui.end_row();
                if l != before {
                    set_layout(&mut self.draft.vertical.layouts, l);
                }
            }
        });

        ui.add_space(14.0);
        subtitle(ui, "Framing");
        hint(ui, "Which part of each source's picture fills a tall frame: slide it to where the person sits, and zoom in for a closer shot. Apply to see it on the vertical monitor.");
        egui::Grid::new("vertical_framing").num_columns(4).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
            for n in &names {
                let mut f = self.draft.vertical.framing_of(n);
                let before = f.clone();
                ui.label(RichText::new(n).strong());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("left").color(TEXT_DIM).size(TEXT_BODY));
                    ui.add(egui::Slider::new(&mut f.x, 0.0..=1.0).show_value(false));
                    ui.label(RichText::new("right").color(TEXT_DIM).size(TEXT_BODY));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("up").color(TEXT_DIM).size(TEXT_BODY));
                    ui.add(egui::Slider::new(&mut f.y, 0.0..=1.0).show_value(false));
                    ui.label(RichText::new("down").color(TEXT_DIM).size(TEXT_BODY));
                });
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut f.zoom, 1.0..=3.0).step_by(0.05).suffix("×").text("zoom"));
                    if ui.small_button("Middle").on_hover_text("Back to the middle, not zoomed").clicked() {
                        f = VerticalFraming { source: n.clone(), ..Default::default() };
                    }
                });
                ui.end_row();
                if f != before {
                    set_framing(&mut self.draft.vertical.framing, f);
                }
            }
        });
    }
}

/// What a tall layout is, in words.
fn describe(frames: &[(usize, [f32; 4])], names: &[String]) -> String {
    let name = |i: usize| names.get(i).cloned().unwrap_or_else(|| format!("source {}", i + 1));
    match frames {
        [] => "empty".into(),
        [(s, _)] => format!("{} fills it", name(*s)),
        [(main, r), rest @ ..] if r[2] >= 0.999 && r[3] >= 0.999 => {
            format!("{} with {} inset", name(*main), rest.iter().map(|(s, _)| name(*s)).collect::<Vec<_>>().join(", "))
        }
        _ => format!("stacked: {}", frames.iter().map(|(s, _)| name(*s)).collect::<Vec<_>>().join(", ")),
    }
}

/// Keep only layouts that differ from automatic.
fn set_layout(list: &mut Vec<VerticalLayout>, l: VerticalLayout) {
    list.retain(|x| !x.scene.eq_ignore_ascii_case(&l.scene));
    if l.mode != VerticalMode::Auto {
        list.push(l);
    }
}

/// Keep only framings that differ from the middle.
fn set_framing(list: &mut Vec<VerticalFraming>, f: VerticalFraming) {
    list.retain(|x| !x.source.eq_ignore_ascii_case(&f.source));
    if f != (VerticalFraming { source: f.source.clone(), ..Default::default() }) {
        list.push(f);
    }
}
