//! Sources › Cameras: the source's picture above its settings, from
//! the multiview's own pictures (nothing is opened twice). A virtual PTZ
//! camera shows its wide camera with the shot and each preset as boxes to
//! drag and zoom; a cropped or mirrored source shows what it'll look like.

use super::*;
use crate::config::{PtzPreset, SourceConfig, SourceLook};
use crate::engine::ptz::{shot_rect, Shot};
use eframe::egui::{pos2, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};

/// A running source's picture, as the multiview shows it.
#[derive(Clone)]
pub struct Picture {
    pub uid: u64,
    pub tex: egui::TextureId,
    pub size: [usize; 2],
}

/// How wide the picture is drawn (it's the multiview's: about 480 px).
const WIDE: f32 = 420.0;

/// A preset's part of the wide picture, as fractions (left, top, right,
/// bottom), for an output of `out`'s shape.
pub fn preset_rect(p: &PtzPreset, src: [usize; 2], out: (u32, u32)) -> Rect {
    let (sw, sh) = (src[0].max(2) as f32, src[1].max(2) as f32);
    let r = shot_rect(Shot { x: p.x, y: p.y, zoom: p.zoom.max(1.0) }, (sw as u32, sh as u32), out);
    Rect::from_min_size(pos2(r[0] / sw, r[1] / sh), vec2(r[2] / sw, r[3] / sh))
}

/// Keep a preset's centre where its shot stays inside the picture.
pub fn keep_inside(p: &mut PtzPreset, src: [usize; 2], out: (u32, u32), max_zoom: f32) {
    p.zoom = p.zoom.clamp(1.0, max_zoom.max(1.0));
    let r = preset_rect(p, src, out);
    p.x = p.x.clamp(r.width() / 2.0, 1.0 - r.width() / 2.0);
    p.y = p.y.clamp(r.height() / 2.0, 1.0 - r.height() / 2.0);
}

impl SettingsUi {
    /// The picture at the top of a source's settings.
    pub(super) fn source_picture(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let Some(src) = self.draft.sources.get(self.sel_source).cloned() else { return };
        let picture = |uid: u64| info.pictures.iter().find(|p| p.uid == uid && uid != 0).cloned();
        // A source changed here shows its new picture once applied.
        let applied = self.original.sources.iter().find(|o| o.uid == src.uid);
        let unchanged = applied.is_some_and(|o| o.device.trim() == src.device.trim());
        let parent = src.framed_from().or_else(|| src.virtual_ptz_of()).and_then(|from| self.draft.sources.iter().find(|p| p.name.trim().eq_ignore_ascii_case(from.trim()))).cloned();

        if src.virtual_ptz_of().is_some() {
            let wide = parent.as_ref().and_then(|p| {
                let same = self.original.sources.iter().any(|o| o.uid == p.uid && o.device.trim() == p.device.trim());
                if same { picture(p.uid) } else { None }
            });
            match wide {
                Some(pic) => self.ptz_picture(ui, info, &src, &pic),
                None => placeholder(ui, &format!("The picture from {} shows here once it's running (Apply on the bar below).", parent.as_ref().map(|p| p.name.as_str()).unwrap_or("the wide camera"))),
            }
            return;
        }
        let own = if unchanged { picture(src.uid) } else { None };
        if src.framed_from().is_some() {
            match own {
                Some(pic) => {
                    let (r, _) = draw_picture(ui, &pic, WIDE, Sense::hover());
                    caption(ui, r, "What it shows now: it follows the people by itself");
                }
                None => placeholder(ui, "Not running yet: Apply on the bar below to see it."),
            }
            return;
        }
        match own {
            Some(pic) => look_picture(ui, &pic, &src.look),
            None if !unchanged => placeholder(ui, "Not running yet: Apply on the bar below to see it."),
            None => placeholder(ui, "No picture from it right now."),
        }
    }

    /// A virtual PTZ camera: its wide camera with the shot it's on now and
    /// its presets as boxes. Drag a box to move it, scroll over it to zoom.
    fn ptz_picture(&mut self, ui: &mut egui::Ui, info: &SettingsContext, src: &SourceConfig, pic: &Picture) {
        let out = (src.width.max(2), src.height.max(2));
        let max_zoom = src.ptz.max_zoom;
        let live = info.virtual_ptz.iter().find(|(uid, _)| *uid == src.uid).map(|(_, v)| v.position());
        let here = self.sel_source;
        let chosen = |s: &SettingsUi| s.sel_preset.filter(|&(at, i)| at == here && i < src.ptz.presets.len()).map(|(_, i)| i);
        let sel = chosen(self).or((!src.ptz.presets.is_empty()).then_some(0));
        ui.horizontal_top(|ui| {
            let (r, resp) = draw_picture(ui, pic, WIDE, Sense::click_and_drag());
            let to_screen = |f: Rect| Rect::from_min_max(r.lerp_inside(f.min.to_vec2()), r.lerp_inside(f.max.to_vec2()));
            let painter = ui.painter_at(r);
            // The shot on air now (if it's running).
            if let Some(s) = live {
                let p = PtzPreset { name: String::new(), x: s.x, y: s.y, zoom: s.zoom };
                let b = to_screen(preset_rect(&p, pic.size, out));
                painter.rect_stroke(b, 0.0, Stroke::new(2.0, ON_AIR), StrokeKind::Inside);
                painter.text(b.left_bottom() + vec2(4.0, -3.0), egui::Align2::LEFT_BOTTOM, "now", egui::FontId::proportional(TEXT_BODY), ON_AIR);
            }
            let boxes: Vec<Rect> = src.ptz.presets.iter().map(|p| to_screen(preset_rect(p, pic.size, out))).collect();
            for (i, b) in boxes.iter().enumerate() {
                let on = sel == Some(i);
                let colour = if on { ACCENT } else { Color32::from_white_alpha(200) };
                if on {
                    painter.rect_filled(*b, 0.0, ACCENT.gamma_multiply(0.12));
                }
                dashed_rect(&painter, *b, Stroke::new(if on { 2.0 } else { 1.2 }, colour));
                painter.text(b.left_top() + vec2(4.0, 2.0), egui::Align2::LEFT_TOP, format!("{}", i + 1), egui::FontId::proportional(TEXT_BODY), colour);
            }
            // Pick a box (the smallest under the pointer: a close-up inside a wider one).
            if resp.drag_started() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let hit = boxes.iter().enumerate().filter(|(_, b)| b.contains(p)).min_by(|a, b| a.1.area().total_cmp(&b.1.area())).map(|(i, _)| i);
                    if let Some(i) = hit {
                        self.sel_preset = Some((here, i));
                    }
                }
            }
            let sel = chosen(self).or(sel);
            let presets = &mut self.draft.sources[self.sel_source].ptz.presets;
            if let Some(i) = sel {
                let p = &mut presets[i];
                if resp.dragged() {
                    let d = resp.drag_delta();
                    p.x += d.x / r.width();
                    p.y += d.y / r.height();
                }
                if resp.hovered() {
                    // (Taken, so the page doesn't scroll as well.)
                    let scroll = ui.input_mut(|i| {
                        let d = i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0;
                        i.smooth_scroll_delta = egui::Vec2::ZERO;
                        d
                    });
                    if scroll != 0.0 {
                        p.zoom *= (scroll * 0.004).exp();
                    }
                }
                keep_inside(p, pic.size, out, max_zoom);
            }
            let resp = resp.on_hover_text("Drag a box to move that preset, scroll over the picture to zoom it in or out. The red box is the shot it's on now.");
            if resp.hovered() {
                ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            // What the chosen preset sends out.
            ui.vertical(|ui| {
                let shown = sel.map(|i| (format!("{}. {}", i + 1, presets[i].name), preset_rect(&presets[i], pic.size, out)));
                let (what, uv) = match (shown, live) {
                    (Some((name, uv)), _) => (name, uv),
                    (None, Some(s)) => ("Now".to_string(), preset_rect(&PtzPreset { name: String::new(), x: s.x, y: s.y, zoom: s.zoom }, pic.size, out)),
                    (None, None) => ("The whole picture".to_string(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                };
                let w = 190.0;
                let (r, _) = ui.allocate_exact_size(vec2(w, w * out.1 as f32 / out.0 as f32), Sense::hover());
                ui.painter().rect_filled(r, 2.0, Color32::BLACK);
                ui.painter().image(pic.tex, r, uv, Color32::WHITE);
                ui.label(RichText::new(what).size(TEXT_BODY));
                if let Some(i) = sel {
                    ui.label(RichText::new(format!("{:.1}×", presets[i].zoom)).size(TEXT_BODY).color(TEXT_DIM));
                }
            });
        });
        if src.ptz.presets.is_empty() {
            hint(ui, "Add a preset (Presets, below) and a box appears to frame the shot with.");
        }
        ui.add_space(8.0);
    }
}

/// The source's picture as a cropped or mirrored source shows it: the
/// whole picture with the part that's kept, beside the result.
fn look_picture(ui: &mut egui::Ui, pic: &Picture, look: &SourceLook) {
    let c = look.crop_rect();
    let cropped = c != [0.0, 0.0, 1.0, 1.0];
    if !cropped && !look.mirror {
        draw_picture(ui, pic, WIDE, Sense::hover());
        ui.add_space(8.0);
        return;
    }
    ui.horizontal_top(|ui| {
        let (r, _) = draw_picture(ui, pic, 300.0, Sense::hover());
        let keep = Rect::from_min_max(r.lerp_inside(vec2(c[0], c[1])), r.lerp_inside(vec2(c[2], c[3])));
        let painter = ui.painter_at(r);
        let shade = Color32::from_black_alpha(150);
        for part in [
            Rect::from_min_max(r.min, pos2(r.max.x, keep.min.y)),
            Rect::from_min_max(pos2(r.min.x, keep.max.y), r.max),
            Rect::from_min_max(pos2(r.min.x, keep.min.y), pos2(keep.min.x, keep.max.y)),
            Rect::from_min_max(pos2(keep.max.x, keep.min.y), pos2(r.max.x, keep.max.y)),
        ] {
            painter.rect_filled(part, 0.0, shade);
        }
        painter.rect_stroke(keep, 0.0, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
        ui.vertical(|ui| {
            let mut uv = Rect::from_min_max(pos2(c[0], c[1]), pos2(c[2], c[3]));
            if look.mirror {
                std::mem::swap(&mut uv.min.x, &mut uv.max.x);
            }
            let aspect = (pic.size[0] as f32 * (c[2] - c[0])) / (pic.size[1] as f32 * (c[3] - c[1])).max(1.0);
            let w = 220.0_f32.min(150.0 * aspect);
            let (r, _) = ui.allocate_exact_size(vec2(w, w / aspect.max(0.1)), Sense::hover());
            ui.painter().rect_filled(r, 2.0, Color32::BLACK);
            ui.painter().image(pic.tex, r, uv, Color32::WHITE);
            ui.label(RichText::new("What it'll look like").size(TEXT_BODY));
            ui.label(RichText::new("Colour changes show in the multiview and on air as you make them").size(TEXT_BODY).color(TEXT_DIM));
        });
    });
    ui.add_space(8.0);
}

/// Draw a picture `width` wide (its own shape); returns where.
fn draw_picture(ui: &mut egui::Ui, pic: &Picture, width: f32, sense: Sense) -> (Rect, egui::Response) {
    let w = width.min(ui.available_width()).max(120.0);
    let aspect = pic.size[1].max(1) as f32 / pic.size[0].max(1) as f32;
    let (r, resp) = ui.allocate_exact_size(vec2(w, w * aspect), sense);
    ui.painter().rect_filled(r, 2.0, Color32::BLACK);
    ui.painter().image(pic.tex, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    // The pictures change as they arrive.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(66));
    (r, resp)
}

fn caption(ui: &mut egui::Ui, _r: Rect, text: &str) {
    ui.label(RichText::new(text).size(TEXT_BODY).color(TEXT_DIM));
    ui.add_space(6.0);
}

/// Where a picture will be, until there is one.
fn placeholder(ui: &mut egui::Ui, text: &str) {
    let w = WIDE.min(ui.available_width());
    let (r, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0 * 0.5), Sense::hover());
    ui.painter().rect_filled(r, 4.0, Color32::from_rgb(18, 20, 25));
    dashed_rect(ui.painter(), r.shrink(1.0), Stroke::new(1.0, LINE));
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(TEXT_BODY), TEXT_DIM);
    ui.add_space(8.0);
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts: [Pos2; 5] = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preset's box has the output's shape and stays inside the picture.
    #[test]
    fn preset_boxes() {
        let wide = [1920, 1080];
        let mut p = PtzPreset { name: "Alex".into(), x: 0.25, y: 0.5, zoom: 2.0 };
        let r = preset_rect(&p, wide, (1920, 1080));
        assert!((r.width() - 0.5).abs() < 1e-4 && (r.height() - 0.5).abs() < 1e-4, "{r:?}");
        assert!((r.center().x - 0.25).abs() < 1e-4);
        // A square close-up of a wide picture.
        let sq = preset_rect(&p, wide, (1080, 1080));
        assert!((sq.width() * 1920.0 - sq.height() * 1080.0).abs() < 1.0, "{sq:?}");
        // Dragged off the edge and zoomed past the limit: pulled back.
        p.x = -0.4;
        p.zoom = 9.0;
        keep_inside(&mut p, wide, (1920, 1080), 3.0);
        assert_eq!(p.zoom, 3.0);
        let r = preset_rect(&p, wide, (1920, 1080));
        assert!(r.min.x >= -1e-4 && (r.min.x).abs() < 1e-3, "{r:?}");
    }
}
