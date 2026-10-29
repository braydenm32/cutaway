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

        // Who's on camera: the video sources with a picture of their own (not a guest spot or a media player).
        let people: Vec<usize> = self.cfg.sources.iter().enumerate().filter(|(_, s)| s.guest_id().is_none() && !s.device.trim().eq_ignore_ascii_case("media")).map(|(i, _)| i).collect();
        let named = people.iter().filter(|&&i| !self.cfg.sources[i].person.trim().is_empty()).count();
        let timelines = self.cfg.output.export_timelines;
        let scenes_seen = self.guide_scenes || self.edit_mode;

        let mut open = true;
        let mut rename: Option<(usize, String)> = None;
        let mut people_changed = false;
        let mut edit_layout = false;
        let mut go_part: Option<(Section, &str)> = None;
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
                ui.label("A few steps to a first recording and its edit. Each button opens the right page in Settings; this stays open while you work.");
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 220.0).max(240.0)).auto_shrink([true, true]).show(ui, |ui| {
                let mut n = 0;
                let mut step = |ui: &mut egui::Ui, state: Step, title: &str, detail: String, button: Option<&str>, extra: Option<&mut dyn FnMut(&mut egui::Ui)>| -> bool {
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
                            if let Some(extra) = extra {
                                extra(ui);
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
                if step(ui, cam_state, "Cameras", cam_text, Some("Cameras…"), None) {
                    go = Some(Section::Sources);
                }

                if !people.is_empty() {
                    let people_state = if named == people.len() { Step::Done } else { Step::Todo };
                    let people_text = format!(
                        "Who is in each picture, for the name titles that follow whoever is on air ({named} of {} named). Set a close-up for each person if two share a camera. A new name for a camera reopens it for a moment.",
                        people.len()
                    );
                    let cfg = &mut self.cfg;
                    let mut fields = |ui: &mut egui::Ui| {
                        for &i in people.iter().take(8) {
                            // The source's name is changed once you leave the box, so the camera isn't reopened at every letter.
                            let id = ui.scope_id().with(("guide_name", cfg.sources[i].uid, i));
                            let mut name: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| cfg.sources[i].name.clone());
                            ui.horizontal(|ui| {
                                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0).hint_text("camera name"));
                                if r.lost_focus() && name.trim() != cfg.sources[i].name.trim() {
                                    rename = Some((i, name.trim().to_string()));
                                }
                                let s = &mut cfg.sources[i];
                                let a = ui.add(egui::TextEdit::singleline(&mut s.person).desired_width(130.0).hint_text("who's in it"));
                                let b = ui.add(egui::TextEdit::singleline(&mut s.role).desired_width(110.0).hint_text("role or handle"));
                                people_changed |= a.changed() || b.changed();
                            });
                            if name == cfg.sources[i].name {
                                ui.data_mut(|d| d.remove::<String>(id));
                            } else {
                                ui.data_mut(|d| d.insert_temp(id, name));
                            }
                        }
                        if people.len() > 8 {
                            ui.label(RichText::new(format!("and {} more in Settings → Video sources", people.len() - 8)).color(TEXT_DIM));
                        }
                    };
                    step(ui, people_state, "Name your people", people_text, None, Some(&mut fields));
                }

                let mic_state = if mics > 0 && heard { Step::Done } else { Step::Todo };
                let mic_text = if mics == 0 {
                    "No microphone yet. Add one so the recording has sound.".to_string()
                } else if heard {
                    format!("{mics} {}, and Cutaway can hear you. Say a few words and watch the meters at the bottom.", if mics == 1 { "input" } else { "inputs" })
                } else {
                    format!("{mics} {}, but it's quiet. Say something, check the mic is unmuted, and that Cutaway may use it (on Mac: System Settings → Privacy & Security → Microphone; on Windows: Settings → Privacy & security → Microphone).", if mics == 1 { "input" } else { "inputs" })
                };
                if step(ui, mic_state, "Microphone", mic_text, Some("Sound…"), None) {
                    go = Some(Section::Audio);
                }

                let scenes_text = "A scene is a layout: which cameras show and where (one full screen, side by side, picture in picture). The buttons along the bottom take them. Edit layout lets you drag the frames on the picture to change them.".to_string();
                let scenes_state = if scenes_seen { Step::Done } else { Step::Todo };
                if step(ui, scenes_state, "Your scenes", scenes_text, Some("Edit layout"), None) {
                    edit_layout = true;
                }

                let folder_text = match free {
                    Some(gb) if gb < 20.0 => format!("{}: only {gb:.0} GB free. An hour of 1080p takes a few GB per camera, so free some space or pick another drive.", folder.display()),
                    Some(gb) => format!("{} ({gb:.0} GB free). An hour of 1080p takes a few GB per camera.", folder.display()),
                    None => format!("{} (not there yet; it's made when you record).", folder.display()),
                };
                let folder_state = if free.is_some_and(|gb| gb < 20.0) { Step::Todo } else { Step::Done };
                if step(ui, folder_state, "Where recordings go", folder_text, Some("Recording…"), None) {
                    go = Some(Section::Recording);
                }

                let (live_state, live_text) = if dests > 0 {
                    (Step::Done, format!("{dests} {} ready.", if dests == 1 { "destination" } else { "destinations" }))
                } else {
                    (Step::Skip, "Optional: skip this if you only record. Otherwise add YouTube, Twitch or another service with its stream key.".to_string())
                };
                if step(ui, live_state, "Going live", live_text, Some("Streaming…"), None) {
                    go = Some(Section::Streaming);
                }

                if step(
                    ui,
                    if self.guide_checked { Step::Done } else { Step::Todo },
                    "Run the check",
                    "Tests every picture, the sound, the disk and the stream servers, and says what to fix.".into(),
                    Some("Run the check"),
                    None,
                ) {
                    check = true;
                }

                step(
                    ui,
                    if recorded { Step::Done } else { Step::Todo },
                    "Record a test",
                    "Press Record (R), talk for ten seconds while switching cameras, then Stop. Watch the take back from its folder before the real show.".into(),
                    None,
                    None,
                );

                let edit_state = if timelines { Step::Done } else { Step::Skip };
                let edit_text = if timelines {
                    "On: every take also saves a project for DaVinci Resolve, Premiere Pro and Final Cut, with each camera on its own track, your live cuts as the first edit, and your markers."
                } else {
                    "Optional: Cutaway can save a project for DaVinci Resolve, Premiere Pro or Final Cut with every take: each camera on its own track, your live cuts as the first edit, and your markers. Turn it on under Recording → For editing."
                };
                if step(ui, edit_state, "Editing afterwards", edit_text.into(), Some("For editing…"), None) {
                    go_part = Some((Section::Recording, "rec_editing"));
                }
                });

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
        if let Some((section, part)) = go_part {
            self.settings = Some(self.new_settings().open_part_at(section, part));
        }
        if edit_layout {
            self.edit_mode = true;
            self.guide_scenes = true;
        }
        if let Some((i, new)) = rename {
            let mut cfg = self.cfg.clone();
            cfg.scenes = self.scenes.clone();
            match cfg.rename_source(i, &new) {
                Ok(()) => {
                    if let Err(e) = self.apply_settings(cfg, ctx) {
                        self.toast(e, true);
                    }
                }
                Err(e) => self.toast(e, true),
            }
        } else if people_changed {
            // Who's in it changes live: nothing reopens.
            self.cfg_dirty = true;
            if let Some(e) = &self.engine {
                let _ = e.update_config(&self.cfg);
            }
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
