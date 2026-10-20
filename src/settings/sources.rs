//! Settings → Video sources: cameras, capture cards, the screen, test
//! pictures, media players and network inputs, plus each source's crop,
//! colour and keying.

use super::*;
use crate::config::{FramingConfig, FramingFollow, FramingShot, MediaConfig, SourceLook};
use crate::engine::netsrc::{self, NetSpec};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Device,
    Screen,
    Test,
    Media,
    SrtListen,
    SrtCall,
    Rtmp,
    Ndi,
    Framed,
    VirtualPtz,
    Phone,
    /// Made in Remote guests (with its audio input), so not offered here.
    Guest,
}

impl SourceKind {
    const ALL: [SourceKind; 11] = [
        SourceKind::Device,
        SourceKind::Screen,
        SourceKind::Test,
        SourceKind::Media,
        SourceKind::SrtListen,
        SourceKind::SrtCall,
        SourceKind::Rtmp,
        SourceKind::Ndi,
        SourceKind::Framed,
        SourceKind::VirtualPtz,
        SourceKind::Phone,
    ];

    fn of(device: &str) -> SourceKind {
        let d = device.trim().to_lowercase();
        if d == "screen" {
            SourceKind::Screen
        } else if d == "test" || d.starts_with("test:") {
            SourceKind::Test
        } else if d == "media" {
            SourceKind::Media
        } else if d.starts_with("ndi:") {
            SourceKind::Ndi
        } else if d.starts_with("frame:") {
            SourceKind::Framed
        } else if d.starts_with("vptz:") {
            SourceKind::VirtualPtz
        } else if d.starts_with("phone:") {
            SourceKind::Phone
        } else if d.starts_with("guest:") {
            SourceKind::Guest
        } else {
            match netsrc::parse_spec(device) {
                Some(NetSpec::SrtListen { .. }) => SourceKind::SrtListen,
                Some(NetSpec::SrtCall { .. }) => SourceKind::SrtCall,
                Some(NetSpec::RtmpListen { .. }) => SourceKind::Rtmp,
                None => SourceKind::Device,
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            SourceKind::Device => "Camera or capture card",
            SourceKind::Screen => "Screen",
            SourceKind::Test => "Test picture",
            SourceKind::Media => "Media player (video files, images)",
            SourceKind::SrtListen => "SRT input (wait for a sender)",
            SourceKind::SrtCall => "SRT input (pull from an address)",
            SourceKind::Rtmp => "RTMP input (OBS, phones, encoders)",
            SourceKind::Ndi => "NDI",
            SourceKind::Framed => "Auto-framed close-up (from another source)",
            SourceKind::VirtualPtz => "Virtual PTZ camera (pan, tilt and zoom in another source)",
            SourceKind::Phone => "Phone camera (scan a QR code)",
            SourceKind::Guest => "Remote guest (set up in Remote guests)",
        }
    }

    fn default_device(self, devices: &[DeviceInfo]) -> String {
        match self {
            SourceKind::Device => devices.first().map(|d| d.name.clone()).unwrap_or_else(|| "#1".into()),
            SourceKind::Screen => "screen".into(),
            SourceKind::Test => "test:smpte".into(),
            SourceKind::Media => "media".into(),
            SourceKind::SrtListen => "srt-in:9000".into(),
            SourceKind::SrtCall => "srt://192.168.1.10:9000".into(),
            SourceKind::Rtmp => format!("rtmp-in:1935/{}", random_key()),
            SourceKind::Ndi => "ndi:".into(),
            SourceKind::Framed => "frame:".into(),
            SourceKind::VirtualPtz => "vptz:".into(),
            SourceKind::Phone => format!("phone:{}", crate::engine::guests::new_id()),
            SourceKind::Guest => format!("guest:{}", crate::engine::guests::new_id()),
        }
    }
}

fn random_key() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() ^ (d.as_secs() as u32).wrapping_mul(2654435761)).unwrap_or(1);
    format!("cam{:05}", n % 100_000)
}

impl SettingsUi {
    pub(super) fn sources(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Video sources", "Cameras, capture cards, the screen, media and network feeds. Each shows in the multiview and can record to its own file.");
        ui.horizontal(|ui| {
            if ui.button("Rescan devices").clicked() {
                self.video = devices::list(Kind::Video);
            }
            hint(ui, &format!("{} camera(s) / capture card(s) found", self.video.len()));
        });
        ui.add_space(6.0);
        let n = self.draft.sources.len();
        if self.sel_source >= n {
            self.sel_source = n.saturating_sub(1);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for i in 0..n {
                    let s = &self.draft.sources[i];
                    let text = format!("{}  {}", i + 1, s.name);
                    let sub = short_device(&s.device);
                    let b = egui::Button::selectable(self.sel_source == i, RichText::new(format!("{text}\n{sub}")).size(13.0)).min_size(vec2(220.0, 40.0));
                    if ui.add(b).clicked() {
                        self.sel_source = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("＋ Add").clicked() {
                        let used: Vec<String> = self.draft.sources.iter().map(|s| s.device.clone()).collect();
                        let device = self.video.iter().find(|d| !used.contains(&d.name)).map(|d| d.name.clone()).unwrap_or("test".into());
                        let name = unique(self.draft.sources.iter().map(|s| s.name.clone()).collect(), "Cam");
                        self.draft.sources.push(SourceConfig { name, device, ..Default::default() });
                        self.sel_source = self.draft.sources.len() - 1;
                    }
                    let has = n > 0;
                    if ui.add_enabled(has, egui::Button::new("Duplicate")).clicked() {
                        let mut s = self.draft.sources[self.sel_source].clone();
                        s.name = unique(self.draft.sources.iter().map(|s| s.name.clone()).collect(), &s.name);
                        self.draft.sources.insert(self.sel_source + 1, s);
                        self.sel_source += 1;
                    }
                    if ui.add_enabled(has, egui::Button::new("Remove")).clicked() {
                        self.draft.sources.remove(self.sel_source);
                    }
                    if ui.add_enabled(has && self.sel_source > 0, egui::Button::new("↑")).clicked() {
                        self.draft.sources.swap(self.sel_source, self.sel_source - 1);
                        self.sel_source -= 1;
                    }
                    if ui.add_enabled(has && self.sel_source + 1 < n, egui::Button::new("↓")).clicked() {
                        self.draft.sources.swap(self.sel_source, self.sel_source + 1);
                        self.sel_source += 1;
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| self.source_detail(ui, info));
        });
    }

    fn source_detail(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let applied: Vec<String> = self.original.sources.iter().map(|s| s.device.trim().to_string()).collect();
        let mut guests_on = false;
        let ndi = self.ndi_found.lock().unwrap().clone();
        // Sources a close-up can be cut from: (name, fps).
        let wides: Vec<(String, u32)> = self
            .draft
            .sources
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != self.sel_source && s.framed_from().is_none() && s.virtual_ptz_of().is_none())
            .map(|(_, s)| (s.name.clone(), s.fps))
            .collect();
        let Some(src) = self.draft.sources.get_mut(self.sel_source) else {
            hint(ui, "No sources yet. Add your cameras, capture cards, the screen, a media player or a network input.");
            return;
        };
        let devices = &self.video;
        let mut scan_ndi = false;
        let kind = SourceKind::of(&src.device);
        let old_name = src.name.clone();
        egui::Grid::new("source_detail").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Name");
            ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut src.name));
            ui.end_row();

            label(ui, "Type");
            egui::ComboBox::from_id_salt("source_kind").selected_text(kind.label()).width(300.0).show_ui(ui, |ui| {
                for k in SourceKind::ALL {
                    if ui.selectable_label(k == kind, k.label()).clicked() && k != kind {
                        src.device = k.default_device(devices);
                        if k == SourceKind::Framed {
                            src.device = format!("frame:{}", wides.first().map(|w| w.0.as_str()).unwrap_or(""));
                            src.framing = FramingConfig::default();
                        }
                        if k == SourceKind::VirtualPtz {
                            src.device = format!("vptz:{}", wides.first().map(|w| w.0.as_str()).unwrap_or(""));
                            src.ptz = crate::config::PtzConfig::default();
                        }
                        if k == SourceKind::Media || matches!(k, SourceKind::SrtListen | SourceKind::SrtCall | SourceKind::Rtmp | SourceKind::Ndi | SourceKind::Phone) {
                            src.record = k != SourceKind::Media;
                        }
                        if k == SourceKind::Phone {
                            let p = super::guests::phone_source(&src.name);
                            (src.device, src.width, src.height, src.fps, src.network_latency_ms) = (p.device, p.width, p.height, p.fps, p.network_latency_ms);
                            guests_on = true;
                        }
                    }
                }
            });
            ui.end_row();

            match kind {
                SourceKind::Device => {
                    label(ui, "Device");
                    video_device_combo(ui, &mut src.device, devices);
                    ui.end_row();
                    let dev = devices.iter().find(|d| d.name == src.device);
                    let modes = dev.map(|d| d.modes.clone()).unwrap_or_default();
                    if !modes.is_empty() {
                        label(ui, "Device modes");
                        mode_combo(ui, &modes, src);
                        ui.end_row();
                    }
                }
                SourceKind::Test => {
                    label(ui, "Pattern");
                    let current = src.device.strip_prefix("test:").unwrap_or("smpte").to_string();
                    egui::ComboBox::from_id_salt("pattern").selected_text(&current).show_ui(ui, |ui| {
                        for p in TEST_PATTERN_NAMES {
                            if ui.selectable_label(current == *p, *p).clicked() {
                                src.device = format!("test:{p}");
                            }
                        }
                    });
                    ui.end_row();
                }
                SourceKind::SrtListen => {
                    label(ui, "Listen on port");
                    let mut port = match netsrc::parse_spec(&src.device) {
                        Some(NetSpec::SrtListen { port }) => port,
                        _ => 9000,
                    };
                    if ui.add(egui::DragValue::new(&mut port).range(1024..=65535)).changed() {
                        src.device = format!("srt-in:{port}");
                    }
                    ui.end_row();
                }
                SourceKind::SrtCall => {
                    label(ui, "Address");
                    ui.add(egui::TextEdit::singleline(&mut src.device).desired_width(320.0).hint_text("srt://host:port?streamid=..."));
                    ui.end_row();
                }
                SourceKind::Rtmp => {
                    let (mut port, mut key) = match netsrc::parse_spec(&src.device) {
                        Some(NetSpec::RtmpListen { port, key }) => (port, key),
                        _ => (1935, String::new()),
                    };
                    label(ui, "Listen on port");
                    let a = ui.add(egui::DragValue::new(&mut port).range(1024..=65535)).changed();
                    ui.end_row();
                    label(ui, "Stream key");
                    let b = ui
                        .horizontal(|ui| {
                            let c = ui.add(egui::TextEdit::singleline(&mut key).desired_width(160.0).hint_text("any")).changed();
                            let d = ui.small_button("New key").clicked();
                            if d {
                                key = random_key();
                            }
                            c || d
                        })
                        .inner;
                    if a || b {
                        src.device = format!("rtmp-in:{port}/{}", key.trim());
                    }
                    ui.end_row();
                }
                SourceKind::Ndi => {
                    label(ui, "NDI source");
                    ui.horizontal(|ui| {
                        let mut name = src.device.trim().strip_prefix("ndi:").unwrap_or("").to_string();
                        let r = egui::ComboBox::from_id_salt("ndi_source").selected_text(if name.is_empty() { "choose…".to_string() } else { name.clone() }).width(260.0).show_ui(ui, |ui| {
                            match &ndi {
                                Some(Ok(list)) if !list.is_empty() => {
                                    for n in list {
                                        ui.selectable_value(&mut name, n.clone(), n);
                                    }
                                }
                                Some(Ok(_)) => {
                                    ui.label(RichText::new("No NDI sources found on the network").color(TEXT_DIM));
                                }
                                Some(Err(e)) => {
                                    ui.label(RichText::new(e).color(AMBER));
                                }
                                None => {
                                    ui.label(RichText::new("Press Find to look for sources").color(TEXT_DIM));
                                }
                            }
                        });
                        let _ = r;
                        if ui.button("Find").clicked() {
                            scan_ndi = true;
                        }
                        src.device = format!("ndi:{name}");
                    });
                    ui.end_row();
                }
                SourceKind::Framed => {
                    let from = src.framed_from().unwrap_or("").to_string();
                    label(ui, "Cut from");
                    egui::ComboBox::from_id_salt("frame_from").selected_text(if from.is_empty() { "choose…" } else { &from }).width(240.0).show_ui(ui, |ui| {
                        if wides.is_empty() {
                            ui.label(RichText::new("Add the wide camera first").color(TEXT_DIM));
                        }
                        for (name, _) in &wides {
                            if ui.selectable_label(name.eq_ignore_ascii_case(&from), name).clicked() {
                                src.device = format!("frame:{name}");
                            }
                        }
                    });
                    ui.end_row();
                    if let Some((_, fps)) = wides.iter().find(|w| w.0.eq_ignore_ascii_case(&from)) {
                        src.fps = *fps;
                    }
                    framing_editor(ui, &mut src.framing);
                }
                SourceKind::VirtualPtz => {
                    let from = src.virtual_ptz_of().unwrap_or("").to_string();
                    label(ui, "Cut from");
                    egui::ComboBox::from_id_salt("vptz_from").selected_text(if from.is_empty() { "choose…" } else { &from }).width(240.0).show_ui(ui, |ui| {
                        if wides.is_empty() {
                            ui.label(RichText::new("Add the wide camera first").color(TEXT_DIM));
                        }
                        for (name, _) in &wides {
                            if ui.selectable_label(name.eq_ignore_ascii_case(&from), name).clicked() {
                                src.device = format!("vptz:{name}");
                            }
                        }
                    });
                    ui.end_row();
                    if let Some((_, fps)) = wides.iter().find(|w| w.0.eq_ignore_ascii_case(&from)) {
                        src.fps = *fps;
                    }
                    label(ui, "Zoom limit");
                    ui.add(egui::Slider::new(&mut src.ptz.max_zoom, 1.0..=4.0).step_by(0.1).suffix("×"));
                    ui.end_row();
                    label(ui, "Moves to presets take");
                    ui.add(egui::Slider::new(&mut src.ptz.glide_seconds, 0.2..=6.0).step_by(0.1).suffix(" s"));
                    ui.end_row();
                }
                SourceKind::Screen | SourceKind::Media | SourceKind::Phone | SourceKind::Guest => {}
            }

            let network = matches!(kind, SourceKind::SrtListen | SourceKind::SrtCall | SourceKind::Rtmp | SourceKind::Ndi | SourceKind::Phone | SourceKind::Guest);
            label(ui, match kind {
                SourceKind::Device | SourceKind::Screen => "Capture size",
                SourceKind::Framed | SourceKind::VirtualPtz => "Output size",
                _ => "Picture size",
            });
            ui.horizontal(|ui| {
                size_combo(ui, "src_size", &mut src.width, &mut src.height);
                ui.add(egui::DragValue::new(&mut src.width).range(160..=7680).speed(2));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut src.height).range(90..=4320).speed(2));
            });
            ui.end_row();
            if !matches!(kind, SourceKind::Framed | SourceKind::VirtualPtz) {
                label(ui, "Frame rate");
                let modes = devices.iter().find(|d| d.name == src.device).map(|d| d.modes.clone()).unwrap_or_default();
                let allowed = modes.iter().find(|m| m.width == src.width && m.height == src.height).map(|m| m.fps.clone());
                fps_picker(ui, "src_fps", &mut src.fps, allowed.as_deref());
                ui.end_row();
            }
            if network {
                label(ui, "Buffer");
                ui.add(egui::DragValue::new(&mut src.network_latency_ms).range(40..=3000).speed(10).suffix(" ms"));
                ui.end_row();
            }
            label(ui, "Record to its own file");
            ui.checkbox(&mut src.record, "");
            ui.end_row();
            if !matches!(kind, SourceKind::Framed | SourceKind::VirtualPtz) {
                label(ui, "Delay");
                ui.add(egui::DragValue::new(&mut src.delay_ms).range(0..=2000).suffix(" ms"));
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        match kind {
            SourceKind::Device => {
                if let Some(d) = self.video.iter().find(|d| d.name == src.device) {
                    hint(ui, &format!("Reports: {}", if d.summary.is_empty() { "no format details" } else { &d.summary }));
                } else if !self.video.is_empty() {
                    ui.label(RichText::new("⚠ This device isn't connected right now. It will connect by itself when plugged in.").color(AMBER));
                }
            }
            SourceKind::Screen => hint(ui, if cfg!(windows) { "Captures your main display." } else { "Captures your main display (allow Screen Recording for Cutaway in System Settings → Privacy & Security)." }),
            SourceKind::Test => {}
            SourceKind::Media => {
                media_editor(ui, &mut src.media);
                hint(ui, &format!("To hear its sound, add an audio input set to \"Sound from {}\".", src.name));
            }
            SourceKind::SrtListen | SourceKind::Rtmp => {
                if let Some(spec) = netsrc::parse_spec(&src.device) {
                    ui.label(RichText::new("Send to:").strong());
                    ui.label(RichText::new(netsrc::sender_address(&spec)).monospace().color(ACCENT));
                }
                hint(ui, &format!("For OBS, a phone app (e.g. Larix), a hardware encoder or another computer: H.264 or HEVC with AAC sound. To hear it, add an audio input set to \"Sound from {}\".", src.name));
            }
            SourceKind::Framed => {
                if let Some(p) = crate::platform::vision::problem() {
                    ui.label(RichText::new(format!("⚠ Auto-framing isn't available here: {p}. The whole picture is shown instead.")).color(AMBER));
                }
                let from = src.framed_from().unwrap_or("the wide camera");
                hint(ui, &format!(
                    "Finds the people in {from} and cuts out a shot that follows them, gliding like a camera operator: another camera without one. {how}; nothing leaves the computer.",
                    how = if cfg!(target_os = "macos") { "People are found by Apple's Vision framework" } else { "People are found by a small open model on the graphics card" }
                ));
            }
            SourceKind::VirtualPtz => {
                let from = src.virtual_ptz_of().unwrap_or("the wide camera");
                hint(ui, &format!("A camera operator inside {from}: pan, tilt and zoom with the PTZ panel, the Stream Deck or the remote, and save shots as presets it glides to."));
            }
            SourceKind::SrtCall => hint(ui, "Cutaway connects to an SRT listener (a receiver, an encoder or a relay) and pulls its stream."),
            SourceKind::Phone | SourceKind::Guest => {
                let id = src.link_id().map(|l| l.0.to_string()).unwrap_or_default();
                let link = info.guest_base.as_ref().filter(|_| applied.contains(&src.device.trim().to_string())).map(|b| format!("{b}/g/{id}"));
                if kind == SourceKind::Phone {
                    match &link {
                        Some(link) => {
                            ui.horizontal(|ui| {
                                qr_code(ui, link, 150.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("Scan with the phone's camera app").strong());
                                    ui.add(egui::Label::new(RichText::new(link).monospace().size(12.0)).truncate());
                                    if ui.small_button("Copy link").clicked() {
                                        ui.ctx().copy_text(link.clone());
                                    }
                                });
                            });
                        }
                        None => hint(ui, "Its QR code appears here once Remote guests is on and this is applied."),
                    }
                    hint(ui, "Picture only, up to 1080p, straight over your network; hold the phone sideways. Safari on iPhone, Chrome on Android.");
                } else {
                    if let Some(link) = &link {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(link).monospace().size(12.0)).truncate());
                            if ui.small_button("Copy link").clicked() {
                                ui.ctx().copy_text(link.clone());
                            }
                        });
                    }
                    hint(ui, &format!("A guest who joins by link. Their sound arrives as the audio input \"Sound from {}\"; their links, and adding or removing spots, are in Remote guests.", src.name));
                }
            }
            SourceKind::Ndi => {
                super::ndi_offer(ui);
            }
        }
        ui.add_space(10.0);
        let is_virtual = kind == SourceKind::VirtualPtz;
        let is_ndi = kind == SourceKind::Ndi;
        let presets = match src.ptz.presets.len() {
            0 => "no presets".to_string(),
            1 => "1 preset".to_string(),
            n => format!("{n} presets"),
        };
        let (ptz_title, ptz_state) = if is_virtual {
            ("Presets", presets)
        } else if src.ptz.control == crate::config::PtzControl::None {
            ("PTZ camera control", "off".to_string())
        } else {
            ("PTZ camera control", format!("{} · {presets}", src.ptz.control.label().split(" (").next().unwrap_or("")))
        };
        part(ui, ("ptz", self.sel_source), ptz_title, &ptz_state, src.has_ptz(), |ui| ptz_editor(ui, &mut src.ptz, is_virtual, is_ndi));
        let look = look_summary(&src.look);
        part(ui, ("look", self.sel_source), "Picture adjustments", &look, false, |ui| {
            look_editor(ui, &mut src.look, &mut self.lut_check);
        });
        if guests_on {
            self.draft.guests.enabled = true;
        }
        if scan_ndi {
            let out = self.ndi_found.clone();
            *out.lock().unwrap() = None;
            std::thread::spawn(move || {
                let r = crate::engine::ndi::find_sources(2500).map_err(|e| format!("{e:#}"));
                *out.lock().unwrap() = Some(r);
            });
        }
        // Renamed: close-ups cut from this source follow the new name.
        let new_name = self.draft.sources[self.sel_source].name.clone();
        if new_name != old_name {
            for s in &mut self.draft.sources {
                if s.framed_from().is_some_and(|f| f.eq_ignore_ascii_case(&old_name)) {
                    s.device = format!("frame:{new_name}");
                }
                if s.virtual_ptz_of().is_some_and(|f| f.eq_ignore_ascii_case(&old_name)) {
                    s.device = format!("vptz:{new_name}");
                }
            }
        }
    }
}

/// A PTZ camera's controls and presets.
fn ptz_editor(ui: &mut egui::Ui, p: &mut crate::config::PtzConfig, is_virtual: bool, is_ndi: bool) {
    use crate::config::{PtzControl, PtzPreset};
    egui::Grid::new("ptz").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        if !is_virtual {
            label(ui, "Controlled by");
            egui::ComboBox::from_id_salt("ptz_control").selected_text(p.control.label()).width(380.0).show_ui(ui, |ui| {
                for c in PtzControl::ALL {
                    if c == PtzControl::Ndi && !is_ndi {
                        continue;
                    }
                    ui.selectable_value(&mut p.control, c, c.label());
                }
            });
            ui.end_row();
            if matches!(p.control, PtzControl::ViscaIp | PtzControl::ViscaTcp | PtzControl::ViscaUdp) {
                label(ui, "Camera's address");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.address).desired_width(200.0).hint_text(format!("192.168.1.50 (port {})", p.control.default_port())));
                    ui.label("camera");
                    ui.add(egui::DragValue::new(&mut p.camera).range(1..=7)).on_hover_text("VISCA camera number: 1 unless it's daisy-chained");
                });
                ui.end_row();
            }
        }
        if is_virtual || p.control != PtzControl::None {
            label(ui, "Speed");
            ui.add(egui::Slider::new(&mut p.speed, 0.1..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            ui.end_row();
            label(ui, "Mounted upside down");
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.invert_pan, "flip left/right");
                ui.checkbox(&mut p.invert_tilt, "flip up/down");
            });
            ui.end_row();
        }
    });
    if !is_virtual && p.control == PtzControl::None {
        hint(ui, "For network PTZ cameras: most speak VISCA (Sony, BirdDog, PTZOptics, Panasonic, Lumens…).");
        return;
    }
    ui.add_space(6.0);
    subtitle(ui, "Presets");
    let mut remove = None;
    for (i, pr) in p.presets.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}.", i + 1)).color(TEXT_DIM));
            ui.add_sized([180.0, 22.0], egui::TextEdit::singleline(&mut pr.name));
            if is_virtual {
                hint(ui, &format!("{:.0}% across, {:.0}% down, {:.1}×", pr.x * 100.0, pr.y * 100.0, pr.zoom));
            }
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        p.presets.remove(i);
    }
    if ui.button("＋ Preset").clicked() {
        let name = format!("Preset {}", p.presets.len() + 1);
        p.presets.push(PtzPreset { name, ..Default::default() });
    }
    hint(
        ui,
        if is_virtual {
            "Frame a shot with the PTZ panel, then right-click a preset there and choose Save, or press ＋ in the panel. Presets are recalled with a smooth move."
        } else {
            "Presets live in the camera, in this order (preset 1 is the camera's memory 0). Frame a shot with the PTZ panel, then right-click a preset there to save it into the camera."
        },
    );
}

/// Who an auto-framed source follows, and how tight (grid rows).
fn framing_editor(ui: &mut egui::Ui, f: &mut FramingConfig) {
    label(ui, "Follow");
    egui::ComboBox::from_id_salt("frame_follow").selected_text(f.follow.label()).width(240.0).show_ui(ui, |ui| {
        for v in FramingFollow::ALL {
            ui.selectable_value(&mut f.follow, v, v.label());
        }
    });
    ui.end_row();
    label(ui, "Shot");
    ui.horizontal(|ui| {
        for v in FramingShot::ALL {
            ui.selectable_value(&mut f.shot, v, v.label());
        }
    });
    ui.end_row();
    label(ui, "Moves take");
    ui.add(egui::Slider::new(&mut f.reframe_seconds, 0.3..=4.0).step_by(0.1).suffix(" s"));
    ui.end_row();
    label(ui, "Zoom limit");
    ui.add(egui::Slider::new(&mut f.max_zoom, 1.0..=3.0).step_by(0.1).suffix("×"));
    ui.end_row();
}

fn media_editor(ui: &mut egui::Ui, m: &mut MediaConfig) {
    subtitle(ui, "Playlist");
    let mut remove = None;
    let mut swap = None;
    egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(8)).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width().min(520.0));
        if m.files.is_empty() {
            hint(ui, "Empty. Add MP4/MOV videos (H.264, HEVC or ProRes, with AAC sound) or PNG/JPEG images.");
        }
        let n = m.files.len();
        for (i, f) in m.files.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).color(TEXT_DIM));
                let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
                let missing = !f.exists();
                ui.label(RichText::new(&name).color(if missing { AMBER } else { Color32::WHITE })).on_hover_text(f.display().to_string());
                if missing {
                    ui.label(RichText::new("missing").color(AMBER).size(11.0));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                    if i + 1 < n && ui.small_button("↓").clicked() {
                        swap = Some((i, i + 1));
                    }
                    if i > 0 && ui.small_button("↑").clicked() {
                        swap = Some((i, i - 1));
                    }
                });
            });
        }
    });
    if let Some(i) = remove {
        m.files.remove(i);
    }
    if let Some((a, b)) = swap {
        m.files.swap(a, b);
    }
    ui.horizontal(|ui| {
        if ui.button("Add files…").clicked() {
            if let Some(files) = rfd::FileDialog::new().add_filter("Videos and images", &["mp4", "mov", "m4v", "png", "jpg", "jpeg"]).pick_files() {
                m.files.extend(files);
            }
        }
        if ui.button("Add a folder…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                m.files.extend(crate::engine::player::playable_in(&dir));
            }
        }
    });
    ui.add_space(4.0);
    egui::Grid::new("media").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        label(ui, "Loop");
        ui.checkbox(&mut m.loop_playlist, "the playlist");
        ui.end_row();
        label(ui, "At start");
        ui.checkbox(&mut m.autoplay, "play when Cutaway starts");
        ui.end_row();
        label(ui, "When taken");
        ui.checkbox(&mut m.restart_on_air, "start from the beginning");
        ui.end_row();
        label(ui, "Show images for");
        ui.add(egui::DragValue::new(&mut m.image_seconds).range(0.5..=600.0).speed(0.1).suffix(" s"));
        ui.end_row();
    });
}

/// What's been done to a source's picture, in a few words.
fn look_summary(l: &SourceLook) -> String {
    let d = SourceLook::default();
    let mut on = Vec::new();
    if l.crop_left > 0.0 || l.crop_right > 0.0 || l.crop_top > 0.0 || l.crop_bottom > 0.0 {
        on.push("cropped");
    }
    if l.mirror {
        on.push("mirrored");
    }
    if l.brightness != d.brightness || l.contrast != d.contrast || l.saturation != d.saturation {
        on.push("colour");
    }
    if l.lut.is_some() {
        on.push("LUT");
    }
    if l.remove_background {
        on.push("background removed");
    }
    if l.chroma_key {
        on.push("green screen");
    }
    if on.is_empty() { "none".into() } else { on.join(", ") }
}

fn look_editor(ui: &mut egui::Ui, l: &mut SourceLook, lut_check: &mut Option<(std::path::PathBuf, Result<String, String>)>) {
    egui::Grid::new("look").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        label(ui, "Crop left / right");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut l.crop_left, 0.0..=45.0).suffix("%").step_by(0.5));
            ui.add(egui::Slider::new(&mut l.crop_right, 0.0..=45.0).suffix("%").step_by(0.5));
        });
        ui.end_row();
        label(ui, "Crop top / bottom");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut l.crop_top, 0.0..=45.0).suffix("%").step_by(0.5));
            ui.add(egui::Slider::new(&mut l.crop_bottom, 0.0..=45.0).suffix("%").step_by(0.5));
        });
        ui.end_row();
        label(ui, "Mirror");
        ui.checkbox(&mut l.mirror, "flip left to right");
        ui.end_row();
        label(ui, "Brightness");
        ui.add(egui::Slider::new(&mut l.brightness, -0.5..=0.5).custom_formatter(|v, _| format!("{:+.0}", v * 100.0)));
        ui.end_row();
        label(ui, "Contrast");
        ui.add(egui::Slider::new(&mut l.contrast, 0.5..=1.5).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        ui.end_row();
        label(ui, "Saturation");
        ui.add(egui::Slider::new(&mut l.saturation, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        ui.end_row();
        label(ui, "Colour LUT");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let shown = l.lut.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "none".into());
                ui.label(shown);
                if ui.button("Choose…").on_hover_text("A .cube file, e.g. your camera's log-to-Rec.709 LUT").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("Colour LUT", &["cube", "CUBE"]).pick_file() {
                        l.lut = Some(f);
                        l.lut_strength = 1.0;
                    }
                }
                if l.lut.is_some() && ui.small_button("Clear").clicked() {
                    l.lut = None;
                }
            });
            if let Some(path) = &l.lut {
                // Checked once, so a bad file shows straight away.
                if lut_check.as_ref().is_none_or(|(p, _)| p != path) {
                    *lut_check = Some((path.clone(), crate::media::cube::Lut3d::load(path).map(|l| l.describe()).map_err(|e| format!("{e:#}"))));
                }
                match &lut_check.as_ref().unwrap().1 {
                    Ok(what) => {
                        ui.add(egui::Slider::new(&mut l.lut_strength, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).text("strength"));
                        hint(ui, what);
                    }
                    Err(e) => {
                        ui.label(RichText::new(format!("⚠ {e}")).color(AMBER));
                    }
                }
            }
        });
        ui.end_row();
        label(ui, "Remove background");
        ui.horizontal(|ui| {
            ui.checkbox(&mut l.remove_background, "cut the people out (no green screen)");
            if l.remove_background {
                egui::ComboBox::from_id_salt("cutout_q").selected_text(l.cutout_quality.label()).width(110.0).show_ui(ui, |ui| {
                    for q in crate::config::CutoutQuality::ALL {
                        ui.selectable_value(&mut l.cutout_quality, q, q.label());
                    }
                });
            }
        });
        ui.end_row();
        if l.remove_background {
            ui.label("");
            match crate::platform::vision::problem() {
                Some(p) => {
                    ui.label(RichText::new(format!("⚠ Not available here: {p}.")).color(AMBER));
                }
                None => {
                    if !cfg!(target_os = "macos") {
                        hint(ui, "(Fast, Balanced and Best edges only differ on a Mac.)");
                    } else {
                        ui.label("");
                    }
                }
            }
            ui.end_row();
        }
        label(ui, "Green / blue screen");
        ui.checkbox(&mut l.chroma_key, "remove the screen colour");
        ui.end_row();
        if l.chroma_key {
            label(ui, "Screen colour");
            ui.horizontal(|ui| {
                let mut rgb = crate::config::parse_color(&l.key_color).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or([0, 208, 64]);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    l.key_color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
                if ui.small_button("Green").clicked() {
                    l.key_color = "#00d040".into();
                }
                if ui.small_button("Blue").clicked() {
                    l.key_color = "#0047bb".into();
                }
            });
            ui.end_row();
            label(ui, "Amount");
            ui.add(egui::Slider::new(&mut l.key_similarity, 0.05..=0.9));
            ui.end_row();
            label(ui, "Edge softness");
            ui.add(egui::Slider::new(&mut l.key_smoothness, 0.0..=0.5));
            ui.end_row();
            label(ui, "Spill removal");
            ui.add(egui::Slider::new(&mut l.key_spill, 0.0..=1.0));
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Reset picture adjustments").clicked() {
            *l = SourceLook::default();
        }
        hint(ui, "The program only (its own file stays untouched); applies live.");
    });
}
