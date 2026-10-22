//! Waiting, shown nicely: a take being rescued after a crash, a camera
//! coming up (or back), and the little moving marks the rest share.

use super::theme::*;
use super::App;
use crate::engine::FrameSlot;
use crate::recovery::RescueState;
use crate::util::LockExt;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Stroke};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Three dots taking turns, like a tally light thinking about it.
pub(super) fn dots(painter: &egui::Painter, center: egui::Pos2, size: f32, color: Color32, t: f32) {
    for k in 0..3 {
        let phase = (t * 3.0 - k as f32 * 0.45).rem_euclid(std::f32::consts::TAU);
        let lift = (phase.sin().max(0.0)).powf(2.0);
        let c = center + vec2((k as f32 - 1.0) * size * 2.6, -lift * size * 0.9);
        painter.circle_filled(c, size * (0.75 + 0.25 * lift), color.gamma_multiply(0.45 + 0.55 * lift));
    }
}

/// "…" that grows, for a line of text.
pub(super) fn ellipsis(t: f32) -> &'static str {
    ["", ".", "..", "..."][(t * 2.5) as usize % 4]
}

/// What a source's tile shows when there's no fresh picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TileWait {
    /// Pictures are arriving.
    Fine,
    /// Opening, and no picture yet.
    Connecting,
    /// A network source waiting for whoever sends it (it says who).
    Waiting,
    /// It was working a moment ago; Cutaway is getting it back.
    Reconnecting,
    /// Nothing, for a while now.
    NoSignal,
}

/// How long a camera gets to show its first picture, and to come back,
/// before the tile says NO SIGNAL.
const CONNECT_FOR: Duration = Duration::from_secs(15);
const RECONNECT_FOR: Duration = Duration::from_secs(20);

pub(super) fn tile_wait(offline: bool, stale: bool, since_last: Option<Duration>, age: Duration, net_detail: bool) -> TileWait {
    if !offline && !stale {
        return TileWait::Fine;
    }
    match since_last {
        None if net_detail => TileWait::Waiting,
        None if !offline && age < CONNECT_FOR => TileWait::Connecting,
        Some(d) if d < RECONNECT_FOR => TileWait::Reconnecting,
        _ if net_detail => TileWait::Waiting,
        _ => TileWait::NoSignal,
    }
}

impl TileWait {
    pub(super) fn of(slot: &FrameSlot, offline: bool, net_detail: bool) -> Self {
        tile_wait(offline, slot.stale(Duration::from_secs(2)), slot.since_last(), slot.age(), net_detail)
    }
}

/// Draws the tile's waiting state over its picture.
pub(super) fn draw_tile_wait(painter: &egui::Painter, pic: Rect, wait: TileWait, detail: Option<&str>, t: f32) {
    if wait == TileWait::Fine {
        return;
    }
    painter.rect_filled(pic, 3.0, Color32::from_black_alpha(170));
    let small = pic.height() < 90.0;
    let (title, color) = match wait {
        TileWait::Connecting => ("Connecting", Color32::from_gray(225)),
        TileWait::Waiting => ("Standing by", Color32::from_gray(225)),
        TileWait::Reconnecting => ("Reconnecting", AMBER),
        _ => ("NO SIGNAL", AMBER),
    };
    let mid = pic.center();
    if wait == TileWait::NoSignal {
        painter.text(mid - vec2(0.0, 8.0), Align2::CENTER_CENTER, title, FontId::proportional(16.0), AMBER);
    } else {
        if !small {
            dots(painter, mid - vec2(0.0, 26.0), 4.5, color, t);
        }
        // The dots after it grow without the word moving.
        let font = FontId::proportional(15.0);
        let g = painter.layout_no_wrap(title.to_string(), font.clone(), color);
        let left = mid.x - g.size().x / 2.0;
        painter.galley(pos2(left, mid.y - 8.0 - g.size().y / 2.0), g.clone(), color);
        painter.text(pos2(left + g.size().x, mid.y - 8.0), Align2::LEFT_CENTER, ellipsis(t), font, color);
    }
    let detail = detail.map(str::to_string).or_else(|| match wait {
        TileWait::Connecting => Some("Waiting for the first picture".into()),
        TileWait::Reconnecting => Some("The picture stopped. Getting it back".into()),
        _ => None,
    });
    if let Some(d) = detail {
        // (Network sources say "waiting for the sender".)
        let mut c = d.chars();
        let d = c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default();
        painter.text(mid + vec2(0.0, 12.0), Align2::CENTER_CENTER, d, FontId::proportional(11.0), TEXT_DIM);
    }
}

/// What a floor manager might say while a take is put back together.
const RESCUE_LINES: &[&str] = &[
    "Picking up the pieces",
    "Splicing the film back together",
    "Sending it to the edit suite",
    "Sweeping the cutting-room floor",
    "Finding every last frame",
];

/// A take being rescued after Cutaway stopped mid-recording.
pub(super) struct Rescue {
    pub state: Arc<Mutex<RescueState>>,
    since: Instant,
    line: usize,
}

impl Rescue {
    pub(super) fn new(state: Arc<Mutex<RescueState>>) -> Self {
        let line = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0) % RESCUE_LINES.len();
        Rescue { state, since: Instant::now(), line }
    }
}

impl App {
    /// The rescue card: how it's going, then what was saved.
    pub(super) fn rescue_card(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.rescue else { return };
        let st = r.state.lock_ok().clone();
        let t = r.since.elapsed().as_secs_f32();
        let line = RESCUE_LINES[r.line];
        let mut close = false;
        egui::Area::new(egui::Id::unique("rescue")).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, vec2(0.0, 90.0)).show(ctx, |ui| {
            egui::Frame::new()
                .fill(RAISED)
                .stroke(Stroke::new(1.0, LINE))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20))
                .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(120) })
                .show(ui, |ui| {
                    ui.set_width(440.0);
                    let takes = st.takes.len();
                    // A strip of film, the frame under the splicer lit up.
                    let (strip, _) = ui.allocate_exact_size(vec2(440.0, 58.0), egui::Sense::hover());
                    film_strip(ui.painter(), strip, if st.finished { 0.0 } else { t }, st.finished);
                    ui.add_space(12.0);
                    if !st.finished {
                        ui.label(RichText::new(format!("{line}{}", ellipsis(t))).size(20.0).color(Color32::WHITE));
                        ui.add_space(2.0);
                        let what = if takes == 1 { "Your last take" } else { "Some takes" };
                        ui.label(RichText::new(format!("{what} didn't finish properly (Cutaway stopped while recording). Everything up to about the last second is on disk, and Cutaway is turning it back into files that play.")).color(TEXT_DIM));
                        ui.add_space(10.0);
                        ui.add(egui::ProgressBar::new(st.fraction()).desired_height(8.0).fill(ACCENT));
                        ui.add_space(4.0);
                        let mut now = if st.file.is_empty() { "Looking at the files".to_string() } else { format!("{}: file {} of {}", st.file, st.file_index + 1, st.files) };
                        if takes > 1 {
                            now = format!("Take {} of {takes} · {now}", st.take + 1);
                        }
                        ui.label(RichText::new(now).monospace().size(11.5).color(TEXT_DIM));
                        ui.add_space(6.0);
                        ui.label(RichText::new("You can carry on setting up while this finishes.").size(11.5).color(TEXT_DIM));
                    } else {
                        let saved = st.rescued.len();
                        let title = match (saved, takes) {
                            (0, _) => "Your last take couldn't be rescued".to_string(),
                            (1, 1) => "Your last take is safe".to_string(),
                            (n, m) if n == m => format!("All {n} takes are safe"),
                            (n, m) => format!("{n} of {m} takes are safe"),
                        };
                        ui.label(RichText::new(title).size(20.0).color(Color32::WHITE));
                        ui.add_space(2.0);
                        let body = if saved == 0 {
                            "Cutaway couldn't write to its folder. The files are still there as they were left."
                        } else {
                            "It plays up to about the last second before Cutaway stopped. A note in the folder (Take.txt) says what was saved from each file."
                        };
                        ui.label(RichText::new(body).color(TEXT_DIM));
                        if let Some(dir) = st.rescued.first().or(st.takes.first()) {
                            ui.add_space(4.0);
                            ui.add(egui::Label::new(RichText::new(dir.display().to_string()).monospace().size(11.0).color(TEXT_DIM)).truncate());
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new("OK").color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.6)).min_size(vec2(80.0, 28.0))).clicked() {
                                close = true;
                            }
                            if let Some(dir) = st.rescued.first().or(st.takes.first()) {
                                if ui.add(egui::Button::new("Show the folder").min_size(vec2(0.0, 28.0))).clicked() {
                                    let _ = crate::util::open_with_system(dir);
                                    close = true;
                                }
                            }
                        });
                    }
                });
        });
        if close {
            self.rescue = None;
        } else if !st.finished {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// A strip of film running under a splicer; once done, it stops with a tick.
fn film_strip(painter: &egui::Painter, area: Rect, t: f32, done: bool) {
    let painter = painter.with_clip_rect(area);
    let band = Rect::from_center_size(area.center(), vec2(area.width(), 44.0));
    painter.rect_filled(band, 4.0, Color32::from_rgb(20, 22, 27));
    let fw = 52.0;
    let shift = (t * 40.0) % fw;
    let mut x = band.left() - fw + shift;
    let holes = |y: f32, x: f32| {
        for k in 0..4 {
            let hx = x + 6.0 + k as f32 * 12.0;
            painter.rect_filled(Rect::from_min_size(pos2(hx, y), vec2(6.0, 4.0)), 1.0, Color32::from_gray(70));
        }
    };
    let mid = area.center().x;
    while x < band.right() + fw {
        let frame = Rect::from_min_size(pos2(x + 3.0, band.top() + 8.0), vec2(fw - 6.0, band.height() - 16.0));
        // Frames past the splicer are mended (lit); the ones still to come are dim.
        let mended = done || frame.center().x > mid;
        painter.rect_filled(frame, 2.0, if mended { Color32::from_rgb(52, 74, 104) } else { Color32::from_rgb(36, 39, 46) });
        holes(band.top() + 2.0, x);
        holes(band.bottom() - 6.0, x);
        x += fw;
    }
    if done {
        let c = area.center();
        painter.circle_filled(c, 15.0, GREEN);
        painter.line_segment([c + vec2(-6.0, 0.0), c + vec2(-1.5, 5.0)], Stroke::new(2.5, Color32::WHITE));
        painter.line_segment([c + vec2(-1.5, 5.0), c + vec2(7.0, -5.0)], Stroke::new(2.5, Color32::WHITE));
    } else {
        // The splicer: a bright bar the film runs through, with a little spark.
        let bar = Rect::from_center_size(pos2(mid, area.center().y), vec2(6.0, area.height()));
        painter.rect_filled(bar, 2.0, ACCENT);
        let spark = (t * 7.0).sin().abs();
        painter.circle_filled(pos2(mid, area.center().y), 3.0 + spark * 3.0, Color32::WHITE.gamma_multiply(0.4 + 0.6 * spark));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_wait_before_they_give_up() {
        let s = Duration::from_secs;
        assert_eq!(tile_wait(false, false, Some(s(0)), s(60), false), TileWait::Fine);
        // Never had a picture.
        assert_eq!(tile_wait(false, true, None, s(3), false), TileWait::Connecting);
        assert_eq!(tile_wait(false, true, None, s(30), false), TileWait::NoSignal);
        assert_eq!(tile_wait(true, true, None, s(3), false), TileWait::NoSignal, "couldn't open: no point waiting");
        assert_eq!(tile_wait(true, true, None, s(300), true), TileWait::Waiting, "a guest who hasn't joined yet");
        // Had one, lost it.
        assert_eq!(tile_wait(true, true, Some(s(5)), s(300), false), TileWait::Reconnecting);
        assert_eq!(tile_wait(true, true, Some(s(60)), s(300), false), TileWait::NoSignal);
        assert_eq!(tile_wait(true, true, Some(s(60)), s(300), true), TileWait::Waiting, "a guest who left");
    }
}
