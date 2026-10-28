//! The first-show guide: a few steps from opening Cutaway to a first
//! recording, each with a button to the right page in Settings. It opens
//! by itself the first time Cutaway runs, and from the ? button after.

use super::theme::*;
use super::App;
use crate::settings::Section;
use eframe::egui::{self, RichText};

const CAMERA_PRIVACY: &str = "on Mac: System Settings → Privacy & Security → Camera; on Windows: Settings → Privacy & security → Camera";

enum Step {
    Done,
    Todo,
    /// Optional, and not set up.
    Skip,
}

impl App {
    pub(super) fn guide_window(&mut self, ctx: &egui::Context) {
        if !self.guide_open {
            return;
        }
        // What's set up, as it is now.
        let (cams_alive, sound_peak) = match &self.engine {
            Some(e) => (
                e.sources.iter().filter(|s| s.input.alive()).count(),
                e.audio.iter().flat_map(|a| a.shared.levels_db()).fold(-120.0f32, f32::max),
            ),
            None => (0, -120.0),
        };
        let cams = self.cfg.sources.len();
        let mics = self.cfg.audio.len();
        let folder = self.cfg.output.folder.clone();
        let free = crate::engine::health::disk_space(&folder).map(|(free, _)| free as f64 / 1e9);
        let dests = self.cfg.streaming.destinations.iter().filter(|d| d.enabled && !d.key.trim().is_empty()).count();
        if mics > 0 && sound_peak > -60.0 {
            self.guide_heard = true;
        }
        let heard = self.guide_heard;
        let recorded = self.engine.as_ref().is_some_and(|e| e.is_recording()) || self.guide_recorded;

        let mut open = true;
        let mut go: Option<Section> = None;
        let mut check = false;
        let mut done = false;
        egui::Window::new("Your first show")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label("Six steps to a first recording. Each button opens the right page in Settings; this stays open while you work.");
                ui.add_space(8.0);
                let mut n = 0;
                let mut step = |ui: &mut egui::Ui, state: Step, title: &str, detail: String, button: Option<&str>| -> bool {
                    n += 1;
                    let mut clicked = false;
                    ui.horizontal_top(|ui| {
                        let (sym, color) = match state {
                            Step::Done => ("✔", GREEN),
                            Step::Todo => ("○", ACCENT),
                            Step::Skip => ("–", TEXT_DIM),
                        };
                        ui.label(RichText::new(sym).color(color).strong().size(16.0));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("{n}. {title}")).strong());
                            ui.add(egui::Label::new(RichText::new(detail).color(TEXT_DIM)).wrap());
                            if let Some(b) = button {
                                clicked = ui.button(b).clicked();
                            }
                        });
                    });
                    ui.add_space(6.0);
                    clicked
                };

                let cam_state = if cams > 0 && cams_alive == cams { Step::Done } else { Step::Todo };
                let cam_text = match (cams, cams_alive) {
                    (0, _) => "No cameras yet. Plug one in, then add it here (a capture card, a USB camera or a phone).".to_string(),
                    (1, 1) => "1 camera with a picture. Name it so scenes and titles make sense.".to_string(),
                    (c, a) if a == c => format!("{c} cameras with a picture. Name them so scenes and titles make sense."),
                    (c, 0) => format!("{} Check the cable, that no other app is using {}, and that Cutaway may use the camera ({CAMERA_PRIVACY}).", if c == 1 { "The camera has no picture." } else { "None of the cameras has a picture." }, if c == 1 { "it" } else { "them" }),
                    (c, a) => format!("{a} of {c} cameras have a picture. Check the cable, or that no other app is using {}.", if c - a == 1 { "the one without" } else { "them" }),
                };
                if step(ui, cam_state, "Cameras", cam_text, Some("Cameras…")) {
                    go = Some(Section::Sources);
                }

                let mic_state = if mics > 0 && heard { Step::Done } else { Step::Todo };
                let mic_text = if mics == 0 {
                    "No microphone yet. Add one so the recording has sound.".to_string()
                } else if heard {
                    format!("{mics} {}, and Cutaway can hear you. Say a few words and watch the meters at the bottom.", if mics == 1 { "input" } else { "inputs" })
                } else {
                    format!("{mics} {}, but it's quiet. Say something, check the mic is unmuted, and that Cutaway may use it (on Mac: System Settings → Privacy & Security → Microphone; on Windows: Settings → Privacy & security → Microphone).", if mics == 1 { "input" } else { "inputs" })
                };
                if step(ui, mic_state, "Microphone", mic_text, Some("Sound…")) {
                    go = Some(Section::Audio);
                }

                let folder_text = match free {
                    Some(gb) if gb < 20.0 => format!("{}: only {gb:.0} GB free. An hour of 1080p takes a few GB per camera, so free some space or pick another drive.", folder.display()),
                    Some(gb) => format!("{} ({gb:.0} GB free). An hour of 1080p takes a few GB per camera.", folder.display()),
                    None => format!("{} (not there yet; it's made when you record).", folder.display()),
                };
                let folder_state = if free.is_some_and(|gb| gb < 20.0) { Step::Todo } else { Step::Done };
                if step(ui, folder_state, "Where recordings go", folder_text, Some("Recording…")) {
                    go = Some(Section::Recording);
                }

                let (live_state, live_text) = if dests > 0 {
                    (Step::Done, format!("{dests} {} ready.", if dests == 1 { "destination" } else { "destinations" }))
                } else {
                    (Step::Skip, "Optional: skip this if you only record. Otherwise add YouTube, Twitch or another service with its stream key.".to_string())
                };
                if step(ui, live_state, "Going live", live_text, Some("Streaming…")) {
                    go = Some(Section::Streaming);
                }

                if step(
                    ui,
                    if self.guide_checked { Step::Done } else { Step::Todo },
                    "Run the check",
                    "Tests every picture, the sound, the disk and the stream servers, and says what to fix.".into(),
                    Some("Run the check"),
                ) {
                    check = true;
                }

                step(
                    ui,
                    if recorded { Step::Done } else { Step::Todo },
                    "Record a test",
                    "Press Record (R), talk for ten seconds while switching cameras, then Stop. Watch the take back from its folder before the real show.".into(),
                    None,
                );

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        done = true;
                    }
                    ui.label(RichText::new("Open this again from ? at the top.").color(TEXT_DIM));
                });
            });
        if let Some(section) = go {
            self.settings = Some(self.new_settings().open_at(section));
        }
        if check {
            self.guide_checked = true;
            self.start_precheck(None);
        }
        if done || !open {
            self.guide_open = false;
        }
    }
}
