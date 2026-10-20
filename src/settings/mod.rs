//! The Settings window: a sidebar of sections, each editing a draft copy of
//! the configuration. Nothing changes until Apply/OK; changes that need the
//! video engine rebuilt are flagged, everything else applies live.

mod captions;
mod comments;
mod control;
mod graphics;
mod guests;
mod help;
mod macros;
mod prompter;
mod mixing;
mod outputs;
mod rundown;
mod sound;
mod sources;
mod switching;
mod upload;
mod vertical;


use crate::config::{AudioConfig, Codec, Config, Container, Service, SourceConfig, StreamDestination};
use crate::devices::{self, DeviceInfo, Kind, Mode, TEST_PATTERN_NAMES};
use crate::engine::{LogLevel, LogLine};
use crate::ui::theme::*;
use eframe::egui::{self, vec2, Align, Color32, Layout, RichText};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Section {
    General,
    Sources,
    Audio,
    Mixing,
    Pads,
    Syrinscape,
    Program,
    Switching,
    Graphics,
    Rundown,
    Macros,
    Prompter,
    Comments,
    Guests,
    Captions,
    Recording,
    Streaming,
    Vertical,
    Upload,
    Outputs,
    Control,
    Shortcuts,
    Diagnostics,
}

impl Section {
    const ALL: [Section; 23] = [
        Section::General,
        Section::Sources,
        Section::Audio,
        Section::Mixing,
        Section::Pads,
        Section::Syrinscape,
        Section::Program,
        Section::Switching,
        Section::Graphics,
        Section::Rundown,
        Section::Macros,
        Section::Prompter,
        Section::Comments,
        Section::Guests,
        Section::Captions,
        Section::Recording,
        Section::Streaming,
        Section::Vertical,
        Section::Upload,
        Section::Outputs,
        Section::Control,
        Section::Shortcuts,
        Section::Diagnostics,
    ];
    /// For scripted screenshots: "streaming", "sources", ...
    pub fn from_name(name: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.label().to_lowercase().contains(&name.to_lowercase()))
    }

    fn label(self) -> &'static str {
        match self {
            Section::General => "General & shows",
            Section::Sources => "Video sources",
            Section::Audio => "Audio inputs",
            Section::Mixing => "Mixing & headphones",
            Section::Pads => "Sound pads",
            Section::Syrinscape => "Syrinscape",
            Section::Program => "Program",
            Section::Switching => "Switching",
            Section::Graphics => "Graphics",
            Section::Rundown => "Rundown",
            Section::Macros => "Macros",
            Section::Prompter => "Teleprompter",
            Section::Comments => "Viewer comments",
            Section::Guests => "Remote guests",
            Section::Captions => "Captions",
            Section::Recording => "Recording",
            Section::Streaming => "Streaming",
            Section::Vertical => "Vertical version",
            Section::Upload => "Upload",
            Section::Outputs => "Screens & webcam",
            Section::Control => "Remote & MIDI",
            Section::Shortcuts => "Shortcuts",
            Section::Diagnostics => "Diagnostics",
        }
    }

    fn group(self) -> Group {
        Group::ALL.into_iter().find(|g| g.sections().contains(&self)).unwrap_or(Group::General)
    }
}

/// The sidebar: a few groups, each with its pages as tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Group {
    Inputs,
    Sound,
    Look,
    Show,
    Output,
    General,
}

impl Group {
    const ALL: [Group; 6] = [Group::Inputs, Group::Sound, Group::Look, Group::Show, Group::Output, Group::General];

    fn label(self) -> &'static str {
        match self {
            Group::Inputs => "Inputs",
            Group::Sound => "Sound",
            Group::Look => "Look",
            Group::Show => "Show",
            Group::Output => "Output",
            Group::General => "General",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Group::Inputs => "Cameras, mics and guests",
            Group::Sound => "Mixing, headphones, pads",
            Group::Look => "Program, graphics, captions",
            Group::Show => "Switching, rundown, prompter",
            Group::Output => "Recording, streaming, screens",
            Group::General => "Shows, control, diagnostics",
        }
    }

    fn sections(self) -> &'static [Section] {
        match self {
            Group::Inputs => &[Section::Sources, Section::Audio, Section::Guests],
            Group::Sound => &[Section::Mixing, Section::Pads, Section::Syrinscape],
            Group::Look => &[Section::Program, Section::Graphics, Section::Captions],
            Group::Show => &[Section::Switching, Section::Rundown, Section::Macros, Section::Prompter, Section::Comments],
            Group::Output => &[Section::Recording, Section::Streaming, Section::Vertical, Section::Upload, Section::Outputs],
            Group::General => &[Section::General, Section::Control, Section::Shortcuts, Section::Diagnostics],
        }
    }
}

/// What the app should do after showing the window this frame.
pub enum SettingsAction {
    None,
    Close,
    Apply { cfg: Config, close: bool },
    /// Turn the virtual camera on or off now.
    VirtualCamera(bool),
    /// Close and measure lip sync.
    MeasureSync,
}

/// Read-only facts the window shows (engine state, log, ...).
pub struct SettingsContext {
    pub recording: bool,
    pub streaming: bool,
    pub engine_facts: Vec<(String, String)>,
    pub log: Vec<LogLine>,
    pub config_path: Option<PathBuf>,
    /// Where show profiles live (None in demo mode).
    pub profiles_dir: Option<PathBuf>,
    /// Last MIDI message received, for "learn".
    pub midi_last: Option<String>,
    pub midi_connected: Vec<String>,
    /// Remote control server port, while running.
    pub remote_port: Option<u16>,
    pub osc_port: Option<u16>,
    /// Live captions: "listening", or why not (None when off).
    pub captions_status: Option<String>,
    pub vcam_on: bool,
    /// Live level (dB) of each audio input's channels.
    pub input_levels: Vec<Vec<f32>>,
    /// The stinger video as loaded.
    pub stinger: Option<crate::engine::stinger::StingerInfo>,
    /// Remote guests: how they reach Cutaway, the links' start, who's on.
    pub guest_reach: crate::net::tunnel::TunnelState,
    pub guest_base: Option<String>,
    pub guest_status: Vec<crate::engine::guests::GuestInfo>,
    /// NDI receivers watching the program (None: not sending).
    pub ndi_watchers: Option<u32>,
    /// The sources on the program right now (by uid).
    pub on_air: Vec<u64>,
}

/// Applying would take sources or inputs out of a show that's recording or
/// live: asked first.
struct RemoveCheck {
    names: Vec<String>,
    /// Of those, the ones on the program right now.
    on_air: Vec<String>,
    recording: bool,
    streaming: bool,
    close: bool,
}

type EncoderLists = (Vec<String>, Vec<String>);

pub struct SettingsUi {
    draft: Config,
    original: Config,
    section: Section,
    video: Vec<DeviceInfo>,
    audio: Vec<DeviceInfo>,
    sel_source: usize,
    sel_audio: usize,
    sel_dest: usize,
    /// Signing in to Google (Settings → Upload).
    google: Option<upload::GoogleSignIn>,
    show_keys: HashSet<usize>,
    encoders: Arc<Mutex<Option<EncoderLists>>>,
    scanning: bool,
    confirm_discard: bool,
    confirm_reset: bool,
    confirm_remove: Option<RemoveCheck>,
    message: Option<(String, bool)>,
    outputs: Vec<DeviceInfo>,
    sel_graphic: usize,
    profile_name: String,
    rename_to: String,
    ndi_found: Arc<Mutex<Option<Result<Vec<String>, String>>>>,
    vcam_job: Arc<Mutex<Option<Result<String, String>>>>,
    vcam_busy: bool,
    learning: Option<usize>,
    /// Speech recognition languages (asked for once).
    languages: Option<Vec<(String, String)>>,
    /// What happened when the Stream Deck plugin was opened.
    plugin_note: Option<String>,
    pending: Option<SettingsAction>,
    previews: std::collections::HashMap<String, (u64, egui::TextureHandle)>,
    /// Syrinscape: a connection for testing and browsing, what was opened.
    syrin: Option<crate::control::syrinscape::Syrinscape>,
    syrin_tested: bool,
    syrin_link: String,
    syrin_soundset: Option<String>,
    sel_segment: usize,
    sel_macro: usize,
    /// Apps that can be listened to (for "Sound from an app").
    apps: Option<Vec<String>>,
    /// Installing the Resolve script: what happened.
    resolve_note: Option<(String, bool)>,
    /// The LUT picked for a source, checked: (file, description or problem).
    lut_check: Option<(PathBuf, Result<String, String>)>,
    /// The tab last open in each group.
    last_tab: std::collections::HashMap<Group, Section>,
}

const RESOLUTIONS: &[(u32, u32, &str)] = &[
    (1280, 720, "1280×720 (720p)"),
    (1920, 1080, "1920×1080 (1080p)"),
    (2560, 1440, "2560×1440 (1440p)"),
    (3840, 2160, "3840×2160 (4K UHD)"),
    (1080, 1920, "1080×1920 (vertical)"),
    (1080, 1080, "1080×1080 (square)"),
];
const RATES: &[u32] = &[24, 25, 30, 50, 60];

impl SettingsUi {
    pub fn new(cfg: Config) -> Self {
        SettingsUi {
            draft: cfg.clone(),
            original: cfg,
            section: Section::Sources,
            video: devices::list(Kind::Video),
            audio: devices::list(Kind::Audio),
            sel_source: 0,
            sel_audio: 0,
            sel_dest: 0,
            google: None,
            show_keys: HashSet::new(),
            encoders: Arc::new(Mutex::new(Some({
                let list = |c: Codec| crate::engine::encode::video_backends(c.video_codec()).into_iter().map(String::from).collect::<Vec<_>>();
                (list(Codec::H264), list(Codec::H265))
            }))),
            scanning: false,
            confirm_discard: false,
            confirm_reset: false,
            confirm_remove: None,
            message: None,
            outputs: crate::platform::list_output_devices(),
            sel_graphic: 0,
            profile_name: String::new(),
            rename_to: String::new(),
            ndi_found: Arc::new(Mutex::new(None)),
            vcam_job: Arc::new(Mutex::new(None)),
            vcam_busy: false,
            learning: None,
            languages: None,
            plugin_note: None,
            pending: None,
            previews: Default::default(),
            syrin: None,
            syrin_tested: false,
            syrin_link: String::new(),
            syrin_soundset: None,
            sel_segment: 0,
            sel_macro: 0,
            apps: None,
            resolve_note: None,
            lut_check: None,
            last_tab: Default::default(),
        }
    }

    pub fn open_at(mut self, section: Section) -> Self {
        self.section = section;
        self
    }

    /// Show this source on the Video sources page.
    pub fn select_source(mut self, i: usize) -> Self {
        self.sel_source = i;
        self
    }

    pub fn dirty(&self) -> bool {
        self.draft != self.original
    }

    /// Called by the app after it tried to apply (`now`: the settings in
    /// effect).
    pub fn applied(&mut self, result: Result<(), String>, now: &Config) {
        match result {
            Ok(()) => {
                // New sources and inputs got their identities: keep them, so
                // applying again finds them rather than opening them anew.
                if self.draft.sources.len() == now.sources.len() {
                    for (d, n) in self.draft.sources.iter_mut().zip(&now.sources) {
                        d.uid = n.uid;
                    }
                }
                if self.draft.audio.len() == now.audio.len() {
                    for (d, n) in self.draft.audio.iter_mut().zip(&now.audio) {
                        d.uid = n.uid;
                    }
                }
                self.original = self.draft.clone();
                self.message = Some(("Saved".into(), false));
            }
            Err(e) => self.message = Some((e, true)),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, info: &SettingsContext) -> SettingsAction {
        let mut action = SettingsAction::None;
        let mut open = true;
        if self.scanning && self.encoders.lock().unwrap().is_some() {
            self.scanning = false;
        }
        if self.scanning {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 80.0).clamp(640.0, 1040.0), (screen.height() - 80.0).clamp(420.0, 720.0));
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let body_h = size.y - 64.0;
                ui.horizontal_top(|ui| {
                    // ---- sidebar: the groups
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        ui.set_min_height(body_h);
                        let here = self.section.group();
                        for g in Group::ALL {
                            let changed = g.sections().iter().any(|s| self.section_dirty(*s));
                            let mut text = egui::text::LayoutJob::default();
                            let name = if changed { format!("{}  •\n", g.label()) } else { format!("{}\n", g.label()) };
                            text.append(&name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(15.0), if g == here { Color32::WHITE } else { Color32::from_rgb(214, 218, 226) }));
                            text.append(g.summary(), 0.0, egui::TextFormat::simple(egui::FontId::proportional(11.5), TEXT_DIM));
                            let b = egui::Button::selectable(g == here, text).min_size(vec2(190.0, 46.0));
                            if ui.add(b).clicked() && g != here {
                                self.last_tab.insert(here, self.section);
                                self.section = self.last_tab.get(&g).copied().unwrap_or(g.sections()[0]);
                            }
                            ui.add_space(2.0);
                        }
                    });
                    ui.separator();
                    // ---- content: the group's pages as tabs, then the page
                    ui.vertical(|ui| {
                        ui.set_width(size.x - 220.0);
                        let tabs = self.section.group().sections();
                        let mut tabs_h = 0.0;
                        if tabs.len() > 1 {
                            let r = ui.horizontal(|ui| {
                                for &t in tabs {
                                    let text = if self.section_dirty(t) { format!("{}  •", t.label()) } else { t.label().to_string() };
                                    let on = self.section == t;
                                    let b = egui::Button::selectable(on, RichText::new(text).size(13.5)).min_size(vec2(0.0, 28.0));
                                    if ui.add(b).clicked() {
                                        self.section = t;
                                    }
                                }
                            });
                            ui.add_space(2.0);
                            ui.separator();
                            tabs_h = r.response.rect.height() + 10.0;
                        }
                        egui::ScrollArea::vertical().id_salt(self.section).max_height(body_h - tabs_h).auto_shrink([false, false]).show(
                            ui,
                            |ui| {
                                ui.add_space(4.0);
                                help::set_page(Some(self.section));
                                match self.section {
                                    Section::General => self.general(ui, info),
                                    Section::Sources => self.sources(ui, info),
                                    Section::Audio => self.audio_inputs(ui),
                                    Section::Mixing => self.mixing(ui),
                                    Section::Pads => self.pads(ui),
                                    Section::Syrinscape => self.syrinscape(ui),
                                    Section::Program => self.program(ui),
                                    Section::Switching => self.switching(ui, info),
                                    Section::Graphics => self.graphics(ui),
                                    Section::Rundown => self.rundown(ui),
                                    Section::Macros => self.macros(ui),
                                    Section::Prompter => self.prompter(ui),
                                    Section::Comments => self.comments(ui),
                                    Section::Guests => self.guests(ui, info),
                                    Section::Captions => self.captions(ui, info),
                                    Section::Recording => self.recording(ui),
                                    Section::Streaming => self.streaming(ui),
                                    Section::Vertical => self.vertical(ui),
                                    Section::Upload => self.upload(ui),
                                    Section::Outputs => self.outputs(ui, info),
                                    Section::Control => self.control(ui, info),
                                    Section::Shortcuts => shortcuts(ui),
                                    Section::Diagnostics => self.diagnostics(ui, info),
                                }
                                help::set_page(None);
                                ui.add_space(12.0);
                            },
                        );
                    });
                });
                ui.separator();
                // ---- bottom bar
                let valid = self.draft.validate();
                let restart = self.original.needs_restart(&self.draft);
                let live = info.recording || info.streaming;
                let blocked = restart && live;
                let lineup = !restart && self.original.lineup_changed(&self.draft);
                ui.horizontal(|ui| {
                    if let Err(e) = &valid {
                        ui.label(RichText::new(format!("⚠ {e}")).color(AMBER));
                    } else if blocked {
                        ui.label(RichText::new("Stop recording and streaming to change the program's size or frame rate.").color(AMBER));
                    } else if restart {
                        ui.label(RichText::new("Applying restarts the video engine (previews blink for a moment).").color(TEXT_DIM));
                    } else if lineup && live {
                        ui.label(RichText::new("Unsaved changes. Sources you added, removed or changed update on Apply; the show carries on.").color(TEXT_DIM));
                    } else if lineup {
                        ui.label(RichText::new("Unsaved changes. Sources you added, removed or changed update on Apply; the rest carries on.").color(TEXT_DIM));
                    } else if self.dirty() {
                        ui.label(RichText::new("Unsaved changes. These apply without interrupting anything.").color(TEXT_DIM));
                    } else if let Some((m, err)) = &self.message {
                        ui.label(RichText::new(m).color(if *err { RED } else { GREEN }));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_apply = self.dirty() && valid.is_ok() && !blocked;
                        if ui.add_enabled(can_apply || (!self.dirty() && valid.is_ok()), egui::Button::new("OK").min_size(vec2(72.0, 28.0))).clicked() {
                            action = if self.dirty() { self.apply(true, info) } else { SettingsAction::Close };
                        }
                        if ui.add_enabled(can_apply, egui::Button::new("Apply").min_size(vec2(72.0, 28.0))).clicked() {
                            action = self.apply(false, info);
                        }
                        if ui.button("Cancel").clicked() {
                            if self.dirty() {
                                self.confirm_discard = true;
                            } else {
                                action = SettingsAction::Close;
                            }
                        }
                        if ui.add_enabled(self.dirty(), egui::Button::new("Revert")).on_hover_text("Undo every change since the last save").clicked() {
                            self.draft = self.original.clone();
                        }
                    });
                });
            });
        if let Some(a) = self.pending.take() {
            action = a;
        }
        if let Some(a) = self.confirm_remove_window(ctx) {
            action = a;
        }
        if (!open || esc) && !self.confirm_discard && !self.confirm_reset && self.confirm_remove.is_none() {
            if self.dirty() {
                self.confirm_discard = true;
            } else {
                action = SettingsAction::Close;
            }
        }
        if self.confirm_discard {
            let r = egui::Modal::new(egui::Id::unique("discard_settings")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("Discard your changes?");
                ui.label("Your edits to the settings haven't been applied.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard").clicked() {
                        action = SettingsAction::Close;
                        self.confirm_discard = false;
                    }
                    if ui.button("Keep editing").clicked() {
                        self.confirm_discard = false;
                    }
                });
            });
            if r.should_close() {
                self.confirm_discard = false;
            }
        }
        if self.confirm_reset {
            let r = egui::Modal::new(egui::Id::unique("reset_settings")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Reset everything?");
                ui.label("All settings go back to defaults, including sources, audio inputs, scenes and stream destinations. You can still Revert before applying.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        let mut fresh = Config::default();
                        fresh.output.folder = self.draft.output.folder.clone();
                        self.draft = fresh;
                        self.confirm_reset = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_reset = false;
                    }
                });
            });
            if r.should_close() {
                self.confirm_reset = false;
            }
        }
        action
    }

    /// Apply the draft, unless that takes sources or inputs out of a show
    /// that's recording or live: then ask first.
    fn apply(&mut self, close: bool, info: &SettingsContext) -> SettingsAction {
        if info.recording || info.streaming {
            let (names, on_air) = self.leaving(&info.on_air);
            if !names.is_empty() {
                self.confirm_remove = Some(RemoveCheck { names, on_air, recording: info.recording, streaming: info.streaming, close });
                return SettingsAction::None;
            }
        }
        SettingsAction::Apply { cfg: self.draft.clone(), close }
    }

    /// The sources and inputs applying would remove, and which of them are
    /// on the program now.
    fn leaving(&self, on_air: &[u64]) -> (Vec<String>, Vec<String>) {
        let video: HashSet<u64> = self.draft.sources.iter().map(|s| s.uid).collect();
        let audio: HashSet<u64> = self.draft.audio.iter().map(|a| a.uid).collect();
        let gone_video: Vec<&SourceConfig> = self.original.sources.iter().filter(|s| s.uid != 0 && !video.contains(&s.uid)).collect();
        let mut names: Vec<String> = gone_video.iter().map(|s| s.name.clone()).collect();
        for a in self.original.audio.iter().filter(|a| a.uid != 0 && !audio.contains(&a.uid)) {
            // (A guest spot's input shares its source's name.)
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&a.name)) {
                names.push(a.name.clone());
            }
        }
        let live = gone_video.iter().filter(|s| on_air.contains(&s.uid)).map(|s| s.name.clone()).collect();
        (names, live)
    }

    fn confirm_remove_window(&mut self, ctx: &egui::Context) -> Option<SettingsAction> {
        let check = self.confirm_remove.as_ref()?;
        let mut action = None;
        let mut done = false;
        let what = match (check.recording, check.streaming) {
            (true, true) => "recording and live",
            (true, false) => "recording",
            _ => "live",
        };
        let r = egui::Modal::new(egui::Id::unique("confirm_remove_sources")).show(ctx, |ui| {
            ui.set_width(420.0);
            let one = check.names.len() == 1;
            ui.heading(format!("Remove {} while {what}?", if one { "this" } else { "these" }));
            ui.add_space(4.0);
            ui.label(format!("{} {} the show as soon as you apply.", list_names(&check.names), if one { "leaves" } else { "leave" }));
            if check.recording {
                ui.label(if one { "Its recording ends there; everything else keeps recording." } else { "Their recordings end there; everything else keeps recording." });
            }
            if !check.on_air.is_empty() {
                ui.add_space(4.0);
                let on = list_names(&check.on_air);
                let is = if check.on_air.len() == 1 { "is" } else { "are" };
                ui.label(RichText::new(format!("⚠ {on} {is} on the program right now: take another scene first, or that part of the picture goes empty.")).color(AMBER));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Remove and apply").color(RED)).clicked() {
                    action = Some(SettingsAction::Apply { cfg: self.draft.clone(), close: check.close });
                    done = true;
                }
                if ui.button("Keep editing").clicked() {
                    done = true;
                }
            });
        });
        if done || r.should_close() {
            self.confirm_remove = None;
        }
        action
    }

    fn section_dirty(&self, s: Section) -> bool {
        let (a, b) = (&self.draft, &self.original);
        match s {
            Section::General => a.ui != b.ui,
            Section::Sources => a.sources != b.sources,
            Section::Audio => a.audio != b.audio,
            Section::Mixing => a.mix != b.mix,
            Section::Pads => a.pads != b.pads,
            Section::Syrinscape => a.syrinscape != b.syrinscape,
            Section::Rundown => a.rundown != b.rundown,
            Section::Macros => a.macros != b.macros,
            Section::Prompter => a.prompter != b.prompter,
            Section::Comments => a.comments != b.comments,
            Section::Guests => a.guests != b.guests,
            Section::Program => a.program != b.program || a.style != b.style,
            Section::Switching => a.transition != b.transition || a.auto_switch != b.auto_switch || a.replay != b.replay,
            Section::Graphics => a.graphics != b.graphics,
            Section::Captions => a.captions != b.captions,
            Section::Recording => a.output != b.output || a.podcast != b.podcast,
            Section::Streaming => a.streaming != b.streaming,
            Section::Vertical => a.vertical != b.vertical,
            Section::Upload => a.upload != b.upload,
            Section::Outputs => a.display != b.display || a.ndi_out != b.ndi_out,
            Section::Control => a.remote != b.remote || a.midi != b.midi || a.osc != b.osc,
            _ => false,
        }
    }

    // ---- General ------------------------------------------------------------------

    fn general(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "General & shows", "The window, safety checks, and saved shows.");
        egui::Grid::new("general").num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
            label(ui, "Interface size");
            ui.add(egui::Slider::new(&mut self.draft.ui.scale, 0.75..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            ui.end_row();
            label(ui, "Ask before");
            ui.vertical(|ui| {
                ui.checkbox(&mut self.draft.ui.confirm_stop_recording, "stopping a recording");
                ui.checkbox(&mut self.draft.ui.confirm_end_stream, "ending a live stream");
            });
            ui.end_row();
            label(ui, "Pre-show check");
            ui.checkbox(&mut self.draft.ui.check_before_start, "before recording or going live");
            ui.end_row();
            label(ui, "Updates");
            ui.checkbox(&mut self.draft.ui.check_updates, "look for a newer Cutaway when it starts");
            ui.end_row();
        });
        ui.add_space(14.0);
        self.shows(ui, info);
        ui.add_space(14.0);
        subtitle(ui, "Settings file");
        if let Some(p) = &info.config_path {
            ui.label(RichText::new(p.display().to_string()).monospace().color(TEXT_DIM));
        } else {
            hint(ui, "Demo mode: settings aren't saved.");
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Export settings…").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Cutaway settings", &["toml"]).set_file_name("cutaway.toml").save_file() {
                    self.message = Some(match self.draft.save(&path) {
                        Ok(()) => (format!("Exported to {}", path.display()), false),
                        Err(e) => (format!("Export failed: {e:#}"), true),
                    });
                }
            }
            if ui.button("Import settings…").on_hover_text("Loads into this window; nothing changes until you apply").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Cutaway settings", &["toml"]).pick_file() {
                    self.message = Some(match Config::load(&path) {
                        Ok(cfg) => {
                            self.draft = cfg;
                            (format!("Imported {} — review and apply", path.display()), false)
                        }
                        Err(e) => (format!("Couldn't import: {e:#}"), true),
                    });
                }
            }
            if ui.button("Reset to defaults…").clicked() {
                self.confirm_reset = true;
            }
        });
    }

    // ---- Audio --------------------------------------------------------------------

    fn audio_inputs(&mut self, ui: &mut egui::Ui) {
        title(
            ui,
            "Audio inputs",
            "Microphones, interfaces and other sound. Each input can be saved as a WAV and feed the mixes.",
        );
        ui.horizontal(|ui| {
            if ui.button("Rescan devices").clicked() {
                self.audio = devices::list(Kind::Audio);
            }
            hint(ui, &format!("{} audio input(s) found", self.audio.len()));
            ui.separator();
            let unsaved = self.draft != self.original;
            let b = ui.add_enabled(!unsaved, egui::Button::new("Measure lip sync…"));
            if b.on_hover_text(if unsaved { "Apply your changes first" } else { "Clap once: Cutaway works out the audio and video delays" }).clicked() {
                self.pending = Some(SettingsAction::MeasureSync);
            }
        });
        ui.add_space(6.0);
        let n = self.draft.audio.len();
        if self.sel_audio >= n {
            self.sel_audio = n.saturating_sub(1);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for i in 0..n {
                    let a = &self.draft.audio[i];
                    let b = egui::Button::selectable(
                        self.sel_audio == i,
                        RichText::new(format!("{}\n{} · {} ch", a.name, short_audio_device(&a.device), a.channels)).size(13.0),
                    )
                    .min_size(vec2(220.0, 40.0));
                    if ui.add(b).clicked() {
                        self.sel_audio = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("＋ Add").clicked() {
                        let device = self.audio.first().map(|d| d.name.clone()).unwrap_or("default".into());
                        let base = if self.draft.audio.is_empty() { "DAW" } else { "Input" };
                        let name = unique(self.draft.audio.iter().map(|a| a.name.clone()).collect(), base);
                        self.draft.audio.push(AudioConfig { name, device, ..Default::default() });
                        self.sel_audio = self.draft.audio.len() - 1;
                    }
                    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
                        self.draft.audio.remove(self.sel_audio);
                    }
                });
            });
            ui.add_space(14.0);
            let sound_sources: Vec<String> = self
                .draft
                .sources
                .iter()
                .filter(|s| {
                    let d = s.device.trim().to_lowercase();
                    d == "media" || d.starts_with("ndi:") || crate::engine::netsrc::is_network(&d)
                })
                .map(|s| s.name.clone())
                .collect();
            let ducking_on = self.draft.mix.ducking;
            ui.vertical(|ui| {
                let Some(a) = self.draft.audio.get_mut(self.sel_audio) else {
                    hint(ui, "No audio inputs. Add your interface to record your DAW.");
                    return;
                };
                let devices = &self.audio;
                let max = devices.iter().find(|d| d.name == a.device).map(|d| d.max_channels.max(1)).unwrap_or(64).min(64);
                egui::Grid::new("audio_detail").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    label(ui, "Name");
                    ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut a.name));
                    ui.end_row();
                    label(ui, "Device");
                    ui.vertical(|ui| {
                        let opened = audio_device_combo(ui, &mut a.device, devices, &sound_sources, self.apps.as_deref());
                        if opened && self.apps.is_none() {
                            self.apps = Some(crate::platform::app_audio::list());
                        }
                        if a.device.starts_with("app:") {
                            hint(ui, if cfg!(target_os = "macos") { "Needs Screen Recording permission (System Settings → Privacy & Security)." } else { "The app is found by name each time Cutaway starts." });
                        }
                    });
                    ui.end_row();
                    let stereo_only = a.device.starts_with("source:") || a.is_pads() || a.device.starts_with("app:");
                    if stereo_only {
                        a.channels = 2;
                    }
                    label(ui, "Channels to capture");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut a.channels).range(1..=max));
                        if max < 64 {
                            hint(ui, &format!("device offers up to {max}"));
                        }
                    });
                    ui.end_row();
                    a.program_left = a.program_left.clamp(1, a.channels);
                    a.program_right = a.program_right.clamp(1, a.channels);
                    label(ui, "Program left / right");
                    ui.horizontal(|ui| {
                        channel_combo(ui, "left", &mut a.program_left, a.channels);
                        channel_combo(ui, "right", &mut a.program_right, a.channels);
                        if a.program_left == a.program_right {
                            hint(ui, "(mono to both sides)");
                        }
                    });
                    ui.end_row();
                    label(ui, "Gain");
                    let mut g = a.gain_db as f32;
                    if ui.add(egui::Slider::new(&mut g, -40.0..=12.0).suffix(" dB").step_by(0.5)).changed() {
                        a.gain_db = g as f64;
                    }
                    ui.end_row();
                    label(ui, "On");
                    ui.checkbox(&mut a.in_program, "heard in the mixes");
                    ui.end_row();
                    label(ui, "Heard in");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut a.to_recording, "the recording");
                        ui.checkbox(&mut a.to_stream, "the live stream");
                    });
                    ui.end_row();
                    label(ui, "Duck under voices");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut a.duck, "turn down while others talk");
                        if a.duck && !ducking_on {
                            hint(ui, "(ducking is off in Mixing & headphones)");
                        }
                    });
                    ui.end_row();
                    label(ui, "Record to WAV");
                    ui.checkbox(&mut a.record, "");
                    ui.end_row();
                    label(ui, "One file per channel");
                    ui.checkbox(&mut a.split_channels, "mono WAVs");
                    ui.end_row();
                    label(ui, "Delay");
                    ui.add(egui::DragValue::new(&mut a.delay_ms).range(0..=2000).suffix(" ms"));
                    ui.end_row();
                });
                ui.add_space(10.0);
                let state = mixing::processing_summary(&a.processing);
                part(ui, ("fx", self.sel_audio), "Voice processing", &state, a.processing.enabled, |ui| mixing::processing_editor(ui, &mut a.processing));
            });
        });
    }

    // ---- Program ------------------------------------------------------------------

    fn program(&mut self, ui: &mut egui::Ui) {
        title(ui, "Program", "The switched picture you record and stream, and how frames look in it.");
        let p = &mut self.draft.program;
        egui::Grid::new("program").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Size");
            ui.horizontal(|ui| {
                size_combo(ui, "prog_size", &mut p.width, &mut p.height);
                ui.add(egui::DragValue::new(&mut p.width).range(160..=7680).speed(2));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut p.height).range(90..=4320).speed(2));
            });
            ui.end_row();
            label(ui, "Frame rate");
            fps_picker(ui, "prog_fps", &mut p.fps, None);
            ui.end_row();
            label(ui, "Background colour");
            let mut rgb = crate::config::parse_color(&p.background_color)
                .map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
                .unwrap_or([16, 19, 26]);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                p.background_color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            }
            ui.end_row();
            label(ui, "Background image");
            ui.horizontal(|ui| {
                let mut text = p.background_image.as_ref().map(|x| x.display().to_string()).unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut text).hint_text("none").desired_width(300.0)).changed() {
                    p.background_image = if text.trim().is_empty() { None } else { Some(PathBuf::from(text.trim())) };
                }
                if ui.button("Choose…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                        p.background_image = Some(f);
                    }
                }
                if p.background_image.is_some() && ui.button("Clear").clicked() {
                    p.background_image = None;
                }
            });
            ui.end_row();
            if let Some(img) = &p.background_image {
                if !img.exists() {
                    ui.label("");
                    ui.label(RichText::new("⚠ That file doesn't exist; the colour will be used.").color(AMBER));
                    ui.end_row();
                }
            }
            label(ui, "Program monitor quality");
            ui.add(egui::Slider::new(&mut p.preview_width, 480..=1920).step_by(32.0).suffix(" px wide"));
            ui.end_row();
            label(ui, "Multiview quality");
            ui.add(egui::Slider::new(&mut p.multiview_width, 160..=960).step_by(32.0).suffix(" px wide"));
            ui.end_row();
        });
        ui.add_space(14.0);
        subtitle(ui, "Frames that don't fill the screen");
        let st = &mut self.draft.style;
        egui::Grid::new("style").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Rounded corners");
            ui.add(egui::Slider::new(&mut st.corner_radius, 0.0..=80.0).suffix(" px"));
            ui.end_row();
            label(ui, "Border");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut st.border_width, 0.0..=20.0).suffix(" px"));
                let mut rgb = crate::config::parse_color(&st.border_color).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or([255, 255, 255]);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    st.border_color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
            });
            ui.end_row();
            label(ui, "Drop shadow");
            ui.add(egui::Slider::new(&mut st.shadow, 0.0..=1.0));
            ui.end_row();
        });
    }

    // ---- Recording ----------------------------------------------------------------

    fn recording(&mut self, ui: &mut egui::Ui) {
        title(ui, "Recording", "Where takes go, how they're encoded, and what's saved with them for editing.");
        self.poll_encoder_scan();
        let scan = self.encoders.lock().unwrap().clone();
        let o = &mut self.draft.output;
        let (pw, ph, pfps) = (self.draft.program.width, self.draft.program.height, self.draft.program.fps);
        egui::Grid::new("recording").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Save takes in");
            ui.horizontal(|ui| {
                let mut folder = o.folder.display().to_string();
                if ui.add(egui::TextEdit::singleline(&mut folder).desired_width(340.0)).changed() {
                    o.folder = PathBuf::from(folder);
                }
                if ui.button("Choose…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().set_directory(&o.folder).pick_folder() {
                        o.folder = f;
                    }
                }
            });
            ui.end_row();
            label(ui, "Take name");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut o.take_prefix).desired_width(160.0));
                hint(ui, &format!("→ {}_2026-10-16_19-30-00", crate::util::safe_file_name(if o.take_prefix.trim().is_empty() { "Take" } else { &o.take_prefix })));
            });
            ui.end_row();
            label(ui, "Backup copy");
            ui.horizontal(|ui| {
                let mut text = o.backup_folder.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut text).hint_text("off — choose a folder on another drive").desired_width(300.0)).changed() {
                    o.backup_folder = if text.trim().is_empty() { None } else { Some(PathBuf::from(text.trim())) };
                }
                if ui.button("Choose…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().pick_folder() {
                        o.backup_folder = Some(f);
                    }
                }
                if o.backup_folder.is_some() && ui.button("Off").clicked() {
                    o.backup_folder = None;
                }
            });
            ui.end_row();
            label(ui, "Instant clips");
            ui.horizontal(|ui| {
                ui.label("the Clip button saves the last");
                ui.add(egui::DragValue::new(&mut o.clip_seconds).range(5..=600).suffix(" s"));
            });
            ui.end_row();
            label(ui, "Crash protection");
            hint(ui, "always on");
            ui.end_row();
        });
        {
            let cfg = &self.draft;
            let rate = crate::engine::health::estimate_rate(cfg, &cfg.audio.iter().map(|a| (a.channels, a.record)).collect::<Vec<_>>());
            let free = crate::engine::health::disk_space(&cfg.output.folder).map(|(f, _)| f);
            ui.add_space(4.0);
            hint(ui, &format!(
                "These settings write about {}/hour{}.",
                crate::util::human_size((rate * 3600.0) as u64),
                free.map(|f| format!(" — the drive has room for about {}", crate::engine::health::fmt_left(f as f64 / rate.max(1.0)))).unwrap_or_default()
            ));
            if let Some(b) = cfg.output.backup_folder.as_ref() {
                if b == &cfg.output.folder {
                    ui.label(RichText::new("⚠ The backup folder is the recording folder; pick a folder on another drive.").color(AMBER));
                }
            }
        }
        ui.add_space(10.0);

        let o = &mut self.draft.output;
        let quality = format!(
            "{} · {} · {}",
            if o.container == Container::Mov || o.codec.is_prores() { "MOV" } else { "MP4" },
            crate::engine::encode::codec_name(o.codec.video_codec()),
            if o.codec.is_prores() || (o.iso_bitrate_kbps == 0 && o.program_bitrate_kbps == 0) { "automatic quality".to_string() } else { "set bitrates".to_string() }
        );
        part(ui, "rec_quality", "Formats and quality", &quality, false, |ui| {
            egui::Grid::new("rec_quality_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "File format");
                egui::ComboBox::from_id_salt("container").selected_text(container_label(o.container)).width(260.0).show_ui(ui, |ui| {
                    for c in [Container::Mp4, Container::Mov] {
                        ui.selectable_value(&mut o.container, c, container_label(c));
                    }
                });
                ui.end_row();
                label(ui, "Codec");
                egui::ComboBox::from_id_salt("codec").selected_text(crate::engine::encode::codec_name(o.codec.video_codec())).width(260.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.codec, Codec::H264, "H.264 — opens everywhere");
                    ui.selectable_value(&mut o.codec, Codec::H265, "H.265/HEVC — smaller 4K files");
                    ui.separator();
                    let prores_ok = crate::engine::encode::codec_problem(Codec::Prores.video_codec()).is_none();
                    for (c, text) in [
                        (Codec::ProresProxy, "ProRes 422 Proxy — light editing copies"),
                        (Codec::ProresLt, "ProRes 422 LT"),
                        (Codec::Prores, "ProRes 422 — smooth editing, big files"),
                        (Codec::ProresHq, "ProRes 422 HQ — highest quality"),
                    ] {
                        ui.add_enabled_ui(prores_ok, |ui| {
                            ui.selectable_value(&mut o.codec, c, text);
                        });
                    }
                    if !prores_ok {
                        ui.label(RichText::new("ProRes needs a Mac").color(TEXT_DIM).size(11.5));
                    }
                });
                ui.end_row();
                if o.codec.is_prores() {
                    ui.label("");
                    hint(ui, &format!(
                        "ProRes is recorded as MOV, at about {} GB an hour for the program alone.",
                        crate::engine::encode::auto_bitrate_kbps(o.codec.video_codec(), pw, ph, pfps) as u64 * 3600 / 8 / 1_000_000
                    ));
                    ui.end_row();
                }
                label(ui, "Encoder");
                let list = scan.as_ref().map(|(h264, h265)| if o.codec == Codec::H264 { h264.clone() } else { h265.clone() });
                encoder_combo(ui, "rec_encoder", &mut o.encoder, list.as_deref());
                ui.end_row();
                if !o.codec.is_prores() {
                    label(ui, "Source file bitrate");
                    bitrate_field(ui, &mut o.iso_bitrate_kbps, "set from each source's size and rate");
                    ui.end_row();
                    label(ui, "Program bitrate");
                    let auto = crate::engine::encode::auto_bitrate_kbps(o.codec.video_codec(), pw, ph, pfps);
                    bitrate_field(ui, &mut o.program_bitrate_kbps, &format!("{} Mbps here", auto / 1000));
                    ui.end_row();
                }
                label(ui, "Audio in video files");
                egui::ComboBox::from_id_salt("abr").selected_text(format!("AAC {} kbps", o.audio_bitrate_kbps)).show_ui(ui, |ui| {
                    for r in [128, 160, 192, 256, 320] {
                        ui.selectable_value(&mut o.audio_bitrate_kbps, r, format!("AAC {r} kbps"));
                    }
                });
                ui.end_row();
                label(ui, "Program audio in source files");
                ui.checkbox(&mut o.iso_audio, "the recording mix");
                ui.end_row();
                label(ui, "WAV files");
                egui::ComboBox::from_id_salt("wav").selected_text(format!("{}-bit{}", o.wav_bit_depth, if o.wav_bit_depth == 32 { " float" } else { "" })).show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.wav_bit_depth, 16, "16-bit");
                    ui.selectable_value(&mut o.wav_bit_depth, 24, "24-bit");
                    ui.selectable_value(&mut o.wav_bit_depth, 32, "32-bit float");
                });
                ui.end_row();
            });
        });

        let podcast_on = self.draft.podcast.enabled;
        let o = &mut self.draft.output;
        let extras: Vec<&str> = [
            (o.timecode, "timecode"),
            (o.export_timelines, "timelines"),
            (o.daw_sessions, "audio sessions"),
            (o.resolve_project, "Resolve"),
            (o.proxies, "proxies"),
            (o.input_tracks, "input tracks"),
            (o.mix_wav || podcast_on, "mix WAV"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| n)
        .collect();
        let state = if extras.is_empty() { "nothing extra".to_string() } else { extras.join(", ") };
        part(ui, "rec_editing", "For editing", &state, false, |ui| {
            egui::Grid::new("rec_editing_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Timecode");
                ui.checkbox(&mut o.timecode, "the time of day, in every file");
                ui.end_row();
                label(ui, "Editor timelines");
                ui.checkbox(&mut o.export_timelines, "Final Cut Pro, DaVinci Resolve and Premiere Pro");
                ui.end_row();
                label(ui, "Audio sessions");
                ui.checkbox(&mut o.daw_sessions, "REAPER and DAWproject");
                ui.end_row();
                label(ui, "DaVinci Resolve");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut o.resolve_project, "a project from each take");
                        if ui.button("Install the Resolve script").on_hover_text("Adds \"Cutaway - Build project from a take\" to Resolve's Workspace → Scripts menu").clicked() {
                            self.resolve_note = Some(match crate::engine::sessions::install_resolve_script() {
                                Ok(p) => (format!("Installed in {}. In Resolve: Workspace → Scripts → Cutaway - Build project from a take.", p.display()), false),
                                Err(e) => (format!("Couldn't install it: {e:#}"), true),
                            });
                        }
                    });
                    if let Some((note, err)) = &self.resolve_note {
                        ui.label(RichText::new(note).color(if *err { AMBER } else { GREEN }).size(12.0));
                    }
                });
                ui.end_row();
                label(ui, "Proxy files");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut o.proxies, "small copies, up to");
                    ui.add_enabled(o.proxies, egui::DragValue::new(&mut o.proxy_width).range(320..=1920).speed(16).suffix(" px wide"));
                });
                ui.end_row();
                label(ui, "Each input on its own track");
                ui.checkbox(&mut o.input_tracks, "in the Program file");
                ui.end_row();
                if o.input_tracks {
                    label(ui, "Input tracks");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut o.input_tracks_post_fader, false, "before the fader (clean)");
                        ui.selectable_value(&mut o.input_tracks_post_fader, true, "as heard in the mix");
                    });
                    ui.end_row();
                }
                label(ui, "Mix WAV");
                if podcast_on {
                    ui.add_enabled(false, egui::Checkbox::new(&mut true.clone(), "saved (the podcast copy is made from it)"));
                } else {
                    ui.checkbox(&mut o.mix_wav, "Mix.wav");
                }
                ui.end_row();
            });
        });

        let p = &mut self.draft.podcast;
        let state = if p.enabled { format!("on · {} LUFS", p.target_lufs) } else { "off".to_string() };
        part(ui, "rec_podcast", "Podcast", &state, false, |ui| {
            egui::Grid::new("podcast").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Podcast copy");
                ui.checkbox(&mut p.enabled, "an .m4a of the mix after each take");
                ui.end_row();
                if p.enabled {
                    label(ui, "Loudness");
                    ui.horizontal(|ui| {
                        for (v, t) in [(-16.0, "−16 LUFS (Apple, Spotify)"), (-14.0, "−14"), (-19.0, "−19 (mono speech)")] {
                            ui.selectable_value(&mut p.target_lufs, v, t);
                        }
                    });
                    ui.end_row();
                    label(ui, "Channels");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut p.mono, false, "stereo");
                        ui.selectable_value(&mut p.mono, true, "mono");
                        egui::ComboBox::from_id_salt("pod_br").selected_text(format!("{} kbps", p.bitrate_kbps)).show_ui(ui, |ui| {
                            for r in [64, 96, 128, 160, 192, 256] {
                                ui.selectable_value(&mut p.bitrate_kbps, r, format!("{r} kbps"));
                            }
                        });
                    });
                    ui.end_row();
                    label(ui, "Podcast name");
                    ui.add(egui::TextEdit::singleline(&mut p.show).hint_text("the show's name").desired_width(260.0));
                    ui.end_row();
                    label(ui, "Episode title");
                    ui.add(egui::TextEdit::singleline(&mut p.title).hint_text("blank: podcast name and date").desired_width(260.0));
                    ui.end_row();
                    label(ui, "Author");
                    ui.add(egui::TextEdit::singleline(&mut p.author).desired_width(260.0));
                    ui.end_row();
                }
            });
        });
    }

    // ---- Streaming ----------------------------------------------------------------

    fn streaming(&mut self, ui: &mut egui::Ui) {
        title(ui, "Streaming", "Go live to any number of places at once: the program is encoded once and sent to each.");
        self.poll_encoder_scan();
        let scan = self.encoders.lock().unwrap().clone();
        let n = self.draft.streaming.destinations.len();
        if self.sel_dest >= n {
            self.sel_dest = n.saturating_sub(1);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for i in 0..n {
                    let d = &mut self.draft.streaming.destinations[i];
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.enabled, "").on_hover_text("Include when going live");
                        let warn = if d.problem().is_some() { "  ⚠" } else { "" };
                        let b = egui::Button::selectable(
                            self.sel_dest == i,
                            RichText::new(format!("{}{warn}\n{}", d.name, d.service.label())).size(13.0),
                        )
                        .min_size(vec2(186.0, 40.0));
                        if ui.add(b).clicked() {
                            self.sel_dest = i;
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("add_dest").selected_text("＋ Add").width(90.0).show_ui(ui, |ui| {
                        for s in Service::ALL {
                            if ui.selectable_label(false, s.label()).clicked() {
                                let names = self.draft.streaming.destinations.iter().map(|d| d.name.clone()).collect();
                                let base = match s {
                                    Service::CustomRtmp => "Custom",
                                    other => other.label(),
                                };
                                self.draft.streaming.destinations.push(StreamDestination {
                                    name: unique(names, base),
                                    service: s,
                                    server: s.default_server().into(),
                                    key: String::new(),
                                    enabled: true,
                                    vertical: false,
                                });
                                self.sel_dest = self.draft.streaming.destinations.len() - 1;
                            }
                        }
                    });
                    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
                        self.draft.streaming.destinations.remove(self.sel_dest);
                        self.show_keys.clear();
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let sel = self.sel_dest;
                let Some(d) = self.draft.streaming.destinations.get_mut(sel) else {
                    hint(ui, "Add YouTube, Twitch, Facebook, Kick, any RTMP server, or an SRT receiver.");
                    return;
                };
                egui::Grid::new("dest_detail").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    label(ui, "Name");
                    ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut d.name));
                    ui.end_row();
                    label(ui, "Service");
                    let before = d.service;
                    egui::ComboBox::from_id_salt("service").selected_text(d.service.label()).width(240.0).show_ui(ui, |ui| {
                        for s in Service::ALL {
                            ui.selectable_value(&mut d.service, s, s.label());
                        }
                    });
                    if d.service != before {
                        let was_default = d.server.trim().is_empty() || d.server == before.default_server();
                        if was_default {
                            d.server = d.service.default_server().into();
                        }
                        if d.name == before.label() {
                            d.name = d.service.label().into();
                        }
                    }
                    ui.end_row();
                    label(ui, if d.service.is_srt() { "Address" } else { "Server" });
                    ui.add(egui::TextEdit::singleline(&mut d.server).desired_width(380.0).hint_text(d.service.default_server()));
                    ui.end_row();
                    label(ui, if d.service.is_srt() { "Stream ID (optional)" } else { "Stream key" });
                    ui.horizontal(|ui| {
                        let show = self.show_keys.contains(&sel);
                        ui.add(egui::TextEdit::singleline(&mut d.key).password(!show).desired_width(300.0));
                        if ui.button(if show { "Hide" } else { "Show" }).clicked() {
                            if show {
                                self.show_keys.remove(&sel);
                            } else {
                                self.show_keys.insert(sel);
                            }
                        }
                    });
                    ui.end_row();
                    label(ui, "Go live here");
                    ui.checkbox(&mut d.enabled, "");
                    ui.end_row();
                    label(ui, "Sends");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.vertical, false, "the program");
                        ui.radio_value(&mut d.vertical, true, "the vertical version (9:16)");
                    });
                    ui.end_row();
                });
                if d.vertical && !self.draft.vertical.enabled {
                    ui.label(RichText::new("⚠ The vertical version is off: turn it on in Vertical version.").color(AMBER));
                }
                ui.add_space(6.0);
                hint(ui, d.service.help());
                if let Some(p) = d.problem() {
                    ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
                }
            });
        });

        ui.add_space(14.0);
        let s = &mut self.draft.streaming;
        let up = (s.video_bitrate_kbps + s.audio_bitrate_kbps) as f64 / 1000.0;
        let enabled = s.destinations.iter().filter(|d| d.enabled).count();
        let state = format!("{}×{} · {} fps · {:.1} Mbps", s.width, s.height, s.fps, up);
        part(ui, "stream_quality", "Quality", &state, false, |ui| {
            egui::Grid::new("stream_quality_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Size");
                ui.horizontal(|ui| {
                    size_combo(ui, "stream_size", &mut s.width, &mut s.height);
                    ui.add(egui::DragValue::new(&mut s.width).range(160..=3840).speed(2));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut s.height).range(90..=2160).speed(2));
                });
                ui.end_row();
                label(ui, "Frame rate");
                fps_picker(ui, "stream_fps", &mut s.fps, None);
                ui.end_row();
                label(ui, "Video bitrate");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.video_bitrate_kbps).range(500..=50_000).speed(50).suffix(" kbps"));
                    let (lo, hi) = recommended_kbps(s.width, s.height, s.fps);
                    hint(ui, &format!("typical for this size: {lo}–{hi} kbps"));
                });
                ui.end_row();
                label(ui, "Audio bitrate");
                egui::ComboBox::from_id_salt("stream_abr").selected_text(format!("AAC {} kbps", s.audio_bitrate_kbps)).show_ui(ui, |ui| {
                    for r in [96, 128, 160, 192, 256, 320] {
                        ui.selectable_value(&mut s.audio_bitrate_kbps, r, format!("AAC {r} kbps"));
                    }
                });
                ui.end_row();
                label(ui, "Encoder");
                let list = scan.as_ref().map(|(h264, _)| h264.clone());
                encoder_combo(ui, "stream_encoder", &mut s.encoder, list.as_deref());
                ui.end_row();
            });
            hint(ui, &format!("Upload needed: about {up:.1} Mbps per destination, {:.1} Mbps for the {enabled} on. Keep it under ~70% of your upload speed.", up * enabled as f64));
        });
        let s = &mut self.draft.streaming;
        let state = [(s.auto_reconnect, "reconnects"), (s.record_when_live, "records too"), (s.adaptive_bitrate, "adapts to the connection")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join(", ");
        part(ui, "stream_rel", "Going live", &state, false, |ui| {
            egui::Grid::new("stream_rel_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "If a connection drops");
                ui.checkbox(&mut s.auto_reconnect, "reconnect");
                ui.end_row();
                label(ui, "Give up after");
                ui.add_enabled(
                    s.auto_reconnect,
                    egui::DragValue::new(&mut s.max_retries).range(0..=100).custom_formatter(|v, _| {
                        if v == 0.0 { "never (keep trying)".into() } else { format!("{v} attempts") }
                    }),
                );
                ui.end_row();
                label(ui, "When going live");
                ui.checkbox(&mut s.record_when_live, "also start recording");
                ui.end_row();
                label(ui, "Slow connection");
                ui.checkbox(&mut s.adaptive_bitrate, "lower the bitrate while it can't keep up");
                ui.end_row();
            });
        });
    }

    // ---- Diagnostics ----------------------------------------------------------------

    fn diagnostics(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Diagnostics", "What the engine is using, connected devices, and the log.");
        egui::Grid::new("facts").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            label(ui, "Version");
            ui.label(format!("Cutaway {}", env!("CARGO_PKG_VERSION")));
            ui.end_row();
            label(ui, "Log file");
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::logging::path().display().to_string()).monospace().color(TEXT_DIM));
                if ui.small_button("Show").on_hover_text("Send this file along when reporting a problem").clicked() {
                    crate::logging::reveal();
                }
            });
            ui.end_row();
            for (k, v) in &info.engine_facts {
                label(ui, k);
                ui.label(v);
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            subtitle(ui, "Devices");
            if ui.small_button("Rescan").clicked() {
                self.video = devices::list(Kind::Video);
                self.audio = devices::list(Kind::Audio);
            }
        });
        for (title_text, list) in [("Video", &self.video), ("Audio", &self.audio)] {
            ui.label(RichText::new(title_text).strong());
            if list.is_empty() {
                hint(ui, "none found");
            }
            for d in list {
                ui.label(format!("• {}", d.name));
                if !d.summary.is_empty() {
                    ui.label(RichText::new(format!("    {}", d.summary)).size(11.5).color(TEXT_DIM));
                }
            }
            ui.add_space(4.0);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            subtitle(ui, "Log");
            if ui.small_button("Copy").clicked() {
                let text: Vec<String> = info.log.iter().map(|l| format!("{} {:?} {}", l.time, l.level, l.text)).collect();
                ui.ctx().copy_text(text.join("\n"));
                self.message = Some(("Log copied to the clipboard".into(), false));
            }
        });
        egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(8)).corner_radius(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if info.log.is_empty() {
                hint(ui, "empty");
            }
            for l in info.log.iter().rev().take(80) {
                let color = match l.level {
                    LogLevel::Info => TEXT_DIM,
                    LogLevel::Warning => AMBER,
                    LogLevel::Error => RED,
                };
                ui.label(RichText::new(format!("{}  {}", l.time, l.text)).monospace().size(11.0).color(color));
            }
        });
    }

    // ---- encoder detection ------------------------------------------------------------

    fn poll_encoder_scan(&mut self) {
        if self.scanning && self.encoders.lock().unwrap().is_some() {
            self.scanning = false;
        }
    }
}

// ---- widgets ---------------------------------------------------------------------------

/// What a page is for, in a line (the tab above names it).
pub(super) fn title(ui: &mut egui::Ui, _text: &str, sub: &str) {
    ui.add(egui::Label::new(RichText::new(sub).color(TEXT_DIM)).wrap());
    ui.add_space(10.0);
}

/// A part of a page that folds away, with how it's set when folded.
pub(super) fn part<R>(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, name: &str, state: &str, open: bool, body: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let mut job = egui::text::LayoutJob::default();
    job.append(name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.5), Color32::WHITE));
    if !state.is_empty() {
        job.append(&format!("   {state}"), 0.0, egui::TextFormat::simple(egui::FontId::proportional(12.5), TEXT_DIM));
    }
    let r = egui::CollapsingHeader::new(job).id_salt(id).default_open(open).show(ui, |ui| {
        ui.add_space(4.0);
        let r = body(ui);
        ui.add_space(6.0);
        r
    });
    ui.add_space(4.0);
    r.body_returned
}

pub(super) fn subtitle(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.5).strong().color(TEXT_DIM).extra_letter_spacing(1.0));
    ui.add_space(2.0);
}

/// The width every setting's name takes, so rows line up down a page.
const LABEL_W: f32 = 176.0;

/// A setting's name, with an ⓘ that explains it (on hover) when there's
/// more to say.
pub(super) fn label(ui: &mut egui::Ui, text: &str) {
    let help = help::get(text);
    ui.allocate_ui_with_layout(vec2(LABEL_W, 20.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_width(LABEL_W);
        ui.spacing_mut().item_spacing.x = 5.0;
        let r = ui.add(egui::Label::new(RichText::new(text).color(Color32::from_rgb(200, 205, 214))).selectable(false).sense(egui::Sense::hover()));
        if let Some(h) = help {
            // The name explains itself after a moment; the ⓘ straight away.
            r.on_hover_ui(|ui| help_text(ui, h));
            let i = info_icon(ui);
            if i.hovered() {
                egui::Tooltip::for_widget(&i).gap(8.0).show(|ui| help_text(ui, h));
            }
        }
    });
}

fn help_text(ui: &mut egui::Ui, text: &str) {
    ui.set_max_width(340.0);
    ui.label(RichText::new(text).size(13.0));
}

/// A table's column heading, explained on hover like a setting's name.
pub(super) fn column_head(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(text).color(TEXT_DIM).size(12.0));
        if let Some(h) = help::get(text) {
            let i = info_icon(ui);
            if i.hovered() {
                egui::Tooltip::for_widget(&i).gap(8.0).show(|ui| help_text(ui, h));
            }
        }
    });
}

/// A small ⓘ.
pub(super) fn info_icon(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
    let c = if resp.hovered() { ACCENT } else { Color32::from_rgb(120, 128, 142) };
    let p = ui.painter();
    p.circle_stroke(rect.center(), 6.0, egui::Stroke::new(1.1, c));
    p.text(rect.center() + vec2(0.0, 0.5), egui::Align2::CENTER_CENTER, "i", egui::FontId::proportional(10.0), c);
    resp
}

/// A checkbox that explains itself on hover (when there's more to say).
pub(super) fn check(ui: &mut egui::Ui, value: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let text: egui::WidgetText = text.into();
    let help = help::get(text.text());
    let r = ui.checkbox(value, text);
    match help {
        Some(h) => r.on_hover_ui(|ui| help_text(ui, h)),
        None => r,
    }
}

pub(super) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(TEXT_DIM)).wrap());
}

pub(super) fn short_audio_device(device: &str) -> String {
    match device {
        "test" => "Test tone".into(),
        d => short_device(d),
    }
}

pub(super) fn short_device(device: &str) -> String {
    match device {
        "test" => "Test pattern".into(),
        d if d.starts_with("test:") => format!("Test pattern ({})", &d[5..]),
        "screen" => "Screen capture".into(),
        "default" => "System default".into(),
        "media" => "Media player".into(),
        "pads" => "Sound pads".into(),
        d if d.starts_with("source:") => format!("Sound from {}", d[7..].trim()),
        d if d.starts_with("app:") => format!("Sound from {}", d[4..].trim()),
        d if d.starts_with("ndi:") => format!("NDI {}", d[4..].trim()),
        d if d.starts_with("frame:") => format!("Auto-framed from {}", d[6..].trim()),
        d if d.starts_with("vptz:") => format!("Virtual PTZ in {}", d[5..].trim()),
        d if d.starts_with("guest:") => "Remote guest (link)".into(),
        d if d.starts_with("phone:") => "Phone camera".into(),
        d if d.starts_with("srt-in:") => format!("SRT input :{}", &d[7..]),
        d if d.starts_with("rtmp-in:") => format!("RTMP input :{}", d[8..].split('/').next().unwrap_or("")),
        d => d.chars().take(28).collect(),
    }
}

pub(super) fn unique(existing: Vec<String>, base: &str) -> String {
    let taken = |n: &str| existing.iter().any(|e| e.eq_ignore_ascii_case(n));
    if !taken(base) && !base.is_empty() {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap()
}

pub(super) fn video_device_combo(ui: &mut egui::Ui, value: &mut String, devices: &[DeviceInfo]) {
    egui::ComboBox::from_id_salt("video_device").selected_text(short_device(value)).width(300.0).show_ui(ui, |ui| {
        if devices.is_empty() {
            ui.label(RichText::new("No cameras or capture cards found").color(TEXT_DIM));
        }
        for d in devices {
            ui.selectable_value(value, d.name.clone(), &d.name).on_hover_text(&d.summary);
        }
        ui.separator();
        ui.selectable_value(value, "screen".to_string(), "Screen capture");
        for p in TEST_PATTERN_NAMES {
            ui.selectable_value(value, format!("test:{p}"), format!("Test pattern: {p}"));
        }
    });
}

/// Returns true while the list is open (so the app list can be fetched).
pub(super) fn audio_device_combo(ui: &mut egui::Ui, value: &mut String, devices: &[DeviceInfo], video_sources: &[String], apps: Option<&[String]>) -> bool {
    let r = egui::ComboBox::from_id_salt("audio_device").selected_text(short_audio_device(value)).width(300.0).height(420.0).show_ui(ui, |ui| {
        if devices.is_empty() {
            ui.label(RichText::new("No audio inputs found").color(TEXT_DIM));
        }
        for d in devices {
            ui.selectable_value(value, d.name.clone(), &d.name).on_hover_text(&d.summary);
        }
        ui.separator();
        ui.selectable_value(value, "default".to_string(), "System default input");
        ui.selectable_value(value, "test".to_string(), "Test tone");
        ui.selectable_value(value, "pads".to_string(), "Sound pads");
        if !video_sources.is_empty() {
            ui.separator();
            for v in video_sources {
                ui.selectable_value(value, format!("source:{v}"), format!("Sound from {v}"));
            }
        }
        if crate::platform::app_audio::available() {
            ui.separator();
            ui.label(RichText::new("Sound from an app").color(TEXT_DIM).size(12.0));
            match apps {
                None => {
                    ui.spinner();
                }
                Some([]) => {
                    ui.label(RichText::new("No apps found").color(TEXT_DIM));
                }
                Some(list) => {
                    for app in list {
                        ui.selectable_value(value, format!("app:{app}"), format!("  {app}"));
                    }
                }
            }
        }
    });
    r.inner.is_some()
}

pub(super) fn mode_combo(ui: &mut egui::Ui, modes: &[Mode], src: &mut SourceConfig) {
    let current = modes.iter().find(|m| m.width == src.width && m.height == src.height);
    let text = current.map(|m| m.label()).unwrap_or_else(|| "custom".into());
    egui::ComboBox::from_id_salt("device_modes").selected_text(text).width(300.0).show_ui(ui, |ui| {
        for m in modes {
            if ui.selectable_label(current == Some(m), m.label()).clicked() {
                src.width = m.width;
                src.height = m.height;
                if !m.fps.contains(&src.fps) {
                    src.fps = *m.fps.iter().filter(|f| **f <= 60).max().unwrap_or(&m.fps[0]);
                }
            }
        }
    });
}

pub(super) fn size_combo(ui: &mut egui::Ui, id: &str, w: &mut u32, h: &mut u32) {
    let text = RESOLUTIONS
        .iter()
        .find(|r| r.0 == *w && r.1 == *h)
        .map(|r| r.2.to_string())
        .unwrap_or_else(|| "Custom".into());
    egui::ComboBox::from_id_salt(id).selected_text(text).width(190.0).show_ui(ui, |ui| {
        for &(rw, rh, label) in RESOLUTIONS {
            if ui.selectable_label(*w == rw && *h == rh, label).clicked() {
                *w = rw;
                *h = rh;
            }
        }
    });
}

pub(super) fn fps_picker(ui: &mut egui::Ui, id: &str, fps: &mut u32, allowed: Option<&[u32]>) {
    ui.horizontal(|ui| {
        let options: Vec<u32> = allowed.map(|a| a.to_vec()).unwrap_or_else(|| RATES.to_vec());
        egui::ComboBox::from_id_salt(id).selected_text(format!("{fps} fps")).width(90.0).show_ui(ui, |ui| {
            for r in &options {
                ui.selectable_value(fps, *r, format!("{r} fps"));
            }
        });
        ui.add(egui::DragValue::new(fps).range(1..=120).suffix(" fps"));
        if let Some(a) = allowed {
            if !a.contains(fps) {
                ui.label(RichText::new("(converted from the device's rate)").size(11.5).color(TEXT_DIM));
            }
        }
    });
}

pub(super) fn channel_combo(ui: &mut egui::Ui, id: &str, value: &mut u32, channels: u32) {
    egui::ComboBox::from_id_salt(id).selected_text(format!("Ch {value}")).width(80.0).show_ui(ui, |ui| {
        for c in 1..=channels {
            ui.selectable_value(value, c, format!("Ch {c}"));
        }
    });
}

pub(super) fn encoder_combo(ui: &mut egui::Ui, id: &str, value: &mut String, detected: Option<&[String]>) {
    let shown = if value.eq_ignore_ascii_case("auto") || value.is_empty() {
        "Automatic (best available)".to_string()
    } else {
        format!("{value}{}", if crate::engine::encode::is_hardware(value) { "  (hardware)" } else { "" })
    };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(260.0).show_ui(ui, |ui| {
        ui.selectable_value(value, "auto".to_string(), "Automatic (best available)");
        match detected {
            None => {
                ui.label(RichText::new("No encoders listed").color(TEXT_DIM));
            }
            Some([]) => {
                ui.label(RichText::new("No working encoders found").color(AMBER));
            }
            Some(list) => {
                for e in list {
                    let label = format!("{e}{}", if crate::engine::encode::is_hardware(e) { "  (hardware)" } else { "  (software)" });
                    ui.selectable_value(value, e.clone(), label);
                }
            }
        }
    });
}

pub(super) fn bitrate_field(ui: &mut egui::Ui, value: &mut u32, auto_hint: &str) {
    ui.horizontal(|ui| {
        let mut auto = *value == 0;
        if ui.checkbox(&mut auto, "Automatic").changed() {
            *value = if auto { 0 } else { 20_000 };
        }
        if auto {
            hint(ui, auto_hint);
            if auto_hint.starts_with("set from") {
                hint(ui, "(≈12 Mbps at 1080p30, ≈50 Mbps at 4K30)");
            }
        } else {
            ui.add(egui::DragValue::new(value).range(1_000..=300_000).speed(250).suffix(" kbps"));
        }
    });
}

pub(super) fn container_label(c: Container) -> &'static str {
    match c {
        Container::Mp4 => "MP4 (crash-safe)",
        Container::Mov => "MOV (crash-safe)",
    }
}

/// "A", "A and B", "A, B and C" (in quotes).
fn list_names(names: &[String]) -> String {
    let q: Vec<String> = names.iter().map(|n| format!("\u{201c}{n}\u{201d}")).collect();
    match q.len() {
        0 => String::new(),
        1 => q[0].clone(),
        n => format!("{} and {}", q[..n - 1].join(", "), q[n - 1]),
    }
}

/// Rough, commonly published ranges for live streaming bitrates.
pub(super) fn recommended_kbps(w: u32, h: u32, fps: u32) -> (u32, u32) {
    let pixels = w.max(h) as u64 * w.min(h) as u64;
    let high = fps > 30;
    match pixels {
        p if p <= 1280 * 720 => if high { (4500, 6000) } else { (3000, 4500) },
        p if p <= 1920 * 1080 => if high { (6000, 9000) } else { (4500, 6000) },
        p if p <= 2560 * 1440 => if high { (9000, 18000) } else { (6000, 13000) },
        _ => if high { (20000, 51000) } else { (13000, 34000) },
    }
}

fn shortcuts(ui: &mut egui::Ui) {
    title(ui, "Shortcuts", "Keyboard control while you direct.");
    let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
    let rows: Vec<(String, &str)> = vec![
        ("1 – 9".into(), "Take scene 1–9 (using the current transition)"),
        ("Shift + 1 – 9".into(), "Cut straight to scene 1–9"),
        ("Click a source".into(), "Put that source full screen (Shift+click cuts)"),
        ("T".into(), "Switch between Cut and your transition"),
        ("Space".into(), "Take the preview scene to program (preview mode)"),
        ("Enter".into(), "Cut the preview scene to program (preview mode)"),
        ("A".into(), "Automatic switching on / off"),
        ("C".into(), "Live captions on screen / off"),
        ("M".into(), "Add a marker while recording"),
        ("K".into(), "Save a clip of the last few seconds while recording"),
        ("F1 – F12".into(), "Sound pads 1–12 (hold for \"while held\" pads)"),
        ("B (hold)".into(), "Talkback: your mic in the presenters' ears while held"),
        ("N".into(), "Next rundown segment (Shift+N goes back)"),
        ("Alt + 1 – 9".into(), "Show / hide graphic 1–9"),
        ("E".into(), "Edit the live layout"),
        (format!("{cmd}R"), "Start / stop recording"),
        (format!("{cmd},"), "Open Settings"),
        ("Esc".into(), "Close Settings"),
        ("F / Esc".into(), "On the second-screen window: full screen / leave full screen"),
        ("Right-click a media tile".into(), "Pick from the media player's playlist"),
    ];
    egui::Grid::new("keys").num_columns(2).spacing([24.0, 10.0]).striped(true).show(ui, |ui| {
        for (k, what) in rows {
            ui.label(RichText::new(k).monospace().strong());
            ui.label(what);
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    hint(ui, "Going live is mouse-only on purpose, so a stray key can't start or end a broadcast.");
}

/// Wherever NDI is chosen: when it isn't installed, offer to install NDI's
/// own free runtime (downloaded from NDI, its installer opened), and credit
/// NDI as its licence asks.
pub fn ndi_offer(ui: &mut egui::Ui) {
    use crate::net::ndi_install::{self, Stage};
    if crate::engine::ndi::available() {
        ndi_credit(ui);
        return;
    }
    ui.label(RichText::new("⚠ NDI isn't installed on this computer.").color(AMBER));
    match ndi_install::stage() {
        Stage::Idle if ndi_install::installer().is_none() => {
            hint(ui, "Install the NDI SDK from ndi.video; Cutaway finds it as soon as it's there.");
        }
        Stage::Idle => {
            if ui.button("Install NDI…").on_hover_text("Downloads NDI's free runtime from NDI and opens its installer").clicked() {
                ndi_install::start();
            }
            hint(ui, "Downloads NDI's free runtime from NDI and opens its installer, which asks for your password. Cutaway picks NDI up as soon as it's installed.");
        }
        Stage::Downloading { got, total } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Downloading NDI… {}", ndi_install::progress_text(got, total)));
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        Stage::Opened(_) => {
            hint(ui, "NDI's installer is open: follow its steps (it asks for your password). Cutaway picks NDI up as soon as it's installed.");
            ui.horizontal(|ui| {
                if ui.small_button("Open the installer again").clicked() {
                    ndi_install::open_again();
                }
                if ui.small_button("Download it again").clicked() {
                    ndi_install::start();
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        Stage::Failed(e) => {
            ui.add(egui::Label::new(RichText::new(format!("Couldn't get NDI: {e}")).color(AMBER)).wrap());
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    ndi_install::start();
                }
                ui.hyperlink_to("Get it from NDI's website", ndi_install::PAGE);
            });
        }
    }
    ndi_credit(ui);
}

/// "NDI® is a registered trademark of Vizrt NDI AB", with a link to NDI.
pub fn ndi_credit(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(crate::net::ndi_install::TRADEMARK).size(11.0).color(TEXT_DIM));
        ui.hyperlink_to(RichText::new("ndi.video").size(11.0), crate::net::ndi_install::SITE);
    });
}

/// A QR code for `text`, `size` points across, on a white card (phones
/// scan it from the screen).
pub fn qr_code(ui: &mut egui::Ui, text: &str, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(40));
        return resp;
    };
    let n = code.width();
    let quiet = 2.0;
    let cell = size / (n as f32 + quiet * 2.0);
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, egui::Color32::WHITE);
    let colors = code.to_colors();
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if colors[y * n + x] == qrcode::Color::Dark {
                // A run of dark cells as one rectangle (no seams).
                let start = x;
                while x < n && colors[y * n + x] == qrcode::Color::Dark {
                    x += 1;
                }
                let min = rect.min + vec2((start as f32 + quiet) * cell, (y as f32 + quiet) * cell);
                p.rect_filled(egui::Rect::from_min_size(min, vec2((x - start) as f32 * cell, cell)).expand(0.25), 0.0, egui::Color32::BLACK);
            } else {
                x += 1;
            }
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(recording: bool) -> SettingsContext {
        SettingsContext {
            recording,
            streaming: false,
            engine_facts: Vec::new(),
            log: Vec::new(),
            config_path: None,
            profiles_dir: None,
            midi_last: None,
            midi_connected: Vec::new(),
            remote_port: None,
            osc_port: None,
            captions_status: None,
            vcam_on: false,
            input_levels: Vec::new(),
            stinger: None,
            guest_reach: Default::default(),
            guest_base: None,
            guest_status: Vec::new(),
            ndi_watchers: None,
            on_air: Vec::new(),
        }
    }

    /// Taking a source out while recording asks first; while not, it doesn't.
    #[test]
    fn asks_before_removing_while_recording() {
        let mut cfg = Config::default();
        cfg.sources = vec![
            SourceConfig { name: "Wide".into(), device: "test:bars".into(), ..Default::default() },
            SourceConfig { name: "Close".into(), device: "test:gradient".into(), ..Default::default() },
        ];
        cfg.assign_uids();
        let close_uid = cfg.sources[1].uid;
        let mut win = SettingsUi::new(cfg.clone());
        win.draft.sources.remove(1);
        win.draft.audio.clear();
        let mut info = ctx(true);
        info.on_air = vec![close_uid];
        assert!(matches!(win.apply(false, &info), SettingsAction::None));
        let check = win.confirm_remove.take().expect("asks");
        assert!(check.names.contains(&"Close".to_string()), "{:?}", check.names);
        assert_eq!(check.on_air, vec!["Close".to_string()]);
        assert!(matches!(win.apply(false, &ctx(false)), SettingsAction::Apply { .. }));
        assert!(win.confirm_remove.is_none());
        // Adding one while recording just applies.
        let mut win = SettingsUi::new(cfg.clone());
        win.draft.sources.push(SourceConfig { name: "New".into(), device: "test:smpte".into(), ..Default::default() });
        assert!(matches!(win.apply(false, &ctx(true)), SettingsAction::Apply { .. }));
    }

    /// After applying, a source added in the window keeps the identity it
    /// was given, so applying again doesn't open it a second time.
    #[test]
    fn new_sources_keep_their_identity() {
        let mut cfg = Config::default();
        cfg.assign_uids();
        let mut win = SettingsUi::new(cfg.clone());
        win.draft.sources.push(SourceConfig { name: "New".into(), device: "test:smpte".into(), ..Default::default() });
        let mut now = win.draft.clone();
        now.assign_uids();
        win.applied(Ok(()), &now);
        assert_eq!(win.draft.sources.last().unwrap().uid, now.sources.last().unwrap().uid);
        assert!(!win.dirty());
    }
}
