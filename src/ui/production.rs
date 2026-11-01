//! The production extras in the main window: macros, PTZ cameras, instant
//! replay, the teleprompter and viewer comments.

use crate::util::LockExt;
use super::theme::*;
use super::App;
use crate::control::{Action, CommentCmd, PrompterCmd, PtzCmd, ReplayAngle, ReplayCmd, Target};
use crate::settings::Section;
use eframe::egui::{self, vec2, Color32, Rect, RichText};
use std::time::{Duration, Instant};

fn color_of(s: &str, fallback: u32) -> Color32 {
    let c = crate::config::parse_color(s).unwrap_or(fallback);
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Macros inside macros go this deep at most.
const MAX_DEPTH: usize = 4;
/// How long Undo stays beside PTZ Home.
const PTZ_UNDO_FOR: Duration = Duration::from_secs(20);

/// A command waiting for its time (after a "wait" in a macro or cues).
pub(super) struct Queued {
    pub at: Instant,
    pub command: String,
    /// What it came from, for messages.
    pub from: String,
    /// Cues don't take scenes; macros do.
    pub cue: bool,
}

/// Commands that change the scene (not run as scene or rundown cues).
fn takes_a_scene(a: &Action) -> bool {
    matches!(a, Action::Scene(_) | Action::CutTo(_) | Action::Preview(_) | Action::Take | Action::Cut | Action::Back | Action::Source(_) | Action::Rundown(_))
}

impl App {
    /// Run a list of commands in order; "wait" pauses the rest. `cue`:
    /// they're a scene's or segment's cues (which don't change scenes).
    pub(super) fn run_sequence(&mut self, from: &str, commands: &[String], cue: bool) {
        self.run_sequence_in(from, commands, cue, Vec::new());
    }

    /// The same, for the steps of the macros in `inside`.
    fn run_sequence_in(&mut self, from: &str, commands: &[String], cue: bool, mut inside: Vec<usize>) {
        let mut at = Instant::now();
        let mut flat = Vec::new();
        let mut loops = Vec::new();
        self.expand_macros(commands, &mut inside, &mut flat, &mut loops);
        if let Some(name) = loops.first() {
            self.toast(format!("{from}: the macro {name} runs itself, so that step was skipped"), true);
        }
        for c in flat {
            match Action::parse(&c) {
                Ok(Action::Wait(s)) => at += Duration::from_secs_f32(s.max(0.0)),
                Ok(a) if cue && takes_a_scene(&a) => {}
                Ok(a) => {
                    if at <= Instant::now() {
                        if let Err(e) = self.run_action(a) {
                            self.toast(format!("{from}: \"{c}\": {e}"), true);
                        }
                    } else {
                        self.queued.push(Queued { at, command: c, from: from.to_string(), cue });
                    }
                }
                Err(e) => self.toast(format!("{from}: \"{c}\": {e}"), true),
            }
        }
    }

    /// Macros named in a list, replaced by their commands. `inside`: the
    /// macros being expanded; one that would run itself (or nest too deep)
    /// is left out and named in `loops`, so it can't run without end.
    fn expand_macros(&self, commands: &[String], inside: &mut Vec<usize>, out: &mut Vec<String>, loops: &mut Vec<String>) {
        let names: Vec<String> = self.cfg.macros.iter().map(|m| m.name.clone()).collect();
        for c in commands {
            match Action::parse(c) {
                Ok(Action::Macro(t)) => match t.find(&names) {
                    Some(i) if inside.contains(&i) || inside.len() >= MAX_DEPTH => loops.push(names[i].clone()),
                    Some(i) => {
                        inside.push(i);
                        self.expand_macros(&self.cfg.macros[i].commands, inside, out, loops);
                        inside.pop();
                    }
                    None => out.push(c.clone()),
                },
                _ => out.push(c.clone()),
            }
        }
    }

    /// Run queued commands whose time has come.
    pub(super) fn run_queued(&mut self) {
        if self.queued.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut due: Vec<Queued> = Vec::new();
        let mut i = 0;
        while i < self.queued.len() {
            if self.queued[i].at <= now {
                due.push(self.queued.remove(i));
            } else {
                i += 1;
            }
        }
        due.sort_by_key(|q| q.at);
        for q in due {
            match Action::parse(&q.command) {
                Ok(a) if !(q.cue && takes_a_scene(&a)) => {
                    if let Err(e) = self.run_action(a) {
                        self.toast(format!("{}: \"{}\": {e}", q.from, q.command), true);
                    }
                }
                _ => {}
            }
        }
    }

    /// When the next queued command is due (to wake up for it).
    pub(super) fn next_queued(&self) -> Option<Duration> {
        self.queued.iter().map(|q| q.at.saturating_duration_since(Instant::now())).min()
    }

    /// Macro buttons.
    pub(super) fn macros_panel(&mut self, ui: &mut egui::Ui) {
        if self.cfg.macros.is_empty() {
            return;
        }
        ui.add_space(4.0);
        if !self.section_heading(ui, "macros", "MACROS", Section::Macros) {
            return;
        }
        let running: Vec<String> = self.queued.iter().map(|q| q.from.clone()).collect();
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            for (i, m) in self.cfg.macros.iter().enumerate() {
                let busy = running.contains(&m.name);
                let fill = color_of(&m.color, 0x6a4fc9);
                let text = RichText::new(if busy { format!("{} …", m.name) } else { m.name.clone() }).strong().size(TEXT_BODY).color(Color32::WHITE);
                let tip = if m.commands.is_empty() { "No steps yet (Settings → Macros)".to_string() } else { m.commands.join("\n") };
                if ui.add(egui::Button::new(text).fill(if busy { fill } else { fill.gamma_multiply(0.75) }).min_size(vec2(64.0, 28.0))).on_hover_text(tip).clicked() {
                    run = Some(i);
                }
            }
        });
        if let Some(i) = run {
            self.act(Action::Macro(Target::Index(i)));
        }
        ui.add_space(4.0);
        ui.separator();
    }

    pub(super) fn run_macro(&mut self, m: Target) -> Result<String, String> {
        let names: Vec<String> = self.cfg.macros.iter().map(|m| m.name.clone()).collect();
        let i = m.find(&names).ok_or("there's no such macro")?;
        let mac = self.cfg.macros[i].clone();
        if mac.commands.is_empty() {
            return Err(format!("{} has no commands yet (Settings → Macros)", mac.name));
        }
        self.run_sequence_in(&mac.name, &mac.commands, false, vec![i]);
        Ok(format!("Running {}", mac.name))
    }

    pub(super) fn run_ptz(&mut self, cam: Target, cmd: PtzCmd) -> Result<String, String> {
        use crate::engine::ptz::Move;
        let names: Vec<String> = self.cfg.sources.iter().map(|s| s.name.clone()).collect();
        let i = cam.find(&names).ok_or("there's no such camera")?;
        let name = names[i].clone();
        let e = self.engine.as_ref().ok_or("the engine isn't running")?;
        let p = e.ptz(i).ok_or_else(|| format!("{name} has no PTZ controls (Settings → Video sources)"))?;
        let virt = p.virt.clone();
        let presets: Vec<String> = self.cfg.sources[i].ptz.presets.iter().map(|p| p.name.clone()).collect();
        match cmd {
            PtzCmd::Drive { pan, tilt } => p.send(Move::Drive { pan, tilt }),
            PtzCmd::Zoom(z) => p.send(Move::Zoom(z)),
            PtzCmd::Stop => p.send(Move::Stop),
            PtzCmd::Home => p.send(Move::Home),
            PtzCmd::Recall(t) => {
                // Real cameras: any memory number; virtual ones: saved presets.
                let n = match (&t, t.find(&presets)) {
                    (_, Some(n)) => n,
                    (Target::Index(n), None) if virt.is_none() => *n,
                    _ => return Err(format!("{name} has no preset {t}")),
                };
                p.send(Move::Recall(n));
                let label = presets.get(n).cloned().unwrap_or_else(|| format!("preset {}", n + 1));
                return Ok(format!("{name}: {label}"));
            }
            PtzCmd::Store(t) => {
                let n = match (&t, t.find(&presets)) {
                    (_, Some(n)) => n,
                    (Target::Index(n), None) => *n,
                    (Target::Name(_), None) => presets.len(),
                };
                p.send(Move::Store(n));
                // Remember it (a virtual camera's position, a real one's name).
                let list = &mut self.cfg.sources[i].ptz.presets;
                while list.len() <= n {
                    let k = list.len();
                    list.push(crate::config::PtzPreset { name: format!("Preset {}", k + 1), ..Default::default() });
                }
                if let Target::Name(new) = &t {
                    list[n].name = new.clone();
                }
                if let Some(v) = &virt {
                    let pos = v.position();
                    list[n].x = pos.x;
                    list[n].y = pos.y;
                    list[n].zoom = pos.zoom;
                }
                let label = list[n].name.clone();
                self.save_config_quietly();
                return Ok(format!("{name}: saved {label}"));
            }
        }
        Ok("OK".into())
    }

    /// The PTZ panel: pick a camera, steer it, zoom, and its presets.
    pub(super) fn ptz_panel(&mut self, ui: &mut egui::Ui) {
        let cams: Vec<(usize, String)> = self.cfg.sources.iter().enumerate().filter(|(_, s)| s.has_ptz()).map(|(i, s)| (i, s.name.clone())).collect();
        if cams.is_empty() || self.engine.is_none() {
            return;
        }
        if !cams.iter().any(|(i, _)| *i == self.ptz_cam) {
            self.ptz_cam = cams[0].0;
        }
        ui.add_space(4.0);
        if !self.section_heading(ui, "ptz", "PTZ", Section::Sources) {
            return;
        }
        let live = self.engine.as_ref().map(|e| e.switcher.live().sources()).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            for (i, name) in &cams {
                let on_air = live.contains(i);
                let text = RichText::new(name).size(TEXT_BODY).color(if on_air { ON_AIR } else { Color32::WHITE });
                if ui.add(egui::Button::selectable(self.ptz_cam == *i, text)).on_hover_text(if on_air { "On air" } else { "" }).clicked() {
                    self.ptz_cam = *i;
                }
            }
        });
        let cam = self.ptz_cam;
        let target = Target::Index(cam);
        let status = self.engine.as_ref().and_then(|e| e.ptz(cam)).map(|p| p.status()).unwrap_or_default();
        let mut send: Vec<PtzCmd> = Vec::new();
        ui.horizontal(|ui| {
            // Joystick: drag from the middle; let go to stop.
            let size = 96.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::drag());
            let painter = ui.painter_at(rect);
            painter.circle_filled(rect.center(), size / 2.0, RAISED);
            painter.circle_stroke(rect.center(), size / 2.0, egui::Stroke::new(1.0, LINE));
            for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
                painter.circle_filled(rect.center() + vec2(dx, dy) * (size / 2.0 - 9.0), 2.5, TEXT_DIM);
            }
            let mut knob = rect.center();
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let d = (pos - rect.center()) / (size / 2.0);
                    let d = if d.length() > 1.0 { d / d.length() } else { d };
                    knob = rect.center() + d * (size / 2.0);
                    // A small dead zone, then speed grows with distance.
                    let shape = |v: f32| if v.abs() < 0.12 { 0.0 } else { v.signum() * ((v.abs() - 0.12) / 0.88).powf(1.5) };
                    let want = [shape(d.x), shape(-d.y)];
                    let last = self.ptz_drive.unwrap_or([0.0, 0.0]);
                    if (want[0] - last[0]).abs() > 0.04 || (want[1] - last[1]).abs() > 0.04 || self.ptz_drive.is_none() {
                        self.ptz_drive = Some(want);
                        send.push(PtzCmd::Drive { pan: want[0], tilt: want[1] });
                    }
                }
            } else if self.ptz_drive.take().is_some() {
                send.push(PtzCmd::Stop);
            }
            painter.circle_filled(knob, 13.0, if resp.dragged() { ACCENT } else { Color32::from_gray(160) });
            resp.on_hover_text("Drag to pan and tilt; let go to stop");
            ui.vertical(|ui| {
                // Zoom: hold.
                for (label, z) in [("＋ Zoom in", 1.0f32), ("－ Zoom out", -1.0)] {
                    let r = ui.add(egui::Button::new(label).min_size(vec2(96.0, 28.0)).sense(egui::Sense::click_and_drag()));
                    let down = r.is_pointer_button_down_on();
                    if down && self.ptz_zoom != Some(z) {
                        self.ptz_zoom = Some(z);
                        send.push(PtzCmd::Zoom(z));
                    } else if !down && self.ptz_zoom == Some(z) {
                        self.ptz_zoom = None;
                        send.push(PtzCmd::Stop);
                    }
                }
            });
        });
        // Presets: click to go. "Save here" (or a preset's right-click) keeps
        // the current shot in one.
        let presets: Vec<String> = self.cfg.sources.get(cam).map(|s| s.ptz.presets.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            for (k, name) in presets.iter().enumerate() {
                let r = ui.add(egui::Button::new(RichText::new(name).size(TEXT_BODY)).min_size(vec2(56.0, 26.0))).on_hover_text("Go there. Right-click to save the current shot here.");
                if r.clicked() {
                    send.push(PtzCmd::Recall(Target::Index(k)));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("Save the current shot as \"{name}\"")).clicked() {
                        send.push(PtzCmd::Store(Target::Index(k)));
                        ui.close();
                    }
                });
            }
            ui.menu_button(RichText::new("Save here ▾").size(TEXT_BODY), |ui| {
                ui.label(RichText::new("Keep the shot it has now as:").size(TEXT_SMALL).color(TEXT_DIM));
                for (k, name) in presets.iter().enumerate() {
                    if ui.button(format!("\"{name}\" (replaces it)")).clicked() {
                        send.push(PtzCmd::Store(Target::Index(k)));
                        ui.close();
                    }
                }
                if ui.button("A new preset").clicked() {
                    send.push(PtzCmd::Store(Target::Name(format!("Preset {}", presets.len() + 1))));
                    ui.close();
                }
            })
            .response
            .on_hover_text("Save the current shot as a preset");
        });
        // Home: away from Zoom out. While this camera is on air it takes a
        // one-second hold, and a close-up made from a bigger picture can go
        // back with Undo.
        let on_air = live.contains(&cam);
        let virt = self.engine.as_ref().and_then(|e| e.ptz(cam)).and_then(|p| p.virt.clone());
        let mut home = false;
        ui.horizontal(|ui| {
            if !status.is_empty() && status != "virtual" {
                ui.label(RichText::new(&status).color(TEXT_DIM).size(TEXT_SMALL));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.add(egui::Button::new(RichText::new(if on_air { "⌂ Home (hold)" } else { "⌂ Home" }).size(TEXT_BODY)).min_size(vec2(84.0, 24.0)).sense(egui::Sense::click_and_drag()));
                if on_air {
                    match (r.is_pointer_button_down_on(), self.ptz_home_hold) {
                        (true, None) => self.ptz_home_hold = Some((Instant::now(), false)),
                        (true, Some((since, false))) => {
                            let f = (since.elapsed().as_secs_f32() / super::REC_HOLD_SECS).min(1.0);
                            let fill = Rect::from_min_size(r.rect.min, vec2(r.rect.width() * f, r.rect.height()));
                            ui.painter().with_clip_rect(fill).rect_filled(r.rect, 4.0, ACCENT.gamma_multiply(0.35));
                            if f >= 1.0 {
                                home = true;
                                self.ptz_home_hold = Some((since, true));
                            }
                            ui.ctx().request_repaint();
                        }
                        (true, Some(_)) => {}
                        (false, held) => {
                            if held.is_some_and(|(_, spent)| !spent) {
                                self.toast(format!("{} is on air: hold ⌂ Home for a second to go to the whole picture", self.cfg.sources.get(cam).map(|s| s.name.as_str()).unwrap_or("This camera")), false);
                            }
                            self.ptz_home_hold = None;
                        }
                    }
                    r.on_hover_text("On air now: hold for a second to go to the whole picture");
                } else {
                    self.ptz_home_hold = None;
                    if r.on_hover_text("Go to the whole picture (home)").clicked() {
                        home = true;
                    }
                }
                let undo = self.ptz_home_undo.filter(|(c, _, at)| *c == cam && at.elapsed() < PTZ_UNDO_FOR);
                if let (Some((_, shot, _)), Some(v)) = (undo, &virt) {
                    if ui.button(RichText::new("↶ Undo Home").size(TEXT_BODY)).on_hover_text("Glide back to the shot it had before Home").clicked() {
                        v.go_to(shot);
                        self.ptz_home_undo = None;
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                }
            });
        });
        if home {
            if let Some(v) = &virt {
                self.ptz_home_undo = Some((cam, v.position(), Instant::now()));
            }
            send.push(PtzCmd::Home);
        }
        if self.ptz_drive.is_some() || self.ptz_zoom.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        for c in send {
            match self.run_ptz(target.clone(), c) {
                Ok(m) if m != "OK" => self.toast(m, false),
                Ok(_) => {}
                Err(e) => self.toast(e, true),
            }
        }
        ui.add_space(4.0);
        ui.separator();
    }

    pub(super) fn run_replay(&mut self, cmd: ReplayCmd) -> Result<String, String> {
        let e = self.engine.as_ref().ok_or("the engine isn't running")?;
        match cmd {
            ReplayCmd::Stop => {
                e.replay.stop();
                Ok("Replay stopped".into())
            }
            ReplayCmd::Play { seconds, slow, angle } => {
                // A plain replay shows the angle chosen in the settings.
                let angle = angle.unwrap_or_else(|| ReplayAngle::parse(&self.cfg.replay.angle));
                let d = e.start_replay(seconds, slow, &angle).map_err(|e| format!("{e:#}"))?;
                let from = match &angle {
                    ReplayAngle::Program => String::new(),
                    ReplayAngle::Source(t) => format!(" from {t}"),
                    ReplayAngle::Quad => " from every angle".into(),
                };
                Ok(format!("Replay{from}{} ({:.0} s)", if slow { " in slow motion" } else { "" }, d.as_secs_f32()))
            }
        }
    }

    pub(super) fn run_comment(&mut self, cmd: CommentCmd) -> Result<String, String> {
        match cmd {
            CommentCmd::Hide => {
                self.hide_comment();
                Ok("Comment hidden".into())
            }
            CommentCmd::Toggle if self.comment_on.is_some() => {
                self.hide_comment();
                Ok("Comment hidden".into())
            }
            CommentCmd::Next | CommentCmd::Toggle => {
                let chat = self.chat.as_ref().ok_or("no live chat is set up (Settings → Viewer comments)")?;
                let next = chat.messages().into_iter().find(|m| m.id > self.comment_cursor).ok_or("no new comments yet")?;
                self.show_comment(&next);
                Ok(format!("Showing {}'s comment", next.author))
            }
        }
    }

    fn show_comment(&mut self, m: &crate::net::chat::ChatMessage) {
        let Some(e) = &self.engine else { return };
        let c = crate::engine::graphics::Comment { author: m.author.clone(), text: m.text.clone(), color: m.color.clone().unwrap_or_else(|| m.platform.color().to_string()) };
        e.graphics.set_comment(Some(&c), &self.cfg.comments);
        self.comment_on = Some(m.id);
        self.comment_at = Some(Instant::now());
        self.comment_cursor = self.comment_cursor.max(m.id);
    }

    fn hide_comment(&mut self) {
        if let Some(e) = &self.engine {
            e.graphics.set_comment(None, &self.cfg.comments);
        }
        self.comment_on = None;
        self.comment_at = None;
    }

    /// Start (or restart) reading the live chat when its settings change.
    pub(super) fn apply_chat(&mut self) {
        let c = &self.cfg.comments;
        let wanted = (c.twitch_channel.clone(), c.youtube.clone(), c.youtube_api_key.clone());
        let same = self.chat.as_ref().is_some_and(|ch| ch.made_for == wanted);
        if !c.enabled() {
            self.chat = None;
        } else if !same {
            self.chat = Some(crate::net::chat::Chat::start(c, self.waker.clone()));
        }
    }

    /// Hide a comment that's been up long enough.
    pub(super) fn tick_comments(&mut self, ctx: &egui::Context) {
        let after = self.cfg.comments.hide_after_s;
        if let (Some(at), true) = (self.comment_at, after > 0) {
            let left = Duration::from_secs(after as u64).saturating_sub(at.elapsed());
            if left.is_zero() {
                self.hide_comment();
            } else {
                ctx.request_repaint_after(left);
            }
        }
    }

    /// The live chat: click a comment to show it on screen.
    pub(super) fn comments_panel(&mut self, ui: &mut egui::Ui) {
        let Some(chat) = &self.chat else { return };
        let messages = chat.messages();
        let (tw, yt) = chat.status();
        ui.add_space(4.0);
        if !self.section_heading(ui, "comments", "COMMENTS", Section::Comments) {
            return;
        }
        for (p, st) in [("Twitch", tw), ("YouTube", yt)] {
            if !st.is_empty() {
                let ok = st.starts_with("reading");
                ui.label(RichText::new(format!("{p}: {st}")).color(if ok { TEXT_DIM } else { AMBER }).size(TEXT_SMALL));
            }
        }
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("comments").max_height(170.0).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if messages.is_empty() {
                ui.label(RichText::new("No comments yet").color(TEXT_DIM).size(TEXT_BODY));
            }
            for m in messages.iter().rev().take(40).rev() {
                let showing = self.comment_on == Some(m.id);
                let color = color_of(m.color.as_deref().unwrap_or(m.platform.color()), 0x9b6bff);
                let mut job = egui::text::LayoutJob::default();
                job.append(&m.author, 0.0, egui::TextFormat { color, font_id: egui::FontId::proportional(TEXT_BODY), ..Default::default() });
                job.append(&format!("  {}", m.text), 0.0, egui::TextFormat { color: if showing { Color32::BLACK } else { Color32::from_gray(225) }, font_id: egui::FontId::proportional(TEXT_BODY), ..Default::default() });
                job.wrap.max_width = ui.available_width() - 8.0;
                let fill = if showing { AMBER } else if m.id > self.comment_cursor { RAISED } else { Color32::TRANSPARENT };
                let r = egui::Frame::new().fill(fill).corner_radius(4.0).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(job).sense(egui::Sense::click()))
                });
                let tip = format!("{} · {}", m.platform.name(), if showing { "showing: click to hide" } else { "click to show it on screen" });
                if r.inner.on_hover_text(tip).clicked() {
                    pick = Some(if showing { None } else { Some(m.clone()) });
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Show next").on_hover_text("The oldest comment not shown yet").clicked() {
                self.act(Action::Comment(CommentCmd::Next));
            }
            if self.comment_on.is_some() && ui.button("Hide").clicked() {
                self.hide_comment();
            }
        });
        match pick {
            Some(Some(m)) => self.show_comment(&m),
            Some(None) => self.hide_comment(),
            None => {}
        }
        ui.add_space(4.0);
        ui.separator();
    }
}

// ---- Teleprompter ---------------------------------------------------------------

/// Where the teleprompter is in its script.
pub(super) struct Prompter {
    pub playing: bool,
    /// Characters into the script.
    pub pos: f64,
    pub wpm: f32,
    pub last: Instant,
    /// The script, where each rundown segment starts in it, and a version.
    pub script: std::sync::Arc<String>,
    pub starts: Vec<usize>,
    pub version: u64,
    /// The script's length in characters.
    pub chars: usize,
    /// The settings the script and speed were last taken from.
    pub made_from: Option<(crate::config::PrompterConfig, crate::config::RundownConfig)>,
}

impl Default for Prompter {
    fn default() -> Self {
        Prompter { playing: false, pos: 0.0, wpm: 140.0, last: Instant::now(), script: Default::default(), starts: Vec::new(), version: 0, chars: 0, made_from: None }
    }
}

/// Characters a second at a reading speed (about six to a word, with the space).
pub(super) fn chars_per_second(wpm: f32) -> f64 {
    wpm.clamp(20.0, 400.0) as f64 * 6.0 / 60.0
}

/// The script: the text written, or the rundown's segments and notes.
/// Returns it and where each segment starts.
pub(super) fn build_script(cfg: &crate::config::Config) -> (String, Vec<usize>) {
    let p = &cfg.prompter;
    if !p.use_rundown || cfg.rundown.segments.is_empty() {
        return (p.script.clone(), Vec::new());
    }
    let mut s = String::new();
    let mut starts = Vec::new();
    let mut chars = 0;
    for seg in &cfg.rundown.segments {
        starts.push(chars);
        let at = s.len();
        s.push_str(&format!("▶ {}\n", seg.name.to_uppercase()));
        let notes = seg.notes.trim();
        if !notes.is_empty() {
            s.push_str(notes);
            s.push('\n');
        }
        s.push('\n');
        chars += s[at..].chars().count();
    }
    if !p.script.trim().is_empty() {
        s.push_str(p.script.trim());
        s.push('\n');
    }
    (s, starts)
}

/// Where paragraphs start (after a blank line), in characters.
fn paragraph_starts(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    let chars: Vec<char> = text.chars().collect();
    for i in 1..chars.len() {
        if chars[i] != '\n' && chars[i - 1] == '\n' && i >= 2 && chars[i - 2] == '\n' {
            out.push(i);
        }
    }
    out
}

impl App {
    /// Keep the script up to date and move it along.
    pub(super) fn tick_prompter(&mut self, ctx: &egui::Context) {
        // Rebuilt only when its settings change.
        let fresh = self.prompter.made_from.as_ref().is_none_or(|(p, r)| *p != self.cfg.prompter || *r != self.cfg.rundown);
        if fresh {
            if self.prompter.made_from.as_ref().is_some_and(|(p, _)| p.words_per_minute != self.cfg.prompter.words_per_minute) {
                // A new speed in Settings takes over from the desk's.
                self.prompter.wpm = self.cfg.prompter.words_per_minute;
            }
            self.prompter.made_from = Some((self.cfg.prompter.clone(), self.cfg.rundown.clone()));
            let (script, starts) = build_script(&self.cfg);
            if *self.prompter.script != script {
                self.prompter.chars = script.chars().count();
                self.prompter.script = std::sync::Arc::new(script);
                self.prompter.starts = starts;
                self.prompter.version += 1;
                *self.remote_shared.prompter.lock_ok() = serde_json::json!({ "version": self.prompter.version, "text": *self.prompter.script, "segments": self.prompter.starts }).to_string();
            }
        }
        let now = Instant::now();
        let dt = now.duration_since(self.prompter.last).as_secs_f64().min(0.5);
        self.prompter.last = now;
        if self.prompter.playing {
            let total = self.prompter.chars as f64;
            self.prompter.pos = (self.prompter.pos + chars_per_second(self.prompter.wpm) * dt).min(total);
            if self.prompter.pos >= total {
                self.prompter.playing = false;
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    /// The rundown moved to a segment: the prompter follows.
    pub(super) fn prompter_to_segment(&mut self, i: usize) {
        if self.cfg.prompter.use_rundown {
            let (_, starts) = build_script(&self.cfg);
            if let Some(&s) = starts.get(i) {
                self.prompter.pos = s as f64;
            }
        }
    }

    pub(super) fn run_prompter(&mut self, cmd: PrompterCmd) -> Result<String, String> {
        if self.prompter.script.trim().is_empty() {
            return Err("the teleprompter has no script yet (Settings → Teleprompter)".into());
        }
        let p = &mut self.prompter;
        let marks = {
            let mut m = paragraph_starts(&p.script);
            m.extend(p.starts.iter().copied());
            m.sort_unstable();
            m.dedup();
            m
        };
        match cmd {
            PrompterCmd::Play => p.playing = true,
            PrompterCmd::Pause => p.playing = false,
            PrompterCmd::Toggle => p.playing = !p.playing,
            PrompterCmd::Faster => p.wpm = (p.wpm + 10.0).min(400.0),
            PrompterCmd::Slower => p.wpm = (p.wpm - 10.0).max(20.0),
            PrompterCmd::Top => {
                p.pos = 0.0;
                p.playing = false;
            }
            PrompterCmd::Back => {
                // The start of this paragraph, or the one before if just past it.
                let cur = p.pos as usize;
                p.pos = marks.iter().rev().find(|&&m| m + 3 < cur).copied().unwrap_or(0) as f64;
            }
            PrompterCmd::Forward => {
                let cur = p.pos as usize;
                if let Some(&m) = marks.iter().find(|&&m| m > cur) {
                    p.pos = m as f64;
                }
            }
        }
        p.last = Instant::now();
        Ok(format!("Prompter {} · {:.0} words a minute", if p.playing { "rolling" } else { "paused" }, p.wpm))
    }

    /// Prompter controls in the main window.
    pub(super) fn prompter_panel(&mut self, ui: &mut egui::Ui) {
        if self.prompter.script.trim().is_empty() {
            return;
        }
        ui.add_space(4.0);
        if !self.section_heading(ui, "prompter", "PROMPTER", Section::Prompter) {
            return;
        }
        let total = self.prompter.chars.max(1) as f64;
        let mut cmd = None;
        ui.horizontal(|ui| {
            let playing = self.prompter.playing;
            if ui.add(egui::Button::new(RichText::new(if playing { "⏸" } else { "▶" }).size(TEXT_STATE)).min_size(vec2(36.0, 28.0))).on_hover_text("Roll / pause").clicked() {
                cmd = Some(PrompterCmd::Toggle);
            }
            if ui.button("⏮").on_hover_text("Back to the top").clicked() {
                cmd = Some(PrompterCmd::Top);
            }
            if ui.button("◀").on_hover_text("Back a paragraph").clicked() {
                cmd = Some(PrompterCmd::Back);
            }
            if ui.button("▶▶").on_hover_text("Forward a paragraph").clicked() {
                cmd = Some(PrompterCmd::Forward);
            }
            ui.separator();
            if ui.button("−").on_hover_text("Slower").clicked() {
                cmd = Some(PrompterCmd::Slower);
            }
            ui.label(RichText::new(format!("{:.0} wpm", self.prompter.wpm)).monospace());
            if ui.button("＋").on_hover_text("Faster").clicked() {
                cmd = Some(PrompterCmd::Faster);
            }
        });
        let mut frac = (self.prompter.pos / total) as f32;
        let r = ui.add(egui::Slider::new(&mut frac, 0.0..=1.0).show_value(false));
        if r.changed() {
            self.prompter.pos = frac as f64 * total;
        }
        if let Some(c) = cmd {
            if let Err(e) = self.run_prompter(c) {
                self.toast(e, true);
            }
        }
        ui.add_space(4.0);
        ui.separator();
    }
}

/// Draw the teleprompter into `area`: the script, the line being read a
/// third of the way down, mirrored for prompter glass if asked.
pub(super) fn draw_prompter(ui: &egui::Ui, area: egui::Rect, cfg: &crate::config::PrompterConfig, text: &str, pos: f64) {
    let painter = ui.painter_at(area);
    let bg = color_of(&cfg.background, 0x000000);
    let fg = color_of(&cfg.text_color, 0xffffff);
    painter.rect_filled(area, 0.0, bg);
    if text.trim().is_empty() {
        painter.text(area.center(), egui::Align2::CENTER_CENTER, "No script (Settings → Teleprompter)", egui::FontId::proportional(TEXT_TITLE), Color32::from_gray(120));
        return;
    }
    let size = (area.height() * cfg.text_size.clamp(0.02, 0.2)).max(10.0);
    let margin = area.width() * 0.08;
    let wrap = area.width() - margin * 2.0;
    let galley = painter.layout(text.to_string(), egui::FontId::proportional(size), fg, wrap);
    // Where the reading position is in the laid-out text.
    let n = text.chars().count();
    let i = (pos.floor() as usize).min(n);
    let frac = (pos - pos.floor()) as f32;
    let here = galley.pos_from_cursor(egui::text::CCursor::new(i));
    let next = galley.pos_from_cursor(egui::text::CCursor::new((i + 1).min(n)));
    let y = here.top() + (next.top() - here.top()) * frac;
    let reading = area.top() + area.height() * 0.3;
    let origin = egui::pos2(area.left() + margin, reading - y);
    if cfg.mirror {
        // Tessellate, then flip left to right.
        let shape = egui::Shape::galley(origin, galley, fg);
        let prims = ui.ctx().tessellate(vec![egui::epaint::ClippedShape { clip_rect: area, shape, transform_after_tessellation: egui::emath::TSTransform::IDENTITY }], ui.ctx().pixels_per_point());
        let cx = area.center().x;
        for p in prims {
            if let egui::epaint::Primitive::Mesh(mut mesh) = p.primitive {
                for v in &mut mesh.vertices {
                    v.pos.x = 2.0 * cx - v.pos.x;
                }
                // Flipping turns the triangles over; draw them anyway.
                painter.add(egui::Shape::mesh(mesh));
            }
        }
    } else {
        painter.galley(origin, galley, fg);
    }
    // A marker at the reading line, and shading above it.
    painter.rect_filled(egui::Rect::from_min_max(area.min, egui::pos2(area.right(), reading - size * 0.9)), 0.0, bg.gamma_multiply(0.55));
    let arrow_x = if cfg.mirror { area.right() - margin * 0.5 } else { area.left() + margin * 0.5 };
    let dir = if cfg.mirror { -1.0 } else { 1.0 };
    let tip = egui::pos2(arrow_x + dir * size * 0.35, reading + size * 0.55);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, egui::pos2(arrow_x - dir * size * 0.2, reading + size * 0.2), egui::pos2(arrow_x - dir * size * 0.2, reading + size * 0.9)],
        AMBER,
        egui::Stroke::NONE,
    ));
}
