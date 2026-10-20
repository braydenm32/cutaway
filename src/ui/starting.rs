//! Starting the engine without the window waiting: cameras are opened (and
//! the old engine stopped) in the background, while the monitors show a
//! "starting" screen with a line from the studio floor.

use super::theme::*;
use super::App;
use crate::engine::Engine;
use crossbeam_channel::{bounded, Receiver};
use eframe::egui::{self, vec2, Color32, Stroke};
use std::time::{Duration, Instant};

/// What a floor manager might say while the cameras come up.
pub const LINES: &[(&str, &str)] = &[
    ("Quiet on set!", "The cameras are warming up."),
    ("Places, everyone.", "Cameras coming up."),
    ("Stand by, studio.", "Opening the cameras."),
    ("Hold for picture…", "The cameras are on their way."),
    ("Scene one, take one.", "Just as soon as the cameras are ready."),
    ("Still in make-up.", "The cameras will be out in a moment."),
    ("Rolling… speed…", "Waiting on the cameras."),
    ("Lights, camera…", "Camera, singular, for now. The rest are coming."),
    ("Checking the gate.", "Cameras up in a second."),
    ("Back in five, four, three…", "Cameras coming up."),
];

/// A pick from the lines, different each time.
fn pick() -> usize {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize ^ d.as_secs() as usize).unwrap_or(0);
    n % LINES.len()
}

/// An engine on its way.
pub(super) struct Starting {
    rx: Receiver<Result<Engine, String>>,
    since: Instant,
    line: usize,
    /// How many cameras and inputs it's opening (for the message).
    sources: usize,
    /// Turn the virtual camera on once it's running.
    pub vcam: bool,
}

impl App {
    /// (Re)start the engine in the background. The window carries on; the
    /// monitors say what's happening until it's ready.
    pub(super) fn start_engine(&mut self) {
        if let Some(s) = &mut self.starting {
            // Already starting: start again with these settings once it's up.
            self.restart_again = true;
            s.vcam |= self.engine.as_ref().is_some_and(|e| e.virtual_camera_on());
            return;
        }
        self.starts_count += 1;
        let old = self.engine.take();
        let vcam = old.as_ref().is_some_and(|e| e.virtual_camera_on()) || self.vcam_wanted;
        self.vcam_wanted = false;
        self.textures.clear();
        self.peak_hold.clear();
        self.engine_error = None;
        let (cfg, waker) = (self.cfg.clone(), self.waker.clone());
        let (tx, rx) = bounded(1);
        let spawned = std::thread::Builder::new().name("starting".into()).spawn(move || {
            // The old one lets go of the cameras first.
            if let Some(mut e) = old {
                e.shutdown();
            }
            // (Screenshots of this screen: hold it a moment.)
            if let Some(secs) = std::env::var("CUTAWAY_UI_TEST_SLOW_START").ok().and_then(|v| v.parse::<f64>().ok()) {
                std::thread::sleep(Duration::from_secs_f64(secs.clamp(0.0, 30.0)));
            }
            let r = Engine::start(&cfg, waker.clone()).map_err(|e| format!("{e:#}"));
            if let Err(crossbeam_channel::SendError(Ok(mut e))) = tx.send(r) {
                // Nobody waiting (closing): don't leave it running.
                e.shutdown();
            }
            waker();
        });
        match spawned {
            Ok(_) => self.starting = Some(Starting { rx, since: Instant::now(), line: pick(), sources: self.cfg.sources.len(), vcam }),
            Err(e) => self.engine_error = Some(format!("couldn't start ({e})")),
        }
    }

    /// The engine arrived (or couldn't start).
    pub(super) fn tick_starting(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.starting else { return };
        let got = match s.rx.try_recv() {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err("starting stopped unexpectedly".into()),
        };
        let vcam = s.vcam;
        self.starting = None;
        match got {
            Ok(e) => {
                *self.remote_shared.program.lock().unwrap() = Some(e.program_preview.clone());
                self.engine = Some(e);
                self.engine_error = None;
                if self.restart_again {
                    // Settings changed again while it started.
                    self.restart_again = false;
                    self.vcam_wanted = vcam;
                    self.start_engine();
                    return;
                }
                if vcam {
                    self.set_virtual_camera(true);
                }
            }
            Err(e) => {
                self.engine_error = Some(e);
                self.restart_again = false;
            }
        }
    }

    /// Wait (a while) for an engine that's starting, so closing the app
    /// stops it properly.
    pub(super) fn finish_starting(&mut self) {
        if let Some(s) = self.starting.take() {
            if let Ok(Ok(e)) = s.rx.recv_timeout(Duration::from_secs(15)) {
                self.engine = Some(e);
            }
        }
    }

    pub(super) fn is_starting(&self) -> bool {
        self.starting.is_some()
    }

    /// The monitors while the engine starts: a slate with a line from the floor.
    pub(super) fn starting_screen(&self, ui: &mut egui::Ui) {
        let Some(s) = &self.starting else { return };
        let (title, sub) = LINES[s.line % LINES.len()];
        let area = ui.available_rect_before_wrap();
        let painter = ui.painter_at(area);
        // A clapperboard: striped top, slate below.
        let w = (area.width() * 0.42).clamp(260.0, 460.0);
        let h = w * 0.62;
        let top = area.center().y - h * 0.62;
        let board = egui::Rect::from_min_size(egui::pos2(area.center().x - w / 2.0, top + h * 0.22), vec2(w, h * 0.78));
        let sticks = egui::Rect::from_min_size(egui::pos2(board.left(), top), vec2(w, h * 0.17));
        // The top stick claps open and shut.
        let t = s.since.elapsed().as_secs_f32();
        let open = (((t * 1.4).sin() + 1.0) * 0.5).powf(3.0) * 0.32;
        painter.rect_filled(board, 6.0, Color32::from_rgb(28, 31, 38));
        painter.rect_stroke(board, 6.0, Stroke::new(1.5, LINE), egui::StrokeKind::Inside);
        let stripes = |r: egui::Rect, angle: f32| {
            let pivot = r.left_bottom();
            let rot = egui::emath::Rot2::from_angle(-angle);
            let map = |p: egui::Pos2| pivot + rot * (p - pivot);
            let n = 7;
            let sw = r.width() / n as f32;
            for k in 0..n {
                let x0 = r.left() + k as f32 * sw;
                let skew = r.height() * 0.6;
                let pts = vec![map(egui::pos2(x0, r.bottom())), map(egui::pos2(x0 + skew, r.top())), map(egui::pos2(x0 + skew + sw, r.top())), map(egui::pos2(x0 + sw, r.bottom()))];
                let c = if k % 2 == 0 { Color32::from_gray(235) } else { Color32::from_gray(25) };
                painter.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
            }
        };
        let base = egui::Rect::from_min_size(egui::pos2(board.left(), board.top() - h * 0.12), vec2(w, h * 0.12));
        stripes(base, 0.0);
        stripes(sticks.translate(vec2(0.0, base.top() - sticks.bottom())), open);
        // The slate's writing.
        let inner = board.shrink(18.0);
        painter.text(egui::pos2(inner.left(), inner.top()), egui::Align2::LEFT_TOP, "CUTAWAY", egui::FontId::monospace(13.0), TEXT_DIM);
        let take = format!("TAKE {}", self.starts_count);
        painter.text(egui::pos2(inner.right(), inner.top()), egui::Align2::RIGHT_TOP, take, egui::FontId::monospace(13.0), TEXT_DIM);
        painter.text(egui::pos2(inner.center().x, inner.center().y - 6.0), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional((w / 13.0).clamp(20.0, 32.0)), Color32::WHITE);
        painter.text(egui::pos2(inner.center().x, inner.center().y + 26.0), egui::Align2::CENTER_CENTER, sub, egui::FontId::proportional(15.0), TEXT_DIM);
        let what = match s.sources {
            0 => "Getting the studio ready".to_string(),
            1 => "Starting 1 camera".to_string(),
            n => format!("Starting {n} cameras"),
        };
        let dots = ".".repeat(1 + (t * 2.0) as usize % 3);
        painter.text(egui::pos2(inner.center().x, inner.bottom()), egui::Align2::CENTER_BOTTOM, format!("{what}{dots}"), egui::FontId::monospace(12.5), ACCENT);
        ui.allocate_rect(area, egui::Sense::hover());
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}
