//! Settings → Graphics: logos, watermarks and name titles.

use super::*;
use crate::config::{Graphic, GraphicKind};
use std::hash::{Hash, Hasher};

fn fingerprint(g: &Graphic, canvas: (u32, u32)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{g:?}{canvas:?}").hash(&mut h);
    h.finish()
}

impl SettingsUi {
    pub(super) fn graphics(&mut self, ui: &mut egui::Ui) {
        title(ui, "Graphics", "Logos, watermarks and name titles above every scene. Each gets its own button next to the scenes.");
        let n = self.draft.graphics.len();
        if self.sel_graphic >= n {
            self.sel_graphic = n.saturating_sub(1);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for i in 0..n {
                    let g = &self.draft.graphics[i];
                    let kind = match g.kind {
                        GraphicKind::Image => "image",
                        GraphicKind::Title => "title",
                    };
                    let b = egui::Button::selectable(self.sel_graphic == i, RichText::new(format!("{}\n{kind}{}", g.name, if i < 9 { format!(" · Alt+{}", i + 1) } else { String::new() })).size(13.0))
                        .min_size(vec2(220.0, 40.0));
                    if ui.add(b).clicked() {
                        self.sel_graphic = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let names: Vec<String> = self.draft.graphics.iter().map(|g| g.name.clone()).collect();
                    if ui.button("＋ Logo").clicked() {
                        let mut g = Graphic { name: unique(names.clone(), "Logo"), visible: true, ..Default::default() };
                        if let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                            g.image = f;
                        }
                        self.draft.graphics.push(g);
                        self.sel_graphic = self.draft.graphics.len() - 1;
                    }
                    if ui.button("＋ Name title").clicked() {
                        self.draft.graphics.push(Graphic::title(&unique(names, "Title"), "Your Name", "Role or topic"));
                        self.sel_graphic = self.draft.graphics.len() - 1;
                    }
                    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
                        self.draft.graphics.remove(self.sel_graphic);
                    }
                    if ui.add_enabled(n > 0 && self.sel_graphic > 0, egui::Button::new("↑")).clicked() {
                        self.draft.graphics.swap(self.sel_graphic, self.sel_graphic - 1);
                        self.sel_graphic -= 1;
                    }
                    if ui.add_enabled(n > 0 && self.sel_graphic + 1 < n, egui::Button::new("↓")).clicked() {
                        self.draft.graphics.swap(self.sel_graphic, self.sel_graphic + 1);
                        self.sel_graphic += 1;
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| self.graphic_detail(ui));
        });
    }

    fn graphic_detail(&mut self, ui: &mut egui::Ui) {
        let canvas = (self.draft.program.width, self.draft.program.height);
        let Some(g) = self.draft.graphics.get_mut(self.sel_graphic) else {
            hint(ui, "No graphics yet. Add a logo (a PNG with a transparent background works best) or a name title.");
            return;
        };
        egui::Grid::new("graphic").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Name");
            ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut g.name));
            ui.end_row();
            match g.kind {
                GraphicKind::Image => {
                    label(ui, "Image");
                    ui.horizontal(|ui| {
                        let mut text = g.image.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut text).desired_width(280.0)).changed() {
                            g.image = PathBuf::from(text);
                        }
                        if ui.button("Choose…").clicked() {
                            if let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                                g.image = f;
                            }
                        }
                    });
                    ui.end_row();
                }
                GraphicKind::Title => {
                    label(ui, "Name line");
                    ui.add(egui::TextEdit::singleline(&mut g.title).desired_width(300.0));
                    ui.end_row();
                    label(ui, "Second line");
                    ui.add(egui::TextEdit::singleline(&mut g.subtitle).desired_width(300.0));
                    ui.end_row();
                    label(ui, "Colours");
                    ui.horizontal(|ui| {
                        for (txt, value, fallback) in [("text", &mut g.text_color, [255u8, 255, 255]), ("box", &mut g.box_color, [16, 24, 32])] {
                            let mut rgb = crate::config::parse_color(value).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or(fallback);
                            if ui.color_edit_button_srgb(&mut rgb).changed() {
                                *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                            }
                            ui.label(txt);
                        }
                        ui.add(egui::Slider::new(&mut g.box_opacity, 0.0..=1.0).text("box opacity"));
                    });
                    ui.end_row();
                    label(ui, "Font");
                    ui.horizontal(|ui| {
                        let shown = if g.font.as_os_str().is_empty() { "built in".to_string() } else { g.font.file_name().unwrap_or_default().to_string_lossy().to_string() };
                        ui.label(shown);
                        if ui.button("Choose…").clicked() {
                            if let Some(f) = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf", "ttc"]).pick_file() {
                                g.font = f;
                            }
                        }
                        if !g.font.as_os_str().is_empty() && ui.small_button("Built in").clicked() {
                            g.font = PathBuf::new();
                        }
                    });
                    ui.end_row();
                }
            }
            label(ui, "Position");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut g.x, 0.0..=1.0).text("across"));
                ui.add(egui::Slider::new(&mut g.y, 0.0..=1.0).text("down"));
            });
            ui.end_row();
            label(ui, "Corner");
            ui.horizontal(|ui| {
                let w = g.width;
                for (txt, x, y) in [("↖", 0.03, 0.04), ("↗", 0.97 - w, 0.04), ("↙", 0.03, 0.80), ("↘", 0.97 - w, 0.80)] {
                    if ui.small_button(txt).clicked() {
                        g.x = x.max(0.0);
                        g.y = y;
                    }
                }
                hint(ui, "quick placement");
            });
            ui.end_row();
            label(ui, "Width");
            ui.add(egui::Slider::new(&mut g.width, 0.02..=1.0).custom_formatter(|v, _| format!("{:.0}% of the picture", v * 100.0)));
            ui.end_row();
            label(ui, "Opacity");
            ui.add(egui::Slider::new(&mut g.opacity, 0.05..=1.0));
            ui.end_row();
            label(ui, "Fade");
            ui.add(egui::DragValue::new(&mut g.fade_ms).range(0..=3000).speed(10).suffix(" ms"));
            ui.end_row();
            label(ui, "Showing at start");
            ui.checkbox(&mut g.visible, "");
            ui.end_row();
        });
        // Preview.
        let fp = fingerprint(g, canvas);
        let key = format!("gfx{}", self.sel_graphic);
        let fresh = self.previews.get(&key).is_none_or(|(f, _)| *f != fp);
        if fresh {
            let img = match g.kind {
                GraphicKind::Title => Some(crate::engine::graphics::render_title(g, canvas)),
                GraphicKind::Image => image::open(&g.image).ok().map(|i| i.to_rgba8()),
            };
            match img {
                Some(img) => {
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    let tex = ui.ctx().load_texture(&key, ci, egui::TextureOptions::LINEAR);
                    self.previews.insert(key.clone(), (fp, tex));
                }
                None => {
                    self.previews.remove(&key);
                }
            }
        }
        ui.add_space(8.0);
        subtitle(ui, "Preview");
        let (rect, _) = ui.allocate_exact_size(vec2(384.0, 384.0 * canvas.1 as f32 / canvas.0.max(1) as f32), egui::Sense::hover());
        ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(40, 46, 56));
        if let Some((_, tex)) = self.previews.get(&key) {
            let size = tex.size_vec2();
            let w = g.width * rect.width();
            let h = w * size.y / size.x.max(1.0);
            let r = egui::Rect::from_min_size(rect.min + vec2(g.x * rect.width(), g.y * rect.height()), vec2(w, h));
            ui.painter().with_clip_rect(rect).image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::from_white_alpha((g.opacity * 255.0) as u8));
        } else if g.kind == GraphicKind::Image {
            ui.label(RichText::new("⚠ Choose a PNG or JPEG image.").color(AMBER));
        }
        hint(ui, "Titles can also be edited live from the main window (click ✎ on the title's button) — handy for changing names between guests.");
    }
}
