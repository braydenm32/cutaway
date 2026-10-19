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

    pub(super) fn run_ptz(&mut self, _cam: Target, _cmd: PtzCmd) -> Result<String, String> {
        Err("PTZ isn't ready yet".into())
    }

    pub(super) fn run_replay(&mut self, _cmd: ReplayCmd) -> Result<String, String> {
        Err("replay isn't ready yet".into())
    }

    pub(super) fn run_prompter(&mut self, _cmd: PrompterCmd) -> Result<String, String> {
        Err("the prompter isn't ready yet".into())
    }

    pub(super) fn run_comment(&mut self, _cmd: CommentCmd) -> Result<String, String> {
        Err("comments aren't ready yet".into())
    }
}
