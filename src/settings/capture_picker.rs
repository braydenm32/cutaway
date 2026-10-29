//! Choosing what a screen-capture source captures: a screen (drawn as the
//! displays are arranged), a window or an app, each with a picture of what's
//! on it, like a video call's "share your screen".

use super::*;
use crate::platform::capture::{CaptureTarget, ScreenTarget, TargetKind};
use crate::platform::displays::{self, Display};
use eframe::egui::{pos2, Align2, FontId, Rect, Sense, Stroke, StrokeKind};
use std::sync::atomic::{AtomicBool, Ordering};

type Found = Arc<Mutex<Option<Result<Vec<CaptureTarget>, String>>>>;

pub(super) struct CapturePicker {
    found: Found,
    busy: Arc<AtomicBool>,
    tab: Option<TargetKind>,
    /// Pictures of what was found, by device (made once per listing).
    textures: Vec<(String, egui::TextureHandle)>,
    /// The listing the pictures were made from.
    shown: usize,
    listings: Arc<std::sync::atomic::AtomicUsize>,
}

impl CapturePicker {
    pub fn new() -> Self {
        CapturePicker { found: Arc::new(Mutex::new(None)), busy: Arc::new(AtomicBool::new(false)), tab: None, textures: Vec::new(), shown: 0, listings: Arc::new(0.into()) }
    }

    /// Look again (in the background: it takes a second or two).
    fn refresh(&self) {
        if self.busy.swap(true, Ordering::Relaxed) {
            return;
        }
        let (found, busy, listings) = (self.found.clone(), self.busy.clone(), self.listings.clone());
        std::thread::spawn(move || {
            let r = crate::platform::capture_targets().map_err(|e| format!("{e:#}"));
            *found.lock_ok() = Some(r);
            listings.fetch_add(1, Ordering::Relaxed);
            busy.store(false, Ordering::Relaxed);
        });
    }

    fn texture(&self, device: &str) -> Option<egui::TextureId> {
        self.textures.iter().find(|t| t.0 == device).map(|t| t.1.id())
    }

    /// The picker for a source whose device is `device`; the new device
    /// when something else is chosen.
    pub fn show(&mut self, ui: &mut egui::Ui, device: &str) -> Option<String> {
        let current = ScreenTarget::parse(device).unwrap_or(ScreenTarget::Main);
        let tab = *self.tab.get_or_insert(match current {
            ScreenTarget::Main | ScreenTarget::Display(_) => TargetKind::Display,
            ScreenTarget::Window { .. } => TargetKind::Window,
            ScreenTarget::App(_) => TargetKind::App,
        });
        let busy = self.busy.load(Ordering::Relaxed);
        let found = self.found.lock_ok().clone();
        if found.is_none() && !busy {
            self.refresh();
        }
        // New listing: its pictures as textures.
        let listing = self.listings.load(Ordering::Relaxed);
        if listing != self.shown {
            self.shown = listing;
            self.textures.clear();
            if let Some(Ok(list)) = &found {
                for t in list {
                    if let Some(th) = &t.thumb {
                        let img = egui::ColorImage::from_rgba_unmultiplied(th.size, &th.rgba);
                        let tex = ui.ctx().load_texture(format!("capture {}", t.device), img, egui::TextureOptions::LINEAR);
                        self.textures.push((t.device.clone(), tex));
                    }
                }
            }
        }

        let mut chosen = None;
        ui.horizontal(|ui| {
            for (k, label) in [(TargetKind::Display, "Screens"), (TargetKind::Window, "Windows"), (TargetKind::App, "Apps")] {
                if ui.selectable_label(tab == k, label).clicked() {
                    self.tab = Some(k);
                }
            }
            ui.add_space(8.0);
            if ui.add_enabled(!busy, egui::Button::new("⟳ Look again")).on_hover_text("Find newly opened windows and apps").clicked() {
                self.refresh();
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new("Looking for screens and windows…").color(TEXT_DIM));
            }
        });
        ui.add_space(4.0);
        let list = match &found {
            Some(Ok(l)) => l.clone(),
            Some(Err(e)) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(AMBER));
                Vec::new()
            }
            None => Vec::new(),
        };
        match tab {
            TargetKind::Display => {
                let ds = displays::list_recent();
                if ds.is_empty() {
                    hint(ui, "Captures the main screen.");
                } else {
                    let selected = match &current {
                        ScreenTarget::Display(n) => ds.iter().position(|d| d.name == *n),
                        _ => None,
                    }
                    .or_else(|| ds.iter().position(|d| d.main));
                    let thumbs: Vec<Option<egui::TextureId>> = ds.iter().map(|d| self.texture(&display_device(d))).collect();
                    let ours = our_display(ui.ctx(), &ds);
                    if let Some(i) = display_map_with(ui, &ds, selected, ours, 170.0, &thumbs) {
                        chosen = Some(display_device(&ds[i]));
                    }
                    if ds.len() > 1 {
                        hint(ui, "Click the screen to capture. They're drawn as they're arranged in your display settings.");
                    }
                }
            }
            kind => {
                let items: Vec<&CaptureTarget> = list.iter().filter(|t| t.kind == kind).collect();
                if items.is_empty() && found.is_some() && !busy {
                    hint(ui, if kind == TargetKind::Window { "No windows found. Open the window, then press Look again." } else { "No apps with windows found. Open the app, then press Look again." });
                }
                // A remembered window that isn't open right now still shows.
                if let ScreenTarget::Window { app, title } = &current {
                    if kind == TargetKind::Window && !items.iter().any(|t| t.device == device) {
                        ui.label(RichText::new(format!("Capturing \"{title}\" ({}) — not open right now, or its title changed: the app's closest window is used.", crate::platform::capture::short_app(app))).color(AMBER).size(TEXT_BODY));
                    }
                }
                if let ScreenTarget::App(app) = &current {
                    if kind == TargetKind::App && !items.iter().any(|t| t.device == device) {
                        ui.label(RichText::new(format!("Capturing {} — not open right now.", crate::platform::capture::short_app(app))).color(AMBER).size(TEXT_BODY));
                    }
                }
                if let Some(d) = self.cards(ui, &items, device) {
                    chosen = Some(d);
                }
            }
        }
        chosen.filter(|d| d != device)
    }

    /// Windows or apps as cards with their pictures.
    fn cards(&self, ui: &mut egui::Ui, items: &[&CaptureTarget], device: &str) -> Option<String> {
        let (card_w, pic_h) = (184.0, 104.0);
        let mut chosen = None;
        egui::ScrollArea::vertical().id_salt("capture cards").max_height(380.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for t in items {
                    let (rect, resp) = ui.allocate_exact_size(vec2(card_w, pic_h + 40.0), Sense::click());
                    let p = ui.painter_at(rect.expand(3.0));
                    let selected = t.device == device;
                    p.rect_filled(rect, 6.0, if resp.hovered() { RAISED.gamma_multiply(1.4) } else { RAISED });
                    let pic = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(card_w - 12.0, pic_h - 6.0));
                    p.rect_filled(pic, 3.0, Color32::from_gray(10));
                    match (self.texture(&t.device), &t.thumb) {
                        (Some(tex), Some(th)) => {
                            let aspect = th.size[0] as f32 / th.size[1].max(1) as f32;
                            let (w, h) = if pic.width() / pic.height() > aspect { (pic.height() * aspect, pic.height()) } else { (pic.width(), pic.width() / aspect) };
                            p.image(tex, Rect::from_center_size(pic.center(), vec2(w, h)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        _ => {
                            p.text(pic.center(), Align2::CENTER_CENTER, if t.kind == TargetKind::App { "▣" } else { "▢" }, FontId::proportional(TEXT_TITLE), TEXT_DIM);
                        }
                    }
                    let clip = |s: &str, n: usize| if s.chars().count() > n { s.chars().take(n - 1).collect::<String>() + "…" } else { s.to_string() };
                    p.text(pos2(rect.left() + 8.0, rect.bottom() - 26.0), Align2::LEFT_CENTER, clip(&t.name, 26), FontId::proportional(TEXT_BODY), Color32::from_gray(230));
                    p.text(pos2(rect.left() + 8.0, rect.bottom() - 11.0), Align2::LEFT_CENTER, clip(&t.detail, 30), FontId::proportional(TEXT_SMALL), TEXT_DIM);
                    p.rect_stroke(rect, 6.0, Stroke::new(if selected { 2.5 } else { 1.0 }, if selected { ACCENT } else { LINE }), StrokeKind::Inside);
                    if resp.on_hover_text(format!("{}\n{}", t.name, t.detail)).clicked() {
                        chosen = Some(t.device.clone());
                    }
                }
            });
        });
        chosen
    }
}

/// The device that captures a display.
fn display_device(d: &Display) -> String {
    if d.main {
        "screen".into()
    } else {
        ScreenTarget::Display(d.name.clone()).device()
    }
}
