//! Sources › Cameras › Check cameras: reads each camera's normal picture
//! and a white page held in front of it, then says what to change on each
//! camera so they match (the measuring is `engine::camcheck`). Advice only.

use super::preview::{draw_picture, placeholder};
use super::*;
use crate::config::{CameraModel, SourceConfig};
use crate::engine::camcheck::{self, Camera, Context, Finding, Mark, Page, Scene, Setting};
use crate::engine::{Frame, FrameSlot};
use eframe::egui::{Rect, Sense, Stroke, StrokeKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the normal picture is watched (for flicker and grain).
const SCENE_TIME: Duration = Duration::from_millis(2200);
/// How long to wait for the page before giving up.
const PAGE_TIME: Duration = Duration::from_secs(40);

/// What one camera's readings came to.
#[derive(Clone, Default)]
struct Reading {
    scene: Option<Scene>,
    page: Option<Page>,
    /// The picture's brightness when it was read, to notice the light
    /// changing afterwards.
    level: f32,
    /// The picture without the page (to find the page against).
    before: Option<Arc<Frame>>,
}

/// A reading under way on a thread of its own.
struct Job {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<JobState>>,
}

#[derive(Default)]
struct JobState {
    /// The page where it's seen now.
    seen: Option<Page>,
    /// 0..1 for the normal picture's reading.
    progress: f32,
    done: bool,
    scene: Option<Scene>,
    page: Option<Page>,
    level: f32,
    before: Option<Arc<Frame>>,
    /// No picture came.
    no_picture: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The picture's average brightness (0..1), quickly.
fn level_of(f: &Frame) -> f32 {
    let px = f.rgba.chunks_exact(4).step_by(11);
    let (sum, n) = px.fold((0.0f32, 0u32), |(a, n), p| (a + 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32, n + 1));
    sum / n.max(1) as f32 / 255.0
}

/// Faces in a picture, as fractions of it (left, top, width, height), when
/// this computer can find people.
fn faces(f: &Frame) -> Vec<[f32; 4]> {
    let Ok(mut d) = crate::platform::vision::people_detector() else { return Vec::new() };
    let (w, h) = (f.width & !1, f.height & !1);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    let mut y = vec![0u8; w * h];
    let mut uv = vec![128u8; w * h / 2];
    for row in 0..h {
        for col in 0..w {
            let p = &f.rgba[(row * f.width + col) * 4..];
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            y[row * w + col] = (16.0 + 219.0 / 255.0 * (0.2126 * r + 0.7152 * g + 0.0722 * b)).round().clamp(0.0, 255.0) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                let cb = 128.0 + 224.0 / 255.0 * (-0.1146 * r - 0.3854 * g + 0.5 * b);
                let cr = 128.0 + 224.0 / 255.0 * (0.5 * r - 0.4542 * g - 0.0458 * b);
                uv[(row / 2) * w + col] = cb.round().clamp(0.0, 255.0) as u8;
                uv[(row / 2) * w + col + 1] = cr.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    let frame = crate::media::VideoFrame {
        width: w as u32,
        height: h as u32,
        pts: 0,
        pixels: Arc::new(crate::media::Pixels::Nv12 { y, y_stride: w, uv, uv_stride: w, recycle: false }),
        serial: 0,
    };
    d.detect(&frame)
        .map(|found| found.into_iter().filter(|d| d.face).map(|d| [d.rect[0] / w as f32, d.rect[1] / h as f32, d.rect[2] / w as f32, d.rect[3] / h as f32]).collect())
        .unwrap_or_default()
}

impl Job {
    /// Watches the normal picture for a couple of seconds.
    fn scene(slot: Arc<FrameSlot>) -> Job {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(JobState::default()));
        let (st, s2) = (stop.clone(), state.clone());
        let _ = std::thread::Builder::new().name("check camera".into()).spawn(move || {
            let start = Instant::now();
            let mut frames: Vec<Arc<Frame>> = Vec::new();
            let mut last = None;
            while start.elapsed() < SCENE_TIME && !st.load(Ordering::Relaxed) {
                if let Some((g, f)) = slot.latest() {
                    if last != Some(g) && frames.len() < 90 {
                        last = Some(g);
                        frames.push(f);
                    }
                }
                s2.lock_ok().progress = start.elapsed().as_secs_f32() / SCENE_TIME.as_secs_f32();
                std::thread::sleep(Duration::from_millis(8));
            }
            if st.load(Ordering::Relaxed) {
                return;
            }
            let mid = frames.get(frames.len() / 2).cloned();
            let found = mid.as_deref().map(faces).unwrap_or_default();
            let scene = camcheck::measure_scene(&frames.iter().map(|f| f.as_ref()).collect::<Vec<_>>(), &found);
            let mut s = s2.lock_ok();
            s.no_picture = mid.is_none();
            s.level = mid.as_deref().map(level_of).unwrap_or(0.0);
            s.scene = scene;
            s.before = mid;
            s.done = true;
        });
        Job { stop, state }
    }

    /// Looks for the white page until it's held still.
    fn page(slot: Arc<FrameSlot>, before: Option<Arc<Frame>>) -> Job {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(JobState::default()));
        let (st, s2) = (stop.clone(), state.clone());
        let _ = std::thread::Builder::new().name("check camera page".into()).spawn(move || {
            let start = Instant::now();
            let mut last = None;
            let mut pages: Vec<Page> = Vec::new();
            while start.elapsed() < PAGE_TIME && !st.load(Ordering::Relaxed) {
                if let Some((g, f)) = slot.latest() {
                    if last != Some(g) {
                        last = Some(g);
                        match camcheck::find_page(&f, before.as_deref()) {
                            Some(p) => pages.push(p),
                            None => pages.clear(),
                        }
                        s2.lock_ok().seen = pages.last().copied();
                        if let Some(p) = camcheck::settled(&pages) {
                            let mut s = s2.lock_ok();
                            s.page = Some(p);
                            s.done = true;
                            return;
                        }
                    }
                }
                s2.lock_ok().progress = start.elapsed().as_secs_f32() / PAGE_TIME.as_secs_f32();
                std::thread::sleep(Duration::from_millis(30));
            }
            s2.lock_ok().done = true;
        });
        Job { stop, state }
    }
}

enum Stage {
    /// Pick the reference camera and whether to use a page.
    Setup,
    /// Reading the normal picture of `cams[i]`.
    Scene(usize, Job),
    /// Waiting for the page in front of `cams[i]`.
    Page(usize, Job),
    Results,
}

pub struct CamCheck {
    stage: Stage,
    /// The cameras checked (by uid), in the list's order.
    cams: Vec<u64>,
    /// The one the others are matched to.
    reference: u64,
    use_page: bool,
    readings: std::collections::HashMap<u64, Reading>,
    /// Checking just this one again (then back to the results).
    only: Option<u64>,
    /// When the results were made, "17:40".
    when: String,
    /// What was "Mark as done" (by uid).
    marked: HashSet<u64>,
    /// The camera to read next, once its picture is at hand.
    pending: Option<usize>,
}

/// The sources Check cameras looks at: cameras and capture cards (or, with
/// none, anything with a picture of its own, so it can be tried with test
/// pictures).
fn checkable(sources: &[SourceConfig], running: &[(u64, Arc<FrameSlot>)]) -> Vec<u64> {
    let runs = |s: &SourceConfig| running.iter().any(|(u, _)| *u == s.uid);
    let cams: Vec<u64> = sources.iter().filter(|s| runs(s) && sources::is_camera(s)).map(|s| s.uid).collect();
    if !cams.is_empty() {
        return cams;
    }
    sources.iter().filter(|s| runs(s) && sources::is_test(s)).map(|s| s.uid).collect()
}

fn mark_colour(m: Mark) -> Color32 {
    match m {
        Mark::Good => GREEN,
        Mark::Warn => AMBER,
        Mark::Bad => ERROR,
        Mark::Skipped => TEXT_DIM,
    }
}

fn mark_word(m: Mark) -> &'static str {
    match m {
        Mark::Good => "Good",
        Mark::Warn => "Worth fixing",
        Mark::Bad => "Fix this",
        Mark::Skipped => "Not checked",
    }
}

fn dot(ui: &mut egui::Ui, m: Mark) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 5.0, mark_colour(m));
}

impl SettingsUi {
    /// The Check cameras button (Sources › Cameras), or nothing when no
    /// camera is running.
    pub(super) fn camcheck_button(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let cams = checkable(&self.draft.sources, &info.slots);
        let b = ui.add_enabled(!cams.is_empty(), egui::Button::new("Check cameras…"));
        let b = if cams.is_empty() { b.on_disabled_hover_text("Once your cameras are running (Apply), this checks their pictures.") } else { b.on_hover_text("Measures each camera's picture and a white page held in front of it, and says what to change so they match. Changes nothing itself.") };
        if b.clicked() {
            self.camcheck = Some(CamCheck {
                stage: Stage::Setup,
                reference: cams[0],
                cams,
                use_page: true,
                readings: Default::default(),
                only: None,
                when: String::new(),
                marked: HashSet::new(),
                pending: None,
            });
        }
    }

    /// The check, in place of the list of sources while it's open.
    pub(super) fn camcheck(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let Some(mut c) = self.camcheck.take() else { return };
        // Cameras that stopped (or were removed) drop out.
        let running = checkable(&self.draft.sources, &info.slots);
        c.cams.retain(|u| running.contains(u));
        if c.cams.is_empty() {
            hint(ui, "No cameras are running now.");
            if ui.button("Close").clicked() {
                return;
            }
            self.camcheck = Some(c);
            return;
        }
        if !c.cams.contains(&c.reference) {
            c.reference = c.cams[0];
        }
        if let Some(i) = c.pending.take() {
            match c.cams.get(i).and_then(|&u| Self::slot_of(info, u)) {
                Some(slot) => c.stage = Stage::Scene(i, Job::scene(slot)),
                None => c.advance(i),
            }
        }
        let keep = match &c.stage {
            Stage::Setup => self.camcheck_setup(ui, &mut c),
            Stage::Scene(..) | Stage::Page(..) => self.camcheck_reading(ui, info, &mut c),
            Stage::Results => self.camcheck_results(ui, info, &mut c),
        };
        if keep {
            self.camcheck = Some(c);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }

    fn name_of(&self, uid: u64) -> String {
        self.draft.sources.iter().find(|s| s.uid == uid).map(|s| s.name.clone()).unwrap_or_default()
    }

    fn slot_of(info: &SettingsContext, uid: u64) -> Option<Arc<FrameSlot>> {
        info.slots.iter().find(|(u, _)| *u == uid).map(|(_, s)| s.clone())
    }

    fn camcheck_setup(&mut self, ui: &mut egui::Ui, c: &mut CamCheck) -> bool {
        egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            ui.label(RichText::new("Check cameras").size(TEXT_HEADING).strong());
            ui.add_space(4.0);
            hint(ui, &format!(
                "Checks {} for colour, brightness, flicker, black level, focus and grain, and says what to change on each camera. It changes nothing itself, and nothing leaves this computer. About 20 seconds a camera.",
                match c.cams.len() {
                    1 => "1 camera".to_string(),
                    n => format!("{n} cameras"),
                }
            ));
            ui.add_space(8.0);
            egui::Grid::new("camcheck_setup").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Match the others to");
                let shown = self.name_of(c.reference);
                egui::ComboBox::from_id_salt("camcheck_ref").selected_text(shown).width(200.0).show_ui(ui, |ui| {
                    for &u in &c.cams {
                        ui.selectable_value(&mut c.reference, u, self.name_of(u));
                    }
                });
                ui.end_row();
                label(ui, "White page");
                ui.checkbox(&mut c.use_page, "hold a sheet of plain white paper in front of each camera (best for colour)");
                ui.end_row();
            });
            ui.add_space(6.0);
            hint(ui, "Before you start: room lights on as they'll be for the show. Have one sheet of plain white paper, and use the same one for every camera.");
            ui.add_space(8.0);
        });
        ui.add_space(8.0);
        let mut keep = true;
        ui.horizontal(|ui| {
            let go = egui::Button::new(RichText::new("Start").color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55));
            if ui.add(go).clicked() {
                c.readings.clear();
                c.marked.clear();
                c.only = None;
                c.next(0);
            }
            if !c.readings.is_empty() && ui.button("Back to the results").clicked() {
                c.stage = Stage::Results;
            }
            if ui.button("Close").clicked() {
                keep = false;
            }
        });
        keep
    }

    fn camcheck_reading(&mut self, ui: &mut egui::Ui, info: &SettingsContext, c: &mut CamCheck) -> bool {
        // Start the job a stage needs, and move on when it's done.
        let (i, page_stage) = match &c.stage {
            Stage::Scene(i, _) => (*i, false),
            Stage::Page(i, _) => (*i, true),
            _ => return true,
        };
        let uid = c.cams[i];
        let name = self.name_of(uid);
        let Some(slot) = Self::slot_of(info, uid) else {
            c.advance(i);
            return true;
        };
        let state = match &c.stage {
            Stage::Scene(_, j) | Stage::Page(_, j) => j.state.clone(),
            _ => return true,
        };
        if state.lock_ok().done {
            let mut s = state.lock_ok();
            let r = c.readings.entry(uid).or_default();
            if page_stage {
                r.page = s.page.take();
                drop(s);
                c.advance(i);
            } else {
                r.scene = s.scene.take();
                r.level = s.level;
                r.before = s.before.take();
                drop(s);
                if c.use_page {
                    let before = c.readings.get(&uid).and_then(|r| r.before.clone());
                    c.stage = Stage::Page(i, Job::page(slot, before));
                } else {
                    c.advance(i);
                }
            }
            return true;
        }
        let (seen, progress) = {
            let s = state.lock_ok();
            (s.seen, s.progress)
        };
        let count = if c.only.is_some() { String::new() } else { format!("Camera {} of {}: ", i + 1, c.cams.len()) };
        ui.label(RichText::new(format!("{count}{name}")).size(TEXT_HEADING).strong());
        ui.add_space(2.0);
        if page_stage {
            ui.label(RichText::new("Hold the white page where a guest sits, facing this camera, so it fills about a quarter of the picture. Keep your fingers at the edges and hold it still.").size(TEXT_STATE));
            hint(ui, match seen {
                Some(p) if p.clipped => "Got it, but it's coming out pure white: hold still while it reads.",
                Some(_) => "Found it: hold still…",
                None => "Looking for the page…",
            });
        } else {
            ui.label(RichText::new("Step out of the shot and keep things still for two seconds: reading the normal picture.").size(TEXT_STATE));
            ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).desired_width(300.0));
        }
        ui.add_space(6.0);
        match info.pictures.iter().find(|p| p.uid == uid) {
            Some(pic) => {
                let (r, _) = draw_picture(ui, pic, 560.0, Sense::hover());
                if let Some(p) = seen.filter(|_| page_stage) {
                    let b = Rect::from_min_max(r.lerp_inside(vec2(p.rect[0], p.rect[1])), r.lerp_inside(vec2(p.rect[2], p.rect[3])));
                    ui.painter().rect_stroke(b, 2.0, Stroke::new(2.5, GREEN), StrokeKind::Outside);
                }
            }
            None => placeholder(ui, "No picture from it right now."),
        }
        ui.add_space(8.0);
        let mut keep = true;
        ui.horizontal(|ui| {
            if page_stage && ui.button("Skip the page for this camera").clicked() {
                c.advance(i);
            }
            if c.only.is_none() && i + 1 < c.cams.len() && ui.button("Skip this camera").clicked() {
                c.readings.remove(&uid);
                c.next(i + 1);
            }
            if ui.button("Stop").clicked() {
                if c.readings.is_empty() {
                    keep = false;
                } else {
                    c.stage = Stage::Results;
                    c.stamp();
                }
            }
        });
        keep
    }

    /// The findings of every camera, the reference first in the list order.
    fn camcheck_findings(&self, c: &CamCheck, info: &SettingsContext) -> Vec<(u64, Vec<Finding>)> {
        let cx = Context { show_fps: info.show_fps };
        let find = |u: u64| self.draft.sources.iter().find(|s| s.uid == u);
        let camera = |s: &'_ SourceConfig, r: &Reading| -> (String, CameraModel, crate::config::CameraNotes, crate::config::SourceLook, Option<Page>, Option<Scene>) {
            (s.name.clone(), s.camera.model_for(&s.device), s.camera.clone(), s.look.clone(), r.page, r.scene)
        };
        let reference = find(c.reference).zip(c.readings.get(&c.reference)).map(|(s, r)| camera(s, r));
        c.cams
            .iter()
            .filter_map(|&u| {
                let (s, r) = (find(u)?, c.readings.get(&u)?);
                let me = camera(s, r);
                let cam = Camera { name: &me.0, model: me.1, notes: &me.2, look: &me.3, page: me.4, scene: me.5 };
                let found = if u == c.reference {
                    camcheck::advise(&cam, None, &cx)
                } else {
                    let rf = reference.as_ref().map(|m| Camera { name: &m.0, model: m.1, notes: &m.2, look: &m.3, page: m.4, scene: m.5 });
                    camcheck::advise(&cam, rf.as_ref(), &cx)
                };
                Some((u, found))
            })
            .collect()
    }

    fn camcheck_results(&mut self, ui: &mut egui::Ui, info: &SettingsContext, c: &mut CamCheck) -> bool {
        let all = self.camcheck_findings(c, info);
        let mut keep = true;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Check cameras").size(TEXT_HEADING).strong());
            hint(ui, &format!("{} · matched to {}", c.when, self.name_of(c.reference)));
        });
        ui.add_space(4.0);
        // The strip: every camera side by side, with its page.
        egui::ScrollArea::horizontal().id_salt("camcheck_strip").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (u, found) in &all {
                    ui.vertical(|ui| {
                        ui.set_width(168.0);
                        match info.pictures.iter().find(|p| p.uid == *u) {
                            Some(pic) => {
                                let (r, _) = draw_picture(ui, pic, 168.0, Sense::hover());
                                let page = c.readings.get(u).and_then(|r| r.page);
                                if let Some(p) = page {
                                    let look = self.draft.sources.iter().find(|s| s.uid == *u).map(|s| s.look.clone()).unwrap_or_default();
                                    let rgb = camcheck::apply_look(p.rgb, &look).map(|v| (v * 255.0).round() as u8);
                                    let sw = Rect::from_min_size(r.right_bottom() - vec2(44.0, 30.0), vec2(40.0, 26.0));
                                    ui.painter().rect_filled(sw, 3.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                                    ui.painter().rect_stroke(sw, 3.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
                                }
                            }
                            None => {
                                ui.allocate_exact_size(vec2(168.0, 94.0), Sense::hover());
                            }
                        }
                        ui.horizontal(|ui| {
                            dot(ui, camcheck::overall(found));
                            let name = self.name_of(*u);
                            ui.label(RichText::new(if *u == c.reference { format!("{name} (reference)") } else { name }).size(TEXT_BODY));
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        });
        hint(ui, "The square on each picture is the white page as it comes out on the program: they should look the same.");
        ui.add_space(8.0);
        let mut again = None;
        let mut mark_done = None;
        for (u, found) in &all {
            let name = self.name_of(*u);
            let src = self.draft.sources.iter().find(|s| s.uid == *u);
            let worst = camcheck::overall(found);
            let changed_light = Self::slot_of(info, *u).and_then(|s| s.latest()).zip(c.readings.get(u)).is_some_and(|((_, f), r)| r.level > 0.02 && (level_of(&f) - r.level).abs() / r.level > 0.25);
            egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width().min(760.0));
                ui.horizontal(|ui| {
                    dot(ui, worst);
                    ui.label(RichText::new(&name).strong().size(TEXT_STATE));
                    ui.label(RichText::new(mark_word(worst)).color(mark_colour(worst)).size(TEXT_BODY));
                    if let Some(s) = src {
                        hint(ui, s.camera.model_for(&s.device).label());
                    }
                });
                if changed_light {
                    ui.label(RichText::new("The light has changed since this was checked: check again before trusting it.").color(AMBER).size(TEXT_BODY));
                }
                if c.marked.contains(u) {
                    ui.label(RichText::new("✓ Marked as done").color(GREEN).size(TEXT_BODY));
                }
                // The one or two fixes that matter most.
                let mut todo: Vec<&Finding> = found.iter().filter(|f| f.fix.is_some() && f.mark >= Mark::Warn).collect();
                todo.sort_by_key(|f| std::cmp::Reverse(f.mark));
                if todo.is_empty() {
                    ui.label(RichText::new("Nothing to change.").size(TEXT_BODY));
                }
                for f in todo.iter().take(2) {
                    ui.add_space(2.0);
                    ui.label(RichText::new(format!("{}: {}", f.check.label(), f.seen)).size(TEXT_BODY).color(Color32::WHITE));
                    if let Some(fix) = &f.fix {
                        ui.add(egui::Label::new(RichText::new(format!("→ {fix}")).size(TEXT_BODY).color(mark_colour(f.mark))).wrap());
                    }
                }
                egui::CollapsingHeader::new(RichText::new("All six checks").size(TEXT_BODY)).id_salt(("camcheck_all", *u)).show(ui, |ui| {
                    for f in found {
                        ui.horizontal_top(|ui| {
                            dot(ui, f.mark);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(format!("{}: {}", f.check.label(), f.seen)).size(TEXT_BODY));
                                if let Some(fix) = &f.fix {
                                    ui.add(egui::Label::new(RichText::new(fix).size(TEXT_SMALL).color(TEXT_DIM)).wrap());
                                }
                            });
                        });
                    }
                    if let Some(s) = src {
                        let filled = s.camera.filled();
                        if !filled.is_empty() {
                            hint(ui, &format!("Compared with its remembered settings: {}.", filled.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Check again").on_hover_text("Read just this camera again, after changing it").clicked() {
                        again = Some(*u);
                    }
                    let names: Vec<String> = found.iter().flat_map(|f| f.new_settings.iter()).filter(|(k, _)| matches!(k, Setting::WhiteBalance | Setting::Iso)).map(|(k, v)| format!("{} {v}", if *k == Setting::Iso { "ISO" } else { "white balance" })).collect();
                    let tip = if names.is_empty() { "You've made the changes: remember this check for next time".to_string() } else { format!("You've made the changes: the remembered settings become {}", names.join(", ")) };
                    if !c.marked.contains(u) && worst >= Mark::Warn && ui.button("Mark as done").on_hover_text(tip).clicked() {
                        mark_done = Some(*u);
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(u) = mark_done {
            let found = all.iter().find(|(x, _)| *x == u).map(|(_, f)| f.clone()).unwrap_or_default();
            if let Some(s) = self.draft.sources.iter_mut().find(|s| s.uid == u) {
                for (k, v) in found.iter().flat_map(|f| f.new_settings.iter()) {
                    match k {
                        Setting::WhiteBalance => s.camera.white_balance = v.clone(),
                        Setting::Iso => s.camera.iso = v.clone(),
                        // Picture adjustments are set by hand on the source's page.
                        _ => {}
                    }
                }
            }
            c.marked.insert(u);
        }
        if let Some(u) = again {
            if let Some(i) = c.cams.iter().position(|&x| x == u) {
                c.only = Some(u);
                c.marked.remove(&u);
                c.readings.remove(&u);
                c.next(i);
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Check all again").clicked() {
                c.stage = Stage::Setup;
            }
            if ui.button("Done").on_hover_text("Close, and remember this check for next time").clicked() {
                self.remember_check(c);
                keep = false;
            }
        });
        keep
    }

    /// Saves what was measured with each camera, to compare next time.
    fn remember_check(&mut self, c: &CamCheck) {
        for (u, r) in &c.readings {
            if let (Some(s), Some(scene)) = (self.draft.sources.iter_mut().find(|s| s.uid == *u), r.scene) {
                s.camera.last_check = Some(crate::config::LastCheck { when: c.when.clone(), sharpness: scene.sharpness, noise: scene.noise });
            }
        }
    }
}

impl CamCheck {
    /// Reads camera `i` next (from its normal picture; the job starts in
    /// `camcheck`, where the pictures are).
    fn next(&mut self, i: usize) {
        self.pending = Some(i);
    }

    /// Camera `i` is read: on to the next, or the results.
    fn advance(&mut self, i: usize) {
        if self.only.is_none() && i + 1 < self.cams.len() {
            self.next(i + 1);
        } else {
            self.only = None;
            self.stage = Stage::Results;
            self.stamp();
        }
    }

    fn stamp(&mut self) {
        self.when = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    }
}
