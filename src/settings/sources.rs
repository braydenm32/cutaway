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
}

impl SourceKind {
    const ALL: [SourceKind; 9] = [
        SourceKind::Device,
        SourceKind::Screen,
        SourceKind::Test,
        SourceKind::Media,
        SourceKind::SrtListen,
        SourceKind::SrtCall,
        SourceKind::Rtmp,
        SourceKind::Ndi,
        SourceKind::Framed,
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
        }
    }
}

fn random_key() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() ^ (d.as_secs() as u32).wrapping_mul(2654435761)).unwrap_or(1);
    format!("cam{:05}", n % 100_000)
}

impl SettingsUi {
    pub(super) fn sources(&mut self, ui: &mut egui::Ui) {
        title(ui, "Video sources", "Every source is shown in the multiview and can be recorded to its own file.");
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
            ui.vertical(|ui| self.source_detail(ui));
        });
    }

    fn source_detail(&mut self, ui: &mut egui::Ui) {
        let ndi = self.ndi_found.lock().unwrap().clone();
        // Sources a close-up can be cut from: (name, fps).
        let wides: Vec<(String, u32)> = self
            .draft
            .sources
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != self.sel_source && s.framed_from().is_none())
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
                        if k == SourceKind::Media || matches!(k, SourceKind::SrtListen | SourceKind::SrtCall | SourceKind::Rtmp | SourceKind::Ndi) {
                            src.record = k != SourceKind::Media;
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
                SourceKind::Screen | SourceKind::Media => {}
            }

            let network = matches!(kind, SourceKind::SrtListen | SourceKind::SrtCall | SourceKind::Rtmp | SourceKind::Ndi);
            label(ui, match kind {
                SourceKind::Device | SourceKind::Screen => "Capture size",
                SourceKind::Framed => "Output size",
                _ => "Picture size",
            });
            ui.horizontal(|ui| {
                size_combo(ui, "src_size", &mut src.width, &mut src.height);
                ui.add(egui::DragValue::new(&mut src.width).range(160..=7680).speed(2));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut src.height).range(90..=4320).speed(2));
            });
            ui.end_row();
            if kind != SourceKind::Framed {
                label(ui, "Frame rate");
                let modes = devices.iter().find(|d| d.name == src.device).map(|d| d.modes.clone()).unwrap_or_default();
                let allowed = modes.iter().find(|m| m.width == src.width && m.height == src.height).map(|m| m.fps.clone());
                fps_picker(ui, "src_fps", &mut src.fps, allowed.as_deref());
                ui.end_row();
            }
            if network {
                label(ui, "Buffer");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut src.network_latency_ms).range(40..=3000).speed(10).suffix(" ms"));
                    hint(ui, "more rides out network hiccups; less is closer to live");
                });
                ui.end_row();
            }
            label(ui, "Record to its own file");
            ui.checkbox(&mut src.record, "");
            ui.end_row();
            if kind != SourceKind::Framed {
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
                hint(ui, "If the device can't do the exact size or rate, Cutaway converts so every file stays consistent. Delay holds this source back to line it up with slower sources or the audio.");
            }
            SourceKind::Screen => hint(ui, if cfg!(windows) { "Captures your main display." } else { "Captures your main display. On macOS, allow Screen Recording for Cutaway in System Settings → Privacy & Security." }),
            SourceKind::Test => hint(ui, "A generated test picture with a flash every second — handy for trying layouts and checking sync."),
            SourceKind::Media => {
                media_editor(ui, &mut src.media);
                hint(ui, &format!("To hear its sound, add an audio input set to \"Sound from {}\".", src.name));
            }
            SourceKind::SrtListen | SourceKind::Rtmp => {
                if let Some(spec) = netsrc::parse_spec(&src.device) {
                    ui.label(RichText::new("Send to:").strong());
                    ui.label(RichText::new(netsrc::sender_address(&spec)).monospace().color(ACCENT));
                }
                hint(ui, &format!(
                    "Point OBS, a phone camera app (e.g. Larix), a hardware encoder or another computer here. H.264 or HEVC video with AAC sound. Add an audio input set to \"Sound from {}\" to hear it.",
                    src.name
                ));
            }
            SourceKind::Framed => {
                if let Some(p) = crate::platform::vision::problem() {
                    ui.label(RichText::new(format!("⚠ Auto-framing isn't available here: {p}. The whole picture is shown instead.")).color(AMBER));
                }
                let from = src.framed_from().unwrap_or("the wide camera");
                hint(ui, &format!(
                    "Finds the people in {from} and cuts a shot out of it that follows them, gliding like a camera operator — a second (or third) camera without one. Best from a 4K camera: a 1080p close-up cut from 4K stays sharp. Runs and records like any source, at {from}'s frame rate and delay. {how}; nothing leaves the computer.",
                    how = if cfg!(target_os = "macos") { "People are found by Apple's Vision framework" } else { "People are found by a small open model on the graphics card" }
                ));
            }
            SourceKind::SrtCall => hint(ui, "Cutaway connects to an SRT listener (a receiver, an encoder or a relay) and pulls its stream."),
            SourceKind::Ndi => {
                if !crate::engine::ndi::available() {
                    ui.label(RichText::new("⚠ NDI isn't installed on this computer. Install the free NDI Tools from ndi.video/tools, then restart Cutaway.").color(AMBER));
                }
                hint(ui, "NDI sources on your network: other computers running NDI Tools or OBS with NDI, PTZ cameras, NDI converters.");
            }
        }
        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Picture adjustments: crop, mirror, colour, background removal, green screen").strong()).id_salt(("look", self.sel_source)).show(ui, |ui| {
            look_editor(ui, &mut src.look);
        });
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
            }
        }
    }
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
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut f.reframe_seconds, 0.3..=4.0).step_by(0.1).suffix(" s"));
        hint(ui, "longer is calmer");
    });
    ui.end_row();
    label(ui, "Zoom limit");
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut f.max_zoom, 1.0..=3.0).step_by(0.1).suffix("×"));
        hint(ui, "how far the wide picture may be enlarged; more is tighter but softer");
    });
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
    ui.checkbox(&mut m.loop_playlist, "Loop the playlist");
    ui.checkbox(&mut m.autoplay, "Play when Cutaway starts");
    ui.checkbox(&mut m.restart_on_air, "Start from the beginning when a scene with this source is taken");
    ui.horizontal(|ui| {
        ui.label("Show images for");
        ui.add(egui::DragValue::new(&mut m.image_seconds).range(0.5..=600.0).speed(0.1).suffix(" s"));
    });
}

fn look_editor(ui: &mut egui::Ui, l: &mut SourceLook) {
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
        ui.checkbox(&mut l.mirror, "flip left to right (e.g. a camera filming a presenter's screen)");
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
        label(ui, "Remove background");
        ui.horizontal(|ui| {
            ui.checkbox(&mut l.remove_background, "cut the people out, no green screen needed");
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
                None => hint(
                    ui,
                    if cfg!(target_os = "macos") {
                        "Apple's Vision framework finds the people in every picture, on this Mac. What's behind them shows the scene's background or the sources layered below. \"Best edges\" is finer around hair but busier."
                    } else {
                        "A small open model finds the people in every picture, on the graphics card. What's behind them shows the scene's background or the sources layered below. (Fast, Balanced and Best edges only differ on a Mac.)"
                    },
                ),
            }
            ui.end_row();
        }
        label(ui, "Green / blue screen");
        ui.checkbox(&mut l.chroma_key, "remove the screen colour behind this source");
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
        hint(ui, "These change the program only; the source's own file stays untouched. They apply live.");
    });
}
