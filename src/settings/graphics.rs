//! Settings → Graphics: logos, watermarks, name titles and clocks.

use super::*;
use crate::config::{ClockKind, Graphic, GraphicKind};
use std::hash::{Hash, Hasher};

fn fingerprint(g: &Graphic, canvas: (u32, u32)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{g:?}{canvas:?}").hash(&mut h);
    h.finish()
}

impl SettingsUi {
    pub(super) fn graphics(&mut self, ui: &mut egui::Ui) {
        title(ui, "Graphics", "Logos, watermarks, name titles, clocks and tickers over every scene, each with its own button beside the scenes.");
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
                        GraphicKind::Clock => "clock",
                        GraphicKind::Ticker => "ticker",
                    };
                    let b = egui::Button::selectable(self.sel_graphic == i, RichText::new(format!("{}\n{kind}{}", g.name, if i < 9 { format!(" · Alt+{}", i + 1) } else { String::new() })).size(14.0))
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
                        self.draft.graphics.push(Graphic::title(&unique(names.clone(), "Title"), "Your Name", "Role or topic"));
                        self.sel_graphic = self.draft.graphics.len() - 1;
                    }
                    if ui.button("＋ Clock or countdown").clicked() {
                        self.draft.graphics.push(Graphic::clock(&unique(names.clone(), "Clock"), ClockKind::Time, ""));
                        self.sel_graphic = self.draft.graphics.len() - 1;
                    }
                    if ui.button("＋ Scrolling ticker").clicked() {
                        self.draft.graphics.push(Graphic::ticker(&unique(names, "Ticker"), "Your first headline\nAnd another", ""));
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
        ui.add_space(10.0);
        self.rotations(ui);
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
                GraphicKind::Ticker => {
                    label(ui, "Text");
                    ui.add(egui::TextEdit::multiline(&mut g.title).desired_width(340.0).desired_rows(3).hint_text("one item per line"));
                    ui.end_row();
                    label(ui, "Label");
                    ui.add(egui::TextEdit::singleline(&mut g.subtitle).desired_width(160.0).hint_text("e.g. Coming up (optional)"));
                    ui.end_row();
                    label(ui, "Speed");
                    ui.add(egui::Slider::new(&mut g.speed, 1.0..=20.0).step_by(0.5).suffix(" widths a minute"));
                    ui.end_row();
                    text_style(ui, g);
                }
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
                    text_style(ui, g);
                }
                GraphicKind::Clock => {
                    label(ui, "Shows");
                    egui::ComboBox::from_id_salt("clock_kind").selected_text(g.clock.label()).width(240.0).show_ui(ui, |ui| {
                        for k in ClockKind::ALL {
                            ui.selectable_value(&mut g.clock, k, k.label());
                        }
                    });
                    ui.end_row();
                    match g.clock {
                        ClockKind::Time => {
                            label(ui, "Format");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut g.clock_24h, "24-hour");
                                ui.checkbox(&mut g.show_seconds, "seconds");
                            });
                            ui.end_row();
                        }
                        ClockKind::Until => {
                            label(ui, "Counting down to");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut g.until).desired_width(90.0));
                                if crate::engine::timers::parse_time_of_day(&g.until).is_none() {
                                    ui.label(RichText::new("⚠ a time like 19:30 or 7:30 pm").color(AMBER));
                                } else {
                                    hint(ui, "today, by this computer's clock");
                                }
                            });
                            ui.end_row();
                        }
                        ClockKind::Countdown => {
                            label(ui, "From");
                            ui.horizontal(|ui| {
                                let mut m = g.seconds / 60;
                                let mut sec = g.seconds % 60;
                                let a = ui.add(egui::DragValue::new(&mut m).range(0..=599).suffix(" min"));
                                let b = ui.add(egui::DragValue::new(&mut sec).range(0..=59).suffix(" s"));
                                if a.changed() || b.changed() {
                                    g.seconds = m * 60 + sec;
                                }
                            });
                            ui.end_row();
                        }
                        _ => {}
                    }
                    label(ui, "Label");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut g.title).desired_width(200.0).hint_text("none"));
                        for l in ["Starting in", "Back in", "Time left"] {
                            if ui.small_button(l).clicked() {
                                g.title = l.into();
                            }
                        }
                    });
                    ui.end_row();
                    text_style(ui, g);
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
            label(ui, "Comes on");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("motion").selected_text(g.motion.label()).show_ui(ui, |ui| {
                    for m in crate::config::Motion::ALL {
                        ui.selectable_value(&mut g.motion, m, m.label());
                    }
                });
                ui.label("taking");
                ui.add(egui::DragValue::new(&mut g.fade_ms).range(0..=3000).speed(10).suffix(" ms"));
            });
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
                GraphicKind::Clock => {
                    let (text, over) = crate::engine::timers::clock_text(g, &crate::engine::timers::Timers::default(), chrono::Local::now());
                    Some(crate::engine::graphics::render_clock(g, &text, over, canvas))
                }
                GraphicKind::Ticker => {
                    // The band with the text where it starts.
                    let mut band = crate::engine::graphics::render_ticker_band(g, canvas);
                    let (strip, _) = crate::engine::graphics::render_ticker_strip(g, canvas);
                    let label_w = crate::engine::graphics::ticker_label_width(g, canvas) as u32;
                    let shown = band.width().saturating_sub(label_w).min(strip.width());
                    let text = image::imageops::crop_imm(&strip, 0, 0, shown, strip.height()).to_image();
                    image::imageops::overlay(&mut band, &text, label_w as i64, 0);
                    Some(band)
                }
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
        if g.kind == GraphicKind::Ticker {
            hint(ui, "Change the text live from its button in the main window (✎).");
        } else if g.kind == GraphicKind::Clock && g.clock.is_timer() {
            hint(ui, &format!("Start, pause and reset it from its button in the main window (right-click), a Stream Deck Timer key, or the commands timer:{0}:start, timer:{0}:pause and timer:{0}:reset.", g.name));
        } else if g.kind == GraphicKind::Clock && g.clock == ClockKind::Segment {
            hint(ui, "Counts down the rundown segment that's on, and shows in red how far over it runs.");
        } else {
            hint(ui, "Edit titles live from the main window (✎ on the title's button).");
        }
    }
}

/// Text colours, box and font (titles and clocks).
fn text_style(ui: &mut egui::Ui, g: &mut Graphic) {
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
