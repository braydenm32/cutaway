//! Settings → Screens & webcam: a full-screen output for another display,
//! and the virtual camera for video-call apps.

use super::*;
use crate::config::DisplayOutput;

impl SettingsUi {
    pub(super) fn outputs(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Screens & webcam", "Show the program on another screen, send it to other computers as NDI, or use it as a camera in Zoom, Teams, Meet, FaceTime and others.");
        let d = &mut self.draft.display;
        subtitle(ui, "Second screen");
        egui::Grid::new("display").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
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
            ui.checkbox(&mut d.clocks, "show the time and the rundown segment's time left (only on this screen)");
            ui.end_row();
        });
        hint(ui, "A window opens with the picture. Drag it to the projector, TV or confidence monitor, then double-click it (or press F) to fill that screen; Esc leaves full screen. The header's Screen button turns it on and off.");

        ui.add_space(16.0);
        subtitle(ui, "NDI output");
        let n = &mut self.draft.ndi_out;
        egui::Grid::new("ndi_out").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Program");
            ui.checkbox(&mut n.program, "send the program, with the recording mix");
            ui.end_row();
            label(ui, "Name");
            ui.add(egui::TextEdit::singleline(&mut n.name).desired_width(200.0));
            ui.end_row();
            label(ui, "Sources");
            ui.checkbox(&mut n.sources, &format!("send every source too, as \"{} - Cam 1\"…", if n.name.trim().is_empty() { "Cutaway" } else { n.name.trim() }));
            ui.end_row();
        });
        match info.ndi_watchers {
            Some(k) => hint(ui, &format!("On the network now; {k} receiver{} watching.", if k == 1 { "" } else { "s" })),
            None => hint(ui, "Other computers, OBS, vMix, NDI Studio Monitor and NDI recorders see it on the same network (NDI adds this computer's name)."),
        }
        if n.program || n.sources {
            super::ndi_offer(ui);
        } else {
            super::ndi_credit(ui);
        }

        ui.add_space(16.0);
        subtitle(ui, "Virtual camera");
        if !crate::platform::vcam::available() {
            hint(ui, "The virtual camera needs macOS or Windows 11.");
            return;
        }
        if let Some(r) = self.vcam_job.lock().unwrap().take() {
            self.vcam_busy = false;
            self.message = Some(match r {
                Ok(m) => (m, false),
                Err(e) => (e, true),
            });
        }
        let installed = crate::platform::vcam::installed();
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
        ui.checkbox(&mut self.draft.display.virtual_camera, "Turn the virtual camera on when Cutaway starts");
        if cfg!(windows) {
            hint(ui, "The camera shows the program (1920×1080, 30 fps) with sound kept separate: pick your interface or a loopback device as the microphone in the call. The \"Cutaway\" camera is there while Webcam is on. Installing asks for administrator rights once (it's put in Program Files, where Windows' camera service can use it).");
        } else {
            hint(ui, "The camera shows the program (1920×1080, 30 fps) with sound kept separate: pick your interface or a loopback device as the microphone in the call. When Cutaway isn't running, the camera shows a dark card.");
            hint(ui, "macOS only runs camera extensions from apps signed with an Apple Developer account. If Install says the camera isn't included, see \"Virtual camera\" in the README for the one-time signing step.");
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn start_vcam_job(&mut self, remove: bool) {
        self.vcam_busy = true;
        let out = self.vcam_job.clone();
        std::thread::spawn(move || {
            let r = crate::platform::vcam::install(remove).map_err(|e| format!("{e:#}"));
            *out.lock().unwrap() = Some(r);
        });
    }
}
