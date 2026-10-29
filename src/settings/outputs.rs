//! Settings → Screens & webcam: a full-screen output for another display,
//! and the virtual camera for video-call apps.

use crate::util::LockExt;
use super::*;
use crate::config::DisplayOutput;

impl SettingsUi {
    pub(super) fn outputs(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Screens & webcam", "The program on another screen, on the network as NDI, or as a camera in Zoom, Teams, Meet and other video calls.");
        let d = &mut self.draft.display;
        let state = match d.output {
            DisplayOutput::Off => "off".to_string(),
            DisplayOutput::Program => "the program".to_string(),
            DisplayOutput::Multiview => "the multiview".to_string(),
            DisplayOutput::Prompter => "the teleprompter".to_string(),
        };
        part(ui, "screen2", "Second screen", &state, true, |ui| {
            egui::Grid::new("display").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Show");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.output, DisplayOutput::Off, "Nothing");
                    ui.selectable_value(&mut d.output, DisplayOutput::Program, "The program");
                    ui.selectable_value(&mut d.output, DisplayOutput::Multiview, "The multiview");
                    ui.selectable_value(&mut d.output, DisplayOutput::Prompter, "The teleprompter");
                });
                ui.end_row();
                label(ui, "Full screen");
                ui.checkbox(&mut d.fullscreen, "fill the screen it's on");
                ui.end_row();
                label(ui, "Clocks");
                ui.checkbox(&mut d.clocks, "the time and the segment's time left");
                ui.end_row();
            });
            subtitle(ui, "Screen");
            let displays = crate::platform::displays::list_recent();
            if displays.len() < 2 {
                hint(ui, "Only one screen is connected, so it opens in a window over Cutaway (F fills the screen, Esc gets back out). Connect a projector, TV or monitor and it goes there.");
            } else {
                let ours = our_display(ui.ctx(), &displays);
                let chosen = displays.iter().position(|x| x.name == d.screen);
                let shown = chosen.or_else(|| crate::platform::displays::pick_output(&displays, "", ours));
                ui.horizontal(|ui| {
                    if ui.selectable_label(d.screen.is_empty(), "Automatic").on_hover_text("Another screen than the one Cutaway is on").clicked() {
                        d.screen.clear();
                    }
                    if !d.screen.is_empty() && chosen.is_none() {
                        ui.label(RichText::new(format!("{} isn't connected: using another screen for now", d.screen)).color(AMBER).size(TEXT_SMALL));
                    }
                });
                if let Some(i) = display_map(ui, &displays, shown, ours, 150.0) {
                    d.screen = displays[i].name.clone();
                }
                if shown.is_some() && shown == ours {
                    hint(ui, "That's the screen Cutaway is on, so it opens in a window there (F fills the screen, Esc gets back out).");
                }
            }
        });

        let n = &mut self.draft.ndi_out;
        let name = if n.name.trim().is_empty() { "Cutaway".to_string() } else { n.name.trim().to_string() };
        let state = match (n.program, n.sources, info.ndi_watchers) {
            (false, false, _) => "off".to_string(),
            (p, s, w) => format!(
                "{}{}",
                match (p, s) {
                    (true, true) => "the program and every source",
                    (true, false) => "the program",
                    _ => "every source",
                },
                w.map(|k| format!(" · {k} watching")).unwrap_or_default()
            ),
        };
        part(ui, "ndi_out", "NDI output", &state, n.program || n.sources, |ui| {
            egui::Grid::new("ndi_out_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Program");
                ui.checkbox(&mut n.program, "send it, with the recording mix");
                ui.end_row();
                label(ui, "Name");
                ui.add(egui::TextEdit::singleline(&mut n.name).desired_width(200.0));
                ui.end_row();
                label(ui, "Sources");
                ui.checkbox(&mut n.sources, format!("send every source too, as \"{name} - Cam 1\"…"));
                ui.end_row();
            });
            if n.program || n.sources {
                super::ndi_offer(ui);
            } else {
                super::ndi_credit(ui);
            }
        });

        if !crate::platform::vcam::available() {
            part(ui, "vcam", "Virtual camera", "needs macOS or Windows 11", false, |ui| {
                hint(ui, "The virtual camera needs macOS or Windows 11.");
            });
            return;
        }
        if let Some(r) = self.vcam_job.lock_ok().take() {
            self.vcam_busy = false;
            self.message = Some(match r {
                Ok(m) => (m, false),
                Err(e) => (e, true),
            });
        }
        let installed = crate::platform::vcam::installed();
        let state = if info.vcam_on {
            "on"
        } else if installed {
            "installed, off"
        } else {
            "not installed"
        };
        part(ui, "vcam", "Virtual camera", state, true, |ui| {
            ui.horizontal(|ui| {
                let (text, color) = if info.vcam_on {
                    ("On: video apps can choose the \"Cutaway\" camera", GREEN)
                } else if installed {
                    ("Installed, off", TEXT_DIM)
                } else {
                    ("Not installed", AMBER)
                };
                ui.label(RichText::new(text).color(color));
            });
            ui.horizontal(|ui| {
                if installed {
                    let label_text = if info.vcam_on { "Turn off" } else { "Turn on" };
                    if ui.button(label_text).clicked() {
                        self.pending = Some(SettingsAction::VirtualCamera(!info.vcam_on));
                    }
                }
                let busy = self.vcam_busy;
                if ui.add_enabled(!busy, egui::Button::new(if installed { "Reinstall" } else { "Install…" })).clicked() {
                    self.start_vcam_job(false);
                }
                if installed && ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                    self.start_vcam_job(true);
                }
                if busy {
                    ui.spinner();
                    hint(ui, if cfg!(windows) { "waiting for Windows (allow it when asked)…" } else { "waiting for macOS (approve it in System Settings if asked)…" });
                }
            });
            egui::Grid::new("vcam_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "At start");
                ui.checkbox(&mut self.draft.display.virtual_camera, "turn it on when Cutaway starts");
                ui.end_row();
            });
            hint(ui, "It shows the program (1920×1080, 30 fps); pick your interface or a loopback device as the call's microphone.");
            if cfg!(windows) {
                hint(ui, "Installing asks for administrator rights once (it goes in Program Files, where Windows' camera service can use it).");
            } else {
                hint(ui, "macOS only runs camera extensions from apps signed with an Apple Developer account. If Install says the camera isn't included, see \"Virtual camera\" in the README for the one-time signing step.");
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn start_vcam_job(&mut self, remove: bool) {
        self.vcam_busy = true;
        let out = self.vcam_job.clone();
        std::thread::spawn(move || {
            let r = crate::platform::vcam::install(remove).map_err(|e| format!("{e:#}"));
            *out.lock_ok() = Some(r);
        });
    }
}
