//! The first-show guide: a few steps from opening Cutaway to a first
//! recording, each with a button to the right page. It opens by itself
//! the first time Cutaway runs, and from More › App › Help or the status
//! light after. A step's button folds it down to a small button on the
//! rail above More, so the page it went to can be used; its progress (and
//! whether it was open) is remembered on this computer between runs.

use super::theme::*;
use super::App;
use crate::settings::Section;
use eframe::egui::{self, RichText};

/// Less free than this on the recordings drive: the guide's folder step
/// asks for more room.
const GUIDE_LOW_BYTES: u64 = 20_000_000_000;

const CAMERA_PRIVACY: &str = "on Mac: System Settings → Privacy & Security → Camera; on Windows: Windows Settings → Privacy & security → Camera";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Done,
    Todo,
    /// Optional, and not set up.
    Skip,
}

/// What's remembered between runs (in "Main window.ron", beside the
/// window's size), forensics guide #4.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Saved {
    open: bool,
    heard: u64,
    checked: bool,
    scenes: bool,
    recorded: bool,
}

const SAVE_KEY: &str = "first_show_guide";

impl App {
    /// The guide's progress from last time; says whether it was open.
    pub(super) fn load_guide(&mut self, storage: Option<&dyn eframe::Storage>) -> bool {
        let saved: Saved = storage.and_then(|s| eframe::get_value(s, SAVE_KEY)).unwrap_or_default();
        self.guide_heard = saved.heard;
        self.guide_checked = saved.checked;
        self.guide_scenes = saved.scenes;
        self.guide_recorded = saved.recorded;
        saved.open
    }

    pub(super) fn save_guide(&self, storage: &mut dyn eframe::Storage) {
        let saved = Saved { open: self.guide_open, heard: self.guide_heard, checked: self.guide_checked, scenes: self.guide_scenes, recorded: self.guide_recorded };
        eframe::set_value(storage, SAVE_KEY, &saved);
    }

    /// Open the guide (More › App › Help, the status light), unfolded.
    pub(super) fn open_guide(&mut self) {
        self.guide_open = true;
        self.guide_folded = false;
    }

    pub(super) fn guide_window(&mut self, ctx: &egui::Context) {
        if !self.guide_open {
            return;
        }
        // The key as it's set now (numbers audit #22).
        let record_key = self.key_hint(crate::control::shortcuts::Builtin::Record);
        // What's set up, as it is now.
        // Cameras: not test pictures (they stand in when none is found:
        // guide #3), close-ups cut from a camera, media players or guests.
        let is_cam = |s: &crate::config::SourceConfig| {
            let d = s.device.trim().to_lowercase();
            !(d == "test" || d.starts_with("test:") || d == "media" || d.starts_with("vptz:") || d.starts_with("frame:") || s.guest_id().is_some())
        };
        let cam_list: Vec<usize> = self.cfg.sources.iter().enumerate().filter(|(_, s)| is_cam(s)).map(|(i, _)| i).collect();
        let tests = self.cfg.sources.iter().filter(|s| s.device.trim().to_lowercase().starts_with("test")).count();
        // The mic: the input you speak into (not pads, an app's or a
        // source's sound). The step checks that one hears you, not just
        // any input (forensics guide #2).
        let mic = guide_mic(&self.cfg.audio);
        let (cams_alive, levels) = match &self.engine {
            Some(e) => (cam_list.iter().filter(|&&i| e.sources.get(i).is_some_and(|s| s.input.alive())).count(), e.input_levels()),
            None => (0, Vec::new()),
        };
        let loud = |i: usize| levels.get(i).copied().flatten().is_some_and(|db| db > -60.0);
        let cams = cam_list.len();
        let mics = self.cfg.audio.len();
        let mic_name = mic.map(|i| self.cfg.audio[i].name.clone()).unwrap_or_default();
        let mic_uid = mic.map(|i| self.cfg.audio[i].uid).unwrap_or(0);
        // Sound on another input while the mic is quiet: maybe the wrong one.
        let elsewhere = (0..self.cfg.audio.len()).find(|&i| Some(i) != mic && loud(i) && !self.cfg.audio[i].is_pads()).map(|i| self.cfg.audio[i].name.clone());
        let folder = self.cfg.output.folder.clone();
        let free = crate::engine::health::disk_space(&folder).map(|(free, _)| free);
        let dests = self.cfg.streaming.destinations.iter().filter(|d| d.enabled && !d.key.trim().is_empty()).count();
        if let Some(i) = mic.filter(|&i| loud(i)) {
            self.guide_heard = self.cfg.audio[i].uid;
        }
        let heard = mic_uid != 0 && self.guide_heard == mic_uid;
        let recorded = self.engine.as_ref().is_some_and(|e| e.is_recording()) || self.guide_recorded;

        // Who's on camera: as Sources › People & names lists them.
        let people: Vec<usize> = self.people_sources();
        let named = people.iter().filter(|&&i| !self.cfg.sources[i].person.trim().is_empty()).count();
        let name_title = self.has_name_title();
        let timelines = self.cfg.output.export_timelines;
        let scenes_seen = self.guide_scenes || self.edit_mode;
        self.guide_scenes = scenes_seen;

        // Each step's state, for the steps and the folded button's count.
        let cam_state = if cams > 0 && cams_alive == cams { Step::Done } else { Step::Todo };
        let people_state = (!people.is_empty()).then_some(if named == people.len() && name_title { Step::Done } else { Step::Todo });
        let mic_state = if mics > 0 && heard { Step::Done } else { Step::Todo };
        let scenes_state = if scenes_seen { Step::Done } else { Step::Todo };
        let folder_state = if free.is_some_and(|b| b < GUIDE_LOW_BYTES) { Step::Todo } else { Step::Done };
        let live_state = if dests > 0 { Step::Done } else { Step::Skip };
        let check_state = if self.guide_checked { Step::Done } else { Step::Todo };
        let rec_state = if recorded { Step::Done } else { Step::Todo };
        let edit_state = if timelines { Step::Done } else { Step::Skip };
        let states = [Some(cam_state), people_state, Some(mic_state), Some(scenes_state), Some(folder_state), Some(live_state), Some(check_state), Some(rec_state), Some(edit_state)];
        let counted = states.iter().flatten().filter(|s| **s != Step::Skip).count();
        let done_n = states.iter().flatten().filter(|s| **s == Step::Done).count();

        if self.guide_folded {
            // Folded: a small button on the rail, just above More, where it
            // never covers a page or the desk.
            let rail = App::rail_width(ctx);
            let wide = rail >= super::shell::RAIL_WIDE;
            let more_h = if wide { 54.0 } else { 44.0 } + 8.0;
            let mut unfold = false;
            egui::Area::new(egui::Id::unique("guide_folded"))
                .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(4.0, -(more_h + 8.0)))
                .show(ctx, |ui| {
                    let text = if wide { format!("Guide\n{done_n} of {counted}") } else { format!("{done_n}/{counted}") };
                    let b = egui::Button::new(RichText::new(text).size(TEXT_SMALL).color(egui::Color32::WHITE))
                        .fill(RAISED)
                        .stroke(egui::Stroke::new(1.0, ACCENT))
                        .min_size(egui::vec2(rail - 8.0, 38.0));
                    if ui.add(b).on_hover_text(format!("Your first show: {done_n} of {counted} steps done. Click to carry on")).clicked() {
                        unfold = true;
                    }
                });
            if unfold {
                self.guide_folded = false;
            }
            return;
        }

        let mut open = true;
        let mut edit_layout = false;
        let mut go_place: Option<&'static str> = None;
        let mut make_title = false;
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
                ui.label(format!("A few steps to a first recording and its edit ({done_n} of {counted} done). Each button opens the right page and folds this down to a Guide button on the left, above More, to come back to."));
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
                        ui.label(RichText::new(sym).color(color).strong().size(TEXT_HEADING));
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

                let cam_text = match (cams, cams_alive) {
                    (0, _) if tests > 0 => format!("No camera found yet: test pictures stand in for now. Plug one in and add it here (a capture card, a USB camera or a phone). If it's plugged in, check that no other app is using it and that Cutaway may use the camera ({CAMERA_PRIVACY})."),
                    (0, _) => "No cameras yet. Plug one in, then add it here (a capture card, a USB camera or a phone).".to_string(),
                    (1, 1) => "1 camera with a picture. Name it so scenes and titles make sense.".to_string(),
                    (c, a) if a == c => format!("{c} cameras with a picture. Name them so scenes and titles make sense."),
                    (c, 0) => format!("{} Check the cable, that no other app is using {}, and that Cutaway may use the camera ({CAMERA_PRIVACY}).", if c == 1 { "The camera has no picture." } else { "None of the cameras has a picture." }, if c == 1 { "it" } else { "them" }),
                    (c, a) => format!("{a} of {c} cameras have a picture. Check the cable, or that no other app is using {}.", if c - a == 1 { "the one without" } else { "them" }),
                };
                if step(ui, cam_state, "Cameras", cam_text, Some("Cameras…"), None) {
                    go_place = Some("sources/cameras/list");
                }

                if let Some(people_state) = people_state {
                    let people_text = format!(
                        "Who is in each picture, for the name titles that follow whoever is on air ({named} of {} named{}). Saved as you type; a new camera name restarts that camera for a moment. Two people on one camera? Give each a close-up.",
                        people.len(),
                        if name_title { "" } else { "; there's no name title yet" }
                    );
                    // The same rows as Sources › People & names (forensics settings #4).
                    let mut rows = |ui: &mut egui::Ui| {
                        self.people_rows(ui, 8, false);
                        ui.horizontal(|ui| {
                            if !name_title && ui.button("＋ Make a name title").on_hover_text("Adds a title that names whoever is on air (Undo takes it away)").clicked() {
                                make_title = true;
                            }
                            if ui.button("Close-ups…").on_hover_text("A close-up of each person from one camera").clicked() {
                                go_place = Some("sources/closeups/split");
                            }
                            if ui.button("People & names…").clicked() {
                                go_place = Some("sources/people/who");
                            }
                        });
                    };
                    step(ui, people_state, "Name your people", people_text, None, Some(&mut rows));
                }

                let mic_text = if mics == 0 || mic.is_none() {
                    "No microphone yet. Add one so the recording has sound.".to_string()
                } else if heard {
                    format!("Listening to {mic_name}, and Cutaway can hear you. Say a few words and watch its meter at the bottom.")
                } else if let Some(other) = &elsewhere {
                    format!("Listening to {mic_name}, but it's quiet, while {other} hears sound. If {other} is the mic you speak into, pick it in Sound › Inputs.")
                } else {
                    format!("Listening to {mic_name}, but it's quiet. Say something, check the mic is unmuted, and that Cutaway may use it (on Mac: System Settings → Privacy & Security → Microphone; on Windows: Windows Settings → Privacy & security → Microphone). Not the mic you speak into? Pick another in Sound › Inputs.")
                };
                if step(ui, mic_state, "Microphone", mic_text, Some("Sound…"), None) {
                    go = Some(Section::Audio);
                }

                let scenes_text = "A scene is a layout: which cameras show and where (one full screen, side by side, picture in picture). The buttons along the bottom take them. Show › Edit layout (or E) lets you drag the frames on the picture to change them.".to_string();
                if step(ui, scenes_state, "Your scenes", scenes_text, Some("Edit layout"), None) {
                    edit_layout = true;
                }

                let folder_text = match free {
                    Some(b) if b < GUIDE_LOW_BYTES => format!("{}: only {} free. An hour of 1080p takes a few GB per camera, so free some space or pick another drive.", folder.display(), crate::util::human_size(b)),
                    Some(b) => format!("{} ({} free). An hour of 1080p takes a few GB per camera.", folder.display(), crate::util::human_size(b)),
                    None => format!("{} (not there yet; it's made when you record).", folder.display()),
                };
                if step(ui, folder_state, "Where recordings go", folder_text, Some("Recording…"), None) {
                    go = Some(Section::Recording);
                }

                let live_text = if dests > 0 {
                    format!("{dests} {} ready.", if dests == 1 { "destination" } else { "destinations" })
                } else {
                    "Optional: skip this if you only record. Otherwise add YouTube, Twitch or another service with its stream key.".to_string()
                };
                if step(ui, live_state, "Going live", live_text, Some("Streaming…"), None) {
                    go = Some(Section::Streaming);
                }

                if step(
                    ui,
                    check_state,
                    "Run the check",
                    "Tests every picture, the sound, the disk and the stream servers, and says what to fix.".into(),
                    Some("Run the check"),
                    None,
                ) {
                    check = true;
                }

                step(
                    ui,
                    rec_state,
                    "Record a test",
                    format!("Press Record{}, talk for ten seconds while switching cameras, then hold REC to stop. Watch the take back from its folder before the real show.", record_key),
                    None,
                    None,
                );

                let edit_text = if timelines {
                    "On: every take also saves a project for DaVinci Resolve, Premiere Pro and Final Cut, with each camera on its own track, your live cuts as the first edit, and your markers."
                } else {
                    "Optional: Cutaway can save a project for DaVinci Resolve, Premiere Pro or Final Cut with every take: each camera on its own track, your live cuts as the first edit, and your markers. Turn it on in Record › Recording › Editing."
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
                    ui.label(RichText::new("Open this again from More › App › Help, or the status light at the top right.").color(TEXT_DIM));
                });
            });
        if let Some(section) = go {
            self.open_page(section, None);
        }
        if let Some(place) = go_place {
            self.go(place);
        }
        if make_title {
            self.make_name_title(ctx);
        }
        if let Some((section, part)) = go_part {
            self.open_page(section, Some(part));
        }
        if edit_layout {
            self.edit_mode = true;
            self.guide_scenes = true;
        }
        if check {
            self.guide_checked = true;
            self.start_precheck(None);
        }
        // Gone to a page: out of its way, a click from coming back.
        if go.is_some() || go_place.is_some() || go_part.is_some() || edit_layout || check {
            self.guide_folded = true;
        }
        if done || !open {
            self.guide_open = false;
            self.guide_folded = false;
        }
    }
}

/// The input the guide's mic step checks: the first that isn't the sound
/// pads, an app's sound or a source's sound (forensics guide #2).
fn guide_mic(audio: &[crate::config::AudioConfig]) -> Option<usize> {
    audio.iter().position(|a| {
        let d = a.device.trim().to_lowercase();
        !(a.is_pads() || d.starts_with("app:") || d.starts_with("source:"))
    })
}

#[cfg(test)]
mod mic_tests {
    use super::guide_mic;
    use crate::config::AudioConfig;

    #[test]
    fn the_mic_step_checks_the_mic() {
        let a = |name: &str, device: &str| AudioConfig { name: name.into(), device: device.into(), ..Default::default() };
        assert_eq!(guide_mic(&[a("Sound pads", "pads"), a("Music", "app:Spotify"), a("Yeti", "Yeti Stereo Microphone")]), Some(2));
        assert_eq!(guide_mic(&[a("Sound pads", "pads")]), None);
        assert_eq!(guide_mic(&[]), None);
    }
}
