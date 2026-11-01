//! Show › Switch: the column of panels on the right of the desk, with the
//! audio strip pinned under them.
//!
//! The panels share what the audio strip leaves: the strip always keeps
//! room for its meters, faders and ON buttons, and the panels scroll above
//! it. When a panel is out of sight below, a "more below" line says which.

use super::theme::*;
use super::App;
use crate::config::Graphic;
use crate::util::LockExt;
use crate::settings::Section;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke};

/// The meters' height when the column is short (they grow when there's room).
pub(super) const METER_MIN: f32 = 100.0;
/// What the meters get before the panels above take more.
const METER_COMFY: f32 = 140.0;

/// The column's own state (this run only).
#[derive(Default)]
pub(super) struct ColumnState {
    /// Arrange panels is open.
    pub arrange: bool,
    /// Bumped by Reset width, so the column forgets the width it was dragged to.
    pub resets: u32,
    /// The audio strip's height besides its meters (measured last frame).
    pub audio_rest: Option<f32>,
    /// A scroll asked for by "more below".
    pub scroll_to: Option<f32>,
}

/// A panel's name in Arrange panels and the ⋯ menu, its Settings page, and
/// what makes it appear.
fn panel_info(panel: &str) -> (&'static str, Section, &'static str) {
    match panel {
        "graphics" => ("Graphics", Section::Graphics, "always"),
        "stream" => ("Stream", Section::Streaming, "when you have somewhere to stream to"),
        "sound" => ("Sound pads", Section::Pads, "when you have pads, Syrinscape or talkback"),
        "comments" => ("Viewer comments", Section::Comments, "when live chat is set up"),
        "ptz" => ("PTZ", Section::Sources, "when a camera has PTZ or close-ups"),
        "rundown" => ("Rundown", Section::Rundown, "when the rundown has segments"),
        "guests" => ("Guests", Section::Guests, "while guests are set up"),
        "uploads" => ("Uploads", Section::Upload, "while uploads are waiting"),
        "macros" => ("Macros", Section::Macros, "when you have macros"),
        "prompter" => ("Teleprompter", Section::Prompter, "when there's a script"),
        _ => ("Panel", Section::General, ""),
    }
}

impl App {
    /// Whether a panel has anything to show in this show (mirrors each
    /// panel's own check).
    pub(super) fn panel_present(&self, panel: &str) -> bool {
        let engine = self.engine.as_ref();
        match panel {
            "graphics" => true,
            "stream" => self.cfg.streaming.destinations.iter().any(|d| d.enabled) || engine.is_some_and(|e| e.is_streaming()),
            "sound" => {
                engine.is_some_and(|e| !e.pads.state().is_empty() || e.mix.talkback_input().is_some()) || (self.cfg.syrinscape.enabled && !self.cfg.syrinscape.buttons.is_empty())
            }
            "comments" => self.chat.is_some(),
            "ptz" => engine.is_some() && self.cfg.sources.iter().any(|s| s.has_ptz()),
            "rundown" => !self.cfg.rundown.segments.is_empty(),
            "guests" => self.cfg.guests.enabled && !crate::engine::guests::hub().guests().is_empty(),
            "uploads" => !self.uploads.jobs.lock_ok().is_empty() || self.uploads.offer.is_some() || self.uploads.unfinished() > 0,
            "macros" => !self.cfg.macros.is_empty(),
            "prompter" => !self.prompter.script.trim().is_empty(),
            _ => false,
        }
    }

    /// The column: a slim bar with ⋯, the panels, then the audio strip.
    pub(super) fn right_column(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(280.0);
        self.column_bar(ui);
        // The audio strip keeps room for its meters, faders and ON buttons;
        // the panels get the rest, and scroll.
        let audio_rest = self.column.audio_rest.unwrap_or(150.0);
        let max_top = (ui.available_height() - audio_rest - METER_COMFY).max(90.0);
        let mut area = egui::ScrollArea::vertical().id_salt("right_top").max_height(max_top).auto_shrink([false, true]);
        if let Some(y) = self.column.scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }
        let out = area.show(ui, |ui| {
            // Where each panel starts, to say which are out of sight.
            let mut starts: Vec<(&'static str, f32)> = Vec::new();
            let top = ui.min_rect().top();
            for panel in self.cfg.ui.panel_order() {
                let before = ui.cursor().top();
                match panel {
                    "stream" => self.stream_panel(ui),
                    "uploads" => self.uploads_panel(ui),
                    "rundown" => self.rundown_panel(ui),
                    "guests" => self.guests_panel(ui),
                    "ptz" => self.ptz_panel(ui),
                    "comments" => self.comments_panel(ui),
                    "prompter" => self.prompter_panel(ui),
                    "macros" => self.macros_panel(ui),
                    "graphics" => self.graphics_panel(ui),
                    _ => self.sound_panel(ui),
                }
                if ui.cursor().top() > before + 1.0 {
                    starts.push((panel, before - top));
                }
            }
            starts
        });
        self.more_below(ui, &out);
        let before = ui.cursor().top();
        let meter_h = self.audio_panel(ui);
        let used = ui.cursor().top() - before;
        if let Some(m) = meter_h {
            self.column.audio_rest = Some((used - m).max(60.0));
        }
        self.arrange_window(ui.ctx());
    }

    /// A line over the bottom of the panels while some are out of sight
    /// below: which ones, and a click scrolls to them.
    fn more_below(&mut self, ui: &mut egui::Ui, out: &egui::scroll_area::ScrollAreaOutput<Vec<(&'static str, f32)>>) {
        let view = out.inner_rect;
        let seen_to = out.state.clamped_offset().y + view.height();
        if out.content_size.y - seen_to < 6.0 {
            return;
        }
        // Panels that start below what's in view (or the one cut off).
        let below: Vec<&str> = out.inner.iter().filter(|(_, y)| *y > seen_to - 24.0).map(|(p, _)| panel_info(p).0).collect();
        let text = match below.as_slice() {
            // The last panel in view goes on below.
            [] => match out.inner.iter().rev().find(|(_, y)| *y < seen_to) {
                Some((p, _)) => format!("More of {} below ▾", panel_info(p).0),
                None => "More below ▾".to_string(),
            },
            [one] => format!("{one} below ▾"),
            [one, two] => format!("{one} and {two} below ▾"),
            [one, rest @ ..] => format!("{one} and {} more below ▾", rest.len()),
        };
        let bar = Rect::from_min_max(pos2(view.left(), view.bottom() - 26.0), view.right_bottom());
        let painter = ui.painter_at(view);
        // A fade so it reads as "this goes on", then the line itself.
        for k in 0..8 {
            let r = Rect::from_min_max(pos2(bar.left(), bar.top() - 16.0 + k as f32 * 2.0), pos2(bar.right(), bar.top() - 14.0 + k as f32 * 2.0));
            painter.rect_filled(r, 0.0, PANEL.gamma_multiply(k as f32 / 8.0));
        }
        painter.rect_filled(bar, 0.0, PANEL);
        let r = ui.interact(bar, ui.scope_id().with("more_below"), Sense::click());
        let color = if r.hovered() { Color32::WHITE } else { ACCENT };
        painter.text(bar.center(), Align2::CENTER_CENTER, text, FontId::proportional(TEXT_SMALL), color);
        if r.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if r.on_hover_text("Scroll the panels down (or use the mouse wheel)").clicked() {
            let next = out.inner.iter().map(|(_, y)| *y).find(|y| *y > out.state.clamped_offset().y + 8.0 && *y >= seen_to - 24.0);
            let max = (out.content_size.y - view.height()).max(0.0);
            self.column.scroll_to = Some(next.unwrap_or(seen_to).min(max));
        }
    }

    /// The bar over the panels: ⋯ with Arrange panels and each panel's
    /// Settings page (so "Set up" is one place, not a button per panel).
    fn column_bar(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<Section> = None;
        ui.horizontal(|ui| {
            ui.set_height(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().button_padding = vec2(6.0, 0.0);
                let r = ui.menu_button(RichText::new("⋯").size(TEXT_BODY).color(TEXT_DIM), |ui| {
                    ui.set_min_width(200.0);
                    if ui.button("Arrange panels…").clicked() {
                        self.column.arrange = true;
                        ui.close();
                    }
                    ui.separator();
                    ui.label(RichText::new("Set up").size(TEXT_SMALL).color(TEXT_DIM));
                    for p in self.cfg.ui.panel_order() {
                        let (label, section, _) = panel_info(p);
                        if ui.button(format!("{label} ›")).clicked() {
                            open = Some(section);
                            ui.close();
                        }
                    }
                    if ui.button("Audio ›").clicked() {
                        open = Some(Section::Audio);
                        ui.close();
                    }
                });
                r.response.on_hover_text("Arrange the panels, or set one up");
            });
        });
        if let Some(s) = open {
            self.open_page(s, None);
        }
    }

    /// Arrange panels: the order (↑ ↓), which are folded, and Reset.
    fn arrange_window(&mut self, ctx: &egui::Context) {
        if !self.column.arrange {
            return;
        }
        let mut open = true;
        let mut moved: Option<(&'static str, Option<&'static str>)> = None;
        let mut fold: Option<(&'static str, bool)> = None;
        let mut reset = false;
        egui::Window::new("Arrange panels").id(egui::Id::unique("arrange_panels")).open(&mut open).collapsible(false).resizable(false).anchor(Align2::RIGHT_TOP, vec2(-16.0, 70.0)).show(ctx, |ui| {
            ui.set_width(330.0);
            ui.label(RichText::new("Top to bottom, as in the column. Audio always stays at the bottom.").size(TEXT_SMALL).color(TEXT_DIM));
            ui.add_space(4.0);
            let order = self.cfg.ui.panel_order();
            egui::Grid::new("arrange_grid").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                for (k, p) in order.iter().enumerate() {
                    let (label, _, when) = panel_info(p);
                    let present = self.panel_present(p);
                    let name = ui.label(RichText::new(label).size(TEXT_BODY).color(if present { Color32::WHITE } else { TEXT_DIM }));
                    if !present {
                        name.on_hover_text(format!("Not in this show yet: it appears {when}"));
                    }
                    let mut shown = !self.cfg.ui.is_folded(p);
                    if ui.checkbox(&mut shown, RichText::new("Open").size(TEXT_SMALL)).on_hover_text("Unticked: folded down to its heading").changed() {
                        fold = Some((p, !shown));
                    }
                    if ui.add_enabled(k > 0, egui::Button::new("↑").small()).on_hover_text("Move up").clicked() {
                        moved = Some((p, Some(order[k - 1])));
                    }
                    if ui.add_enabled(k + 1 < order.len(), egui::Button::new("↓").small()).on_hover_text("Move down").clicked() {
                        moved = Some((p, order.get(k + 2).copied()));
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("You can also drag a heading by its ⠿ dots, and click it to fold.").size(TEXT_SMALL).color(TEXT_DIM));
            ui.add_space(4.0);
            let default = self.cfg.ui.panel_order.is_empty() && self.cfg.ui.folded.is_empty() && self.cfg.ui.column_width == 0.0;
            if ui.add_enabled(!default, egui::Button::new("Reset order, folds and width")).clicked() {
                reset = true;
            }
        });
        if let Some((p, before)) = moved {
            self.cfg.ui.move_panel(p, before);
            self.cfg_dirty = true;
        }
        if let Some((p, f)) = fold {
            self.cfg.ui.set_folded(p, f);
            self.cfg_dirty = true;
        }
        if reset {
            self.cfg.ui.panel_order.clear();
            self.cfg.ui.folded.clear();
            self.cfg.ui.column_width = 0.0;
            self.column.resets += 1;
            self.cfg_dirty = true;
            self.toast("Panels back in their usual order and width".into(), false);
        }
        if !open {
            self.column.arrange = false;
        }
    }

    /// The Graphics panel with nothing in it yet.
    pub(super) fn graphics_empty(&mut self, ui: &mut egui::Ui) {
        let mut make = false;
        let mut more = false;
        egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("No titles yet. A name title puts each person's name on screen when they're on air.").size(TEXT_BODY).color(TEXT_DIM));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("＋ Make a name title").strong().size(TEXT_BODY).color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(0.0, 28.0));
                if ui.add(b).on_hover_text("Adds a title that names whoever is on air (Undo takes it away)").clicked() {
                    make = true;
                }
                if ui.add(egui::Button::new(RichText::new("Logos, clocks and more ›").size(TEXT_SMALL).color(ACCENT)).frame(false)).clicked() {
                    more = true;
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        if make {
            self.make_name_title(ui.ctx());
        }
        if more {
            self.open_page(Section::Graphics, None);
        }
    }

    /// One click from the empty Graphics panel: a title that names whoever
    /// is on air, from the names set on each source. Its name, once made.
    pub(super) fn make_name_title(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut name = "Names".to_string();
        let mut n = 2;
        while self.cfg.graphics.iter().any(|g| g.name.eq_ignore_ascii_case(&name)) {
            name = format!("Names {n}");
            n += 1;
        }
        let mut cfg = self.cfg.clone();
        cfg.scenes = self.scenes.clone();
        cfg.graphics.push(Graphic::names(&name));
        self.remember(super::undo::Kind::Settings, "Make a name title");
        match self.apply_settings(cfg, ctx) {
            Ok(()) => {
                let named = self.cfg.sources.iter().filter(|s| !s.person.trim().is_empty()).count();
                self.toast(
                    if named == 0 {
                        format!("Made \"{name}\". Now say who's in each picture: Sources › People & names")
                    } else {
                        format!("Made \"{name}\": it names whoever is on air. Click it to show it")
                    },
                    false,
                );
                Some(name)
            }
            Err(e) => {
                self.toast(format!("Couldn't make the title: {e}"), true);
                None
            }
        }
    }

    /// The Segment timer graphic follows the rundown as it's edited (numbers
    /// audit #9): same start, the plan it has now. Every frame, whichever
    /// area is open (the rundown is edited on More › Show tools).
    pub(super) fn sync_segment_clock(&mut self) {
        let Some(e) = &self.engine else { return };
        let Some(i) = self.rundown.status(&self.cfg.rundown.segments).index else { return };
        let Some(seg) = self.cfg.rundown.segments.get(i) else { return };
        if e.graphics.timers.segment().is_some_and(|s| s.name != seg.name || s.planned.as_secs() != seg.seconds as u64) {
            e.graphics.timers.replan_segment(&seg.name, seg.seconds);
        }
    }

    /// The AUDIO heading: fixed (it can't fold or move), so it's drawn
    /// unlike the panels' headings: no dots, no arrow, a line across.
    pub(super) fn audio_heading(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let p = ui.painter();
        p.hline(rect.x_range(), rect.top(), Stroke::new(1.0, LINE));
        p.text(pos2(rect.left(), rect.center().y + 2.0), Align2::LEFT_CENTER, "AUDIO", FontId::proportional(TEXT_SMALL), Color32::from_gray(225));
        p.text(pos2(rect.left() + 0.5, rect.center().y + 2.0), Align2::LEFT_CENTER, "AUDIO", FontId::proportional(TEXT_SMALL), Color32::from_gray(225));
    }
}

/// Each panel named in `PANELS` has a name for Arrange panels.
#[cfg(test)]
#[test]
fn every_panel_has_a_name() {
    for p in crate::config::PANELS {
        assert_ne!(panel_info(p).0, "Panel", "{p}");
    }
}
