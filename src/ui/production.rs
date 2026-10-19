//! The production extras in the main window: macros, PTZ cameras, instant
//! replay, the teleprompter and viewer comments.

use super::theme::*;
use super::App;
use crate::control::{Action, CommentCmd, PrompterCmd, PtzCmd, ReplayCmd, Target};
use crate::settings::Section;
use eframe::egui::{self, vec2, Color32, RichText};
use std::time::{Duration, Instant};

fn color_of(s: &str, fallback: u32) -> Color32 {
    let c = crate::config::parse_color(s).unwrap_or(fallback);
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Macros inside macros go this deep at most.
const MAX_DEPTH: usize = 4;

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
    matches!(a, Action::Scene(_) | Action::CutTo(_) | Action::Preview(_) | Action::Take | Action::Cut | Action::Source(_) | Action::Rundown(_))
}

impl App {
    /// Run a list of commands in order; "wait" pauses the rest. `cue`:
    /// they're a scene's or segment's cues (which don't change scenes).
    pub(super) fn run_sequence(&mut self, from: &str, commands: &[String], cue: bool) {
        let mut at = Instant::now();
        let mut flat = Vec::new();
        self.expand_macros(commands, 0, &mut flat);
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

    /// Macros named in a list, replaced by their commands.
    fn expand_macros(&self, commands: &[String], depth: usize, out: &mut Vec<String>) {
        let names: Vec<String> = self.cfg.macros.iter().map(|m| m.name.clone()).collect();
        for c in commands {
            match Action::parse(c) {
                Ok(Action::Macro(t)) if depth < MAX_DEPTH => match t.find(&names) {
                    Some(i) => self.expand_macros(&self.cfg.macros[i].commands.clone(), depth + 1, out),
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
        self.section_heading(ui, "MACROS", Section::Macros);
        let running: Vec<String> = self.queued.iter().map(|q| q.from.clone()).collect();
        let mut run = None;
        ui.horizontal_wrapped(|ui| {
            for (i, m) in self.cfg.macros.iter().enumerate() {
                let busy = running.contains(&m.name);
                let fill = color_of(&m.color, 0x6a4fc9);
                let text = RichText::new(if busy { format!("{} …", m.name) } else { m.name.clone() }).strong().size(12.5).color(Color32::WHITE);
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
        self.run_sequence(&mac.name, &mac.commands, false);
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
        self.section_heading(ui, "PTZ", Section::Sources);
        let live = self.engine.as_ref().map(|e| e.switcher.live().sources()).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            for (i, name) in &cams {
                let on_air = live.contains(i);
                let text = RichText::new(name).size(12.5).color(if on_air { RED } else { Color32::WHITE });
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
                if ui.add(egui::Button::new("⌂ Home").min_size(vec2(96.0, 24.0))).clicked() {
                    send.push(PtzCmd::Home);
                }
            });
        });
        // Presets: click to go; right-click to save the current shot there.
        let presets: Vec<String> = self.cfg.sources.get(cam).map(|s| s.ptz.presets.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            for (k, name) in presets.iter().enumerate() {
                let r = ui.add(egui::Button::new(RichText::new(name).size(12.5)).min_size(vec2(56.0, 26.0))).on_hover_text("Go there. Right-click to save the current shot here.");
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
            if ui.small_button("＋").on_hover_text("Save the current shot as a new preset").clicked() {
                send.push(PtzCmd::Store(Target::Name(format!("Preset {}", presets.len() + 1))));
            }
        });
        if !status.is_empty() && status != "virtual" {
            ui.label(RichText::new(&status).color(TEXT_DIM).size(11.0));
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
            ReplayCmd::Play { seconds, slow } => {
                let d = e.start_replay(seconds, slow).map_err(|e| format!("{e:#}"))?;
                Ok(format!("Replay{} ({:.0} s)", if slow { " in slow motion" } else { "" }, d.as_secs_f32()))
            }
        }
    }

    pub(super) fn run_prompter(&mut self, _cmd: PrompterCmd) -> Result<String, String> {
        Err("the prompter isn't ready yet".into())
    }

    pub(super) fn run_comment(&mut self, _cmd: CommentCmd) -> Result<String, String> {
        Err("comments aren't ready yet".into())
    }
}
