//! The Settings pages: each edits a draft copy of the configuration, and
//! they open inside the main window as the rail's areas (ui/shell.rs).
//! The pages in `live::LIVE` save as you go, with Undo (see live.rs); on
//! the others nothing changes until Apply/OK, and changes that need the
//! video engine rebuilt are flagged, everything else applies live.

mod captions;
mod capture_picker;
mod comments;
mod control;
mod graphics;
mod guests;
mod help;
mod live;
mod macros;
mod prompter;
mod mixing;
mod preview;
mod outputs;
mod rotations;
mod rundown;
mod scenes;
mod scheduled;
mod shortcuts;
mod sound;
mod sources;
mod switching;
mod upload;
mod vertical;


use crate::util::LockExt;
use crate::config::{AudioConfig, Codec, Config, Container, Service, SourceConfig, StreamDestination};
use crate::devices::{self, DeviceInfo, Kind, Mode, TEST_PATTERN_NAMES};
use crate::engine::{LogLevel, LogLine};
use crate::ui::theme::*;
use eframe::egui::{self, vec2, Align, Color32, Layout, RichText};
pub use live::is_live;
pub use mixing::MixView;
pub use switching::SwitchView;
pub use sources::SourcesView;
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
    Scenes,
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
    pub(crate) const ALL: [Section; 24] = [
        Section::General,
        Section::Sources,
        Section::Audio,
        Section::Mixing,
        Section::Pads,
        Section::Syrinscape,
        Section::Program,
        Section::Switching,
        Section::Scenes,
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
    /// The name a page has in the code (as in the help table).
    fn from_ident(name: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| format!("{s:?}") == name)
    }

    /// For scripted screenshots: "streaming", "sources", ...
    pub fn from_name(name: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.label().to_lowercase().contains(&name.to_lowercase()))
    }

    /// Its name in the old Settings window (⌘K search still knows it by it).
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General & shows",
            Section::Sources => "Video sources",
            Section::Audio => "Audio inputs",
            Section::Mixing => "Mixing & headphones",
            Section::Pads => "Sound pads",
            Section::Syrinscape => "Syrinscape",
            Section::Program => "Program",
            Section::Switching => "Switching",
            Section::Scenes => "Scenes",
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

}

/// The old Settings window's sidebar: a few groups, each with its pages as
/// tabs. The rail's areas (ui/nav.rs) took their place; the names stay
/// for ⌘K search to know them by.
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
            Group::Show => "Scenes, switching, rundown",
            Group::Output => "Recording, streaming, screens",
            Group::General => "Shows, control, diagnostics",
        }
    }

    fn sections(self) -> &'static [Section] {
        match self {
            Group::Inputs => &[Section::Sources, Section::Audio, Section::Guests],
            Group::Sound => &[Section::Mixing, Section::Pads, Section::Syrinscape],
            Group::Look => &[Section::Program, Section::Graphics, Section::Captions],
            Group::Show => &[Section::Scenes, Section::Switching, Section::Rundown, Section::Macros, Section::Prompter, Section::Comments],
            Group::Output => &[Section::Recording, Section::Streaming, Section::Vertical, Section::Upload, Section::Outputs],
            Group::General => &[Section::General, Section::Control, Section::Shortcuts, Section::Diagnostics],
        }
    }
}

// ---- for ⌘K search (ui/search.rs) ----------------------------------------------------

/// Every setting that has help: its page, its name as the page writes it,
/// and what the help says.
pub fn setting_entries() -> impl Iterator<Item = (Section, &'static str, &'static str)> {
    help::entries().iter().map(|e| (e.page, e.label.as_str(), e.text.as_str()))
}

/// The folded part of its page a setting is drawn in, if it's in one.
pub fn folded_part(page: Section, label: &str) -> Option<&'static str> {
    help::part_of(page, label)
}

/// Light up (and scroll to) a setting's name the next time its page shows it.
pub fn light_setting(page: Section, label: &str) {
    help::find_target(page, label);
}

/// The old window's groups: name, what was in it, and its first page.
pub fn old_groups() -> impl Iterator<Item = (&'static str, &'static str, Section)> {
    Group::ALL.into_iter().map(|g| (g.label(), g.summary(), g.sections()[0]))
}

/// What the old "Find a setting…" box found (a page, and a setting on it),
/// for the test that ⌘K finds all of it.
#[cfg(test)]
pub fn old_find(query: &str, max: usize) -> Vec<(Section, Option<String>)> {
    help::search(query, max).into_iter().map(|h| (h.page, h.label)).collect()
}

/// What the app should do after showing the window this frame.
pub enum SettingsAction {
    None,
    Close,
    /// `base`: the settings `cfg` was edited from, so only what changed
    /// since is applied (anything changed on the desk meanwhile stays).
    Apply { cfg: Config, base: Box<Config>, close: bool },
    /// Turn the virtual camera on or off now.
    VirtualCamera(bool),
    /// Close and measure lip sync.
    MeasureSync,
    /// A page that saves as you go: apply this now (only what changed since
    /// `base`), as one Undo step called `label`.
    Live { cfg: Config, base: Box<Config>, label: String },
    /// The page's ↶ Undo and ↷ Redo (the app's own, as ⌘Z).
    Undo,
    Redo,
    /// Go to another place ("area/tab/section").
    Go(&'static str),
    /// Do something on the show now (a media player's next, say).
    Run(crate::control::Action),
    /// Scenes › Scenes › Edit its layout: Show › Edit layout on this scene
    /// (put in preview with Preview on; never taken to air).
    EditLayout(String),
}

/// Read-only facts the window shows (engine state, log, ...).
pub struct SettingsContext {
    pub recording: bool,
    pub streaming: bool,
    /// Bytes per second the take in progress writes, once settled (None:
    /// not recording or too early), with the estimate for the settings that
    /// take started with (so it's only quoted for those settings).
    pub disk_rate: Option<(f64, f64)>,
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
    /// Live level (dB) of each running input's channels, as the
    /// automatic switcher hears them (before processing), by its place in
    /// the settings in effect.
    pub input_levels: Vec<Vec<f32>>,
    /// Each running input's level as its meter on the desk shows it (dB,
    /// the loudest channel; `Engine::input_level`): the one level every
    /// screen shows (numbers audit #4). None while it isn't open.
    pub input_meters: Vec<Option<f32>>,
    /// Whether each running input is ON (heard in the mixes).
    pub input_on: Vec<bool>,
    /// The talkback key as it's set now ("B"), or empty when it has none.
    pub talkback_key: String,
    /// Syrinscape as the desk's connection sees it: what's playing, the
    /// volume, the last problem.
    pub syrin: crate::control::syrinscape::SyrinState,
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
    /// The scene on the program right now, by name.
    pub on_air_scene: Option<String>,
    /// What ↶ Undo and ↷ Redo would undo or redo now.
    pub undo: Option<String>,
    pub redo: Option<String>,
    /// Running sources' pictures (by uid), as the multiview shows them.
    pub pictures: Vec<preview::Picture>,
    /// Virtual PTZ cameras (by uid), for the shot they're on.
    pub virtual_ptz: Vec<(u64, Arc<crate::engine::ptz::VirtualPtz>)>,
}

pub use preview::Picture;
pub(crate) use scenes::new_scene_menu;

/// Applying would take sources or inputs out of a show that's recording or
/// live: asked first.
struct RemoveCheck {
    names: Vec<String>,
    /// Of those, the ones on the program right now.
    on_air: Vec<String>,
    /// Sources that stay but restart (a changed device, size, delay…).
    restarting: Vec<String>,
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
    /// Scheduling on YouTube, while it's under way.
    schedule_job: Option<scheduled::ScheduleJob>,
    show_keys: HashSet<usize>,
    encoders: Arc<Mutex<Option<EncoderLists>>>,
    confirm_discard: bool,
    confirm_reset: bool,
    confirm_remove: Option<RemoveCheck>,
    message: Option<(String, bool)>,
    /// The settings an Apply failed for: its error shows until they change.
    failed: Option<Box<Config>>,
    outputs: Vec<DeviceInfo>,
    sel_graphic: usize,
    profile_name: String,
    /// The show being renamed, and its new name so far.
    renaming: Option<(String, String)>,
    /// The show waiting for "Delete for good".
    deleting: Option<String>,
    ndi_found: Arc<Mutex<Option<Result<Vec<String>, String>>>>,
    capture: capture_picker::CapturePicker,
    vcam_job: Arc<Mutex<Option<Result<String, String>>>>,
    vcam_busy: bool,
    learning: Option<usize>,
    /// A shortcut waiting for its key.
    key_capture: Option<shortcuts::KeyCapture>,
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
    /// Settings → Scenes: the scene picked, its new name being typed, and
    /// the scene waiting for "Delete".
    sel_scene: usize,
    scene_rename: Option<(usize, String)>,
    confirm_scene_delete: Option<usize>,
    /// Apps that can be listened to (for "Sound from an app").
    apps: Option<Vec<String>>,
    /// Installing the Resolve script: what happened.
    resolve_note: Option<(String, bool)>,
    /// The LUT picked for a source, checked: (file, description or problem).
    lut_check: Option<(PathBuf, Result<String, String>)>,
    /// The virtual PTZ preset picked in the picture: (source, preset).
    sel_preset: Option<(usize, usize)>,
    /// The bottom bar's height, as last drawn.
    bar_h: f32,
    /// When a page that saves as you go last applied a change.
    last_live: Option<std::time::Instant>,
    /// Which part of the sources a Sources tab shows.
    view: SourcesView,
    /// A source was picked here (or asked for): a remembered one doesn't
    /// replace it.
    sel_picked: bool,
    /// The same for the input on Sound › Inputs.
    sel_audio_picked: bool,
    /// Which part of the mixing settings shows.
    mix_view: MixView,
    /// Which part of the switching settings shows (the Scenes tab it's on).
    switch_view: SwitchView,
    /// Syrinscape's volume on its page, while it's being set.
    syrin_volume: Option<f64>,
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

/// An input's level, over the fader's range (−60 to +12 dB, as MIDI and
/// the `gain` command use: numbers audit #2). It takes only a change made
/// on it: just drawing it never rounds or clamps the level, which isn't a
/// change of yours and mustn't be saved. Dragging moves in 0.5 dB steps;
/// the box takes any value.
fn gain_slider(ui: &mut egui::Ui, gain_db: &mut f64) -> egui::Response {
    let mut g = *gain_db as f32;
    let r = ui
        .horizontal(|ui| {
            let s = ui.add(egui::Slider::new(&mut g, -60.0..=12.0).show_value(false).step_by(0.5).clamping(egui::SliderClamping::Never));
            let mut v = g;
            let d = ui.add(egui::DragValue::new(&mut v).range(-60.0..=12.0).clamp_existing_to_range(false).speed(0.1).fixed_decimals(1).suffix(" dB"));
            if d.changed() {
                g = v;
            }
            let r = s.union(d);
            if r.double_clicked() {
                g = 0.0;
            }
            r
        })
        .inner;
    if (r.changed() || r.double_clicked()) && (g as f64 - *gain_db).abs() > 1e-6 && (r.dragged() || r.clicked() || r.has_focus() || r.double_clicked()) {
        *gain_db = g as f64;
    }
    r.on_hover_text("Double-click for 0 dB")
}

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
            schedule_job: None,
            show_keys: HashSet::new(),
            encoders: Arc::new(Mutex::new(Some({
                let list = |c: Codec| crate::engine::encode::video_backends(c.video_codec()).into_iter().map(String::from).collect::<Vec<_>>();
                (list(Codec::H264), list(Codec::H265))
            }))),
            confirm_discard: false,
            confirm_reset: false,
            confirm_remove: None,
            message: None,
            failed: None,
            outputs: crate::platform::list_output_devices(),
            sel_graphic: 0,
            profile_name: String::new(),
            renaming: None,
            deleting: None,
            ndi_found: Arc::new(Mutex::new(None)),
            capture: capture_picker::CapturePicker::new(),
            vcam_job: Arc::new(Mutex::new(None)),
            vcam_busy: false,
            learning: None,
            key_capture: None,
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
            sel_scene: 0,
            scene_rename: None,
            confirm_scene_delete: None,
            apps: None,
            resolve_note: None,
            lut_check: None,
            sel_preset: None,
            bar_h: 48.0,
            last_live: None,
            view: SourcesView::Cameras,
            sel_picked: false,
            sel_audio_picked: false,
            mix_view: MixView::Mixer,
            switch_view: SwitchView::Transition,
            syrin_volume: None,
        }
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn open_at(mut self, section: Section) -> Self {
        self.section = section;
        self
    }

    /// Open with source `i` split into `n` close-ups, ready to frame.
    pub fn with_split(mut self, i: usize, n: usize) -> Self {
        self.split_source(i, n);
        self
    }

    /// Show this source on the Video sources page.
    pub fn select_source(mut self, i: usize) -> Self {
        self.sel_source = i;
        self.sel_picked = true;
        self
    }

    /// The source picked on the Sources pages, by name (remembered on this
    /// computer: forensics settings #2).
    pub fn source_name(&self) -> Option<String> {
        self.draft.sources.get(self.sel_source).map(|s| s.name.clone())
    }

    /// Pick the source remembered from last time, unless one was picked
    /// already. By name, never by place (a reorder would point elsewhere);
    /// gone: the first.
    pub fn restore_source(&mut self, name: &str) {
        if !self.sel_picked {
            self.sel_picked = true;
            if let Some(i) = self.draft.sources.iter().position(|s| s.name == name) {
                self.sel_source = i;
            }
        }
    }

    /// The input picked on Sound › Inputs, by name (remembered on this
    /// computer, as the source is).
    pub fn input_name(&self) -> Option<String> {
        self.draft.audio.get(self.sel_audio).map(|a| a.name.clone())
    }

    /// Pick the input remembered from last time, once (by name; gone: the first).
    pub fn restore_input(&mut self, name: &str) {
        if !self.sel_audio_picked {
            self.sel_audio_picked = true;
            if let Some(i) = self.draft.audio.iter().position(|a| a.name == name) {
                self.sel_audio = i;
            }
        }
    }

    /// Which part of the mixing settings the page shows (Sound › Mixer, or
    /// Headphones & talkback).
    pub fn set_mix_view(&mut self, v: MixView) {
        self.mix_view = v;
    }

    /// Which part of the switching settings the page shows (Scenes ›
    /// Switching, Auto-switching, Stinger & fade to black, or Replay).
    pub fn set_switch_view(&mut self, v: SwitchView) {
        self.switch_view = v;
    }

    /// Which part of the sources the page shows (the Sources area's tab).
    pub fn set_sources_view(&mut self, v: SourcesView) {
        self.view = v;
    }

    /// A camera renamed elsewhere (People & names): what's being edited
    /// here that names it follows, so nothing points at the old name.
    pub fn follow_rename(&mut self, uid: u64, old: &str, new: &str) {
        for s in &mut self.draft.sources {
            if s.uid == uid && uid != 0 {
                s.name = new.to_string();
            }
            if s.framed_from().is_some_and(|f| f.trim().eq_ignore_ascii_case(old.trim())) {
                s.device = format!("frame:{new}");
            }
            if s.virtual_ptz_of().is_some_and(|f| f.trim().eq_ignore_ascii_case(old.trim())) {
                s.device = format!("vptz:{new}");
            }
        }
    }

    /// Settings → Scenes with this scene picked, and asking whether to delete it (for the UI self-test).
    pub fn select_scene(mut self, i: usize, confirm_delete: bool) -> Self {
        self.sel_scene = i;
        self.confirm_scene_delete = confirm_delete.then_some(i);
        self
    }

    /// Change the settings being edited, as the page would (for the UI self-test).
    pub fn edit_draft(&mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.draft);
    }

    /// The settings as they are being edited (for the UI self-test).
    pub fn draft_config(&self) -> Config {
        self.draft.clone()
    }

    /// Changes not applied yet (on a page that saves as you go, only those
    /// held back for its Apply: the rest is applied by itself).
    pub fn dirty(&self) -> bool {
        self.unapplied()
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
                // (The lists differ when something was added meanwhile
                // outside Settings: new ones are found by name instead.)
                for d in 0..self.draft.sources.len() {
                    if self.draft.sources[d].uid == 0 {
                        let name = &self.draft.sources[d].name;
                        if let Some(n) = now.sources.iter().find(|n| &n.name == name && self.draft.sources.iter().all(|o| o.uid != n.uid)) {
                            self.draft.sources[d].uid = n.uid;
                        }
                    }
                }
                for d in 0..self.draft.audio.len() {
                    if self.draft.audio[d].uid == 0 {
                        let name = &self.draft.audio[d].name;
                        if let Some(n) = now.audio.iter().find(|n| &n.name == name && self.draft.audio.iter().all(|o| o.uid != n.uid)) {
                            self.draft.audio[d].uid = n.uid;
                        }
                    }
                }
                self.original = self.draft.clone();
                self.message = Some(("Saved".into(), false));
                self.failed = None;
            }
            Err(e) => {
                self.message = Some((e, true));
                self.failed = Some(Box::new(self.draft.clone()));
            }
        }
    }

    /// The settings as a page inside the main window (a rail area), not a
    /// window of its own: the rail and the top bar's tabs stand in for the
    /// sidebar and tabs, so this draws the page and the bottom bar under it.
    /// (Finding a setting is ⌘K search, ui/search.rs.) Esc (not while
    /// typing) asks to close, as ✕ did.
    pub fn show_page(&mut self, ui: &mut egui::Ui, info: &SettingsContext) -> SettingsAction {
        let ctx = ui.ctx().clone();
        let normal = ctx.global_style();
        let mut larger = (*normal).clone();
        for f in larger.text_styles.values_mut() {
            f.size = (f.size * TEXT_SCALE * 2.0).round() / 2.0;
        }
        ctx.set_global_style(larger.clone());
        ui.set_style(larger);
        let typing = ctx.memory(|m| m.focused().is_some());
        let esc = self.key_capture.is_none() && !typing && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut action = SettingsAction::None;
        let size = ui.available_size();
        let body_h = (size.y - self.bar_h).max(120.0);
        ui.allocate_ui_with_layout(vec2(size.x, body_h), Layout::top_down(Align::Min), |ui| {
            ui.set_min_height(body_h);
            egui::ScrollArea::vertical().id_salt(self.section).max_height(body_h).auto_shrink([false, false]).show(ui, |ui| self.page(ui, info));
        });
        let top = ui.cursor().top();
        ui.separator();
        let bar = if live::is_live(self.section) { self.live_bar(ui, info) } else { self.bottom_bar(ui, info) };
        if let Some(a) = bar {
            action = a;
        }
        if matches!(action, SettingsAction::None) {
            if let Some(a) = self.live_step(&ctx) {
                action = a;
            }
        }
        self.bar_h = ui.min_rect().bottom() - top + ui.spacing().item_spacing.y;
        if let Some(a) = self.after(&ctx, esc) {
            action = a;
        }
        ctx.set_global_style(normal);
        action
    }

    /// Asked to leave the page (for Show): closes now when nothing's
    /// changed, or asks "Discard your changes?" first (true: closed).
    pub fn leave(&mut self) -> bool {
        if self.dirty() {
            self.confirm_discard = true;
            false
        } else {
            true
        }
    }

    /// Waiting for an answer or a key: the show's keys stay off meanwhile.
    pub fn busy(&self) -> bool {
        self.key_capture.is_some() || self.confirm_discard || self.confirm_reset || self.confirm_remove.is_some()
    }

    /// Open one of the page's folded parts (by the id it's drawn with).
    pub fn unfold(&self, part: &str) {
        help::open_part(part);
    }

    /// Go to another page, keeping every change made so far.
    pub fn go_to(&mut self, section: Section) {
        self.section = section;
    }

    /// The page itself (its settings, top to bottom).
    fn page(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        ui.add_space(4.0);
        help::set_page(Some(self.section));
        match self.section {
            Section::General => self.general(ui, info),
            Section::Sources => self.sources(ui, info),
            Section::Audio => self.audio_inputs(ui, info),
            Section::Mixing => self.mixing(ui, info),
            Section::Pads => self.pads(ui),
            Section::Syrinscape => self.syrinscape(ui, info),
            Section::Program => self.program(ui),
            Section::Switching => self.switching(ui, info),
            Section::Graphics => self.graphics(ui),
            Section::Rundown => self.rundown(ui),
            Section::Scenes => self.scenes(ui, info),
            Section::Macros => self.macros(ui),
            Section::Prompter => self.prompter(ui),
            Section::Comments => self.comments(ui),
            Section::Guests => self.guests(ui, info),
            Section::Captions => self.captions(ui, info),
            Section::Recording => self.recording(ui, info),
            Section::Streaming => self.streaming(ui),
            Section::Vertical => self.vertical(ui),
            Section::Upload => self.upload(ui),
            Section::Outputs => self.outputs(ui, info),
            Section::Control => self.control(ui, info),
            Section::Shortcuts => self.shortcuts(ui),
            Section::Diagnostics => self.diagnostics(ui, info),
        }
        help::set_page(None);
        ui.add_space(12.0);
    }

    /// Revert, what's going on, then Cancel / Apply / OK. Returns what to do.
    fn bottom_bar(&mut self, ui: &mut egui::Ui, info: &SettingsContext) -> Option<SettingsAction> {
        let mut action = None;
        let valid = self.draft.check();
        let restart = self.original.needs_restart(&self.draft);
        let live = info.recording || info.streaming;
        let blocked = restart && live;
        let lineup = !restart && self.original.lineup_changed(&self.draft);
        let failed = match (&self.message, &self.failed) {
            (Some((m, true)), Some(f)) if **f == self.draft => Some(m.clone()),
            _ => None,
        };
        ui.horizontal(|ui| {
            // Revert on its own at the left, away from OK.
            if ui.add_enabled(self.dirty(), egui::Button::new("Revert")).on_hover_text("Undo every change since the last save").clicked() {
                self.draft = self.original.clone();
                self.scene_rename = None;
            }
            if let Err(e) = &valid {
                let r = ui.add(egui::Label::new(RichText::new(format!("⚠ {}", e.message)).color(AMBER)).sense(egui::Sense::click()));
                let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click to go there");
                if r.clicked() {
                    use crate::config::ProblemAt;
                    match e.at {
                        ProblemAt::Program => self.section = Section::Program,
                        ProblemAt::Source(i) => {
                            self.section = Section::Sources;
                            self.sel_source = i;
                        }
                        ProblemAt::Audio(i) => {
                            self.section = Section::Audio;
                            self.sel_audio = i;
                        }
                        ProblemAt::Recording => self.section = Section::Recording,
                        ProblemAt::Streaming => self.section = Section::Streaming,
                        ProblemAt::Control => self.section = Section::Control,
                    }
                }
            } else if blocked {
                ui.label(RichText::new("Stop recording and streaming to change the program's size or frame rate.").color(AMBER));
            } else if let Some(m) = &failed {
                // Why the last Apply didn't work, until something's changed.
                ui.label(RichText::new(m).color(ERROR));
            } else if restart {
                ui.label(RichText::new("Applying restarts the video engine (previews blink for a moment).").color(TEXT_DIM));
            } else if lineup && live {
                ui.label(RichText::new("Unsaved changes. Sources you added, removed or changed update on Apply; the show carries on.").color(TEXT_DIM));
            } else if lineup {
                ui.label(RichText::new("Unsaved changes. Sources you added, removed or changed update on Apply; the rest carries on.").color(TEXT_DIM));
            } else if self.dirty() {
                ui.label(RichText::new("Unsaved changes. These apply without interrupting anything.").color(TEXT_DIM));
            } else if let Some((m, err)) = &self.message {
                ui.label(RichText::new(m).color(if *err { ERROR } else { GREEN }));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can_apply = self.dirty() && valid.is_ok() && !blocked;
                // Nothing changed: nothing to apply (Cancel or ✕ closes).
                if ui.add_enabled(can_apply, egui::Button::new("OK").min_size(vec2(72.0, 28.0))).on_disabled_hover_text(if self.dirty() { "Fix what's marked first" } else { "Nothing has changed" }).clicked() {
                    action = Some(self.apply(true, info));
                }
                if ui.add_enabled(can_apply, egui::Button::new("Apply").min_size(vec2(72.0, 28.0))).clicked() {
                    action = Some(self.apply(false, info));
                }
                if ui.button("Cancel").clicked() {
                    if self.dirty() {
                        self.confirm_discard = true;
                    } else {
                        action = Some(SettingsAction::Close);
                    }
                }
            });
        });
        action
    }

    /// After the page: what it asked for, the "are you sure?" boxes, and
    /// closing (`close`: asked to close, by ✕, Esc or leaving the page).
    fn after(&mut self, ctx: &egui::Context, close: bool) -> Option<SettingsAction> {
        let mut action = None;
        if let Some(a) = self.pending.take() {
            action = Some(a);
        }
        if let Some(a) = self.confirm_remove_window(ctx) {
            action = Some(a);
        }
        if close && !self.confirm_discard && !self.confirm_reset && self.confirm_remove.is_none() {
            if self.dirty() {
                self.confirm_discard = true;
            } else {
                action = Some(SettingsAction::Close);
            }
        }
        if self.confirm_discard {
            let r = egui::Modal::new(egui::Id::unique("discard_settings")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("Discard your changes?");
                ui.label(if live::is_live(self.section) {
                    "Some changes here wait for Apply because they open, close or restart sources. Everything else is already saved."
                } else {
                    "Your edits to the settings haven't been applied."
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard").clicked() {
                        action = Some(SettingsAction::Close);
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
    /// that's recording or live, or restarts one: then ask first.
    fn apply(&mut self, close: bool, info: &SettingsContext) -> SettingsAction {
        // A scene's name still being typed counts.
        self.commit_scene_rename();
        if info.recording || info.streaming {
            let (names, on_air) = self.leaving(&info.on_air);
            let restarting = self.restarting();
            if !names.is_empty() || !restarting.is_empty() {
                self.confirm_remove = Some(RemoveCheck { names, on_air, restarting, recording: info.recording, streaming: info.streaming, close });
                return SettingsAction::None;
            }
        }
        SettingsAction::Apply { cfg: self.draft.clone(), base: Box::new(self.original.clone()), close }
    }

    /// The sources that stay but reopen when applied (what the engine's
    /// `apply_lineup` remakes), close-ups following their wide camera.
    fn restarting(&self) -> Vec<String> {
        use crate::config::source_key;
        let (old, new) = (self.original.with_framed_rates(), self.draft.with_framed_rates());
        let mut names: Vec<String> = new
            .sources
            .iter()
            .filter(|s| s.uid != 0 && old.sources.iter().find(|o| o.uid == s.uid).is_some_and(|o| source_key(o) != source_key(s)))
            .map(|s| s.name.clone())
            .collect();
        loop {
            let more: Vec<String> = new
                .sources
                .iter()
                .filter(|s| s.uid != 0 && old.sources.iter().any(|o| o.uid == s.uid) && !names.iter().any(|n| n.eq_ignore_ascii_case(&s.name)))
                .filter(|s| s.cut_from().is_some_and(|from| names.iter().any(|n| n.trim().eq_ignore_ascii_case(from.trim()))))
                .map(|s| s.name.clone())
                .collect();
            if more.is_empty() {
                break;
            }
            names.extend(more);
        }
        names
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
            if check.names.is_empty() {
                let one = check.restarting.len() == 1;
                ui.heading(format!("Restart {} while {what}?", if one { "this source" } else { "these sources" }));
                ui.add_space(4.0);
                ui.label(format!("{} {} for a moment as soon as you apply, to take the new settings.", list_names(&check.restarting), if one { "restarts" } else { "restart" }));
                if check.recording {
                    ui.label(if one {
                        "Its own recording ends there and starts again from the next take; the program keeps recording."
                    } else {
                        "Their own recordings end there and start again from the next take; the program keeps recording."
                    });
                }
            } else {
                ui.heading(format!("Remove {} while {what}?", if one { "this" } else { "these" }));
                ui.add_space(4.0);
                ui.label(format!("{} {} the show as soon as you apply.", list_names(&check.names), if one { "leaves" } else { "leave" }));
                if check.recording {
                    ui.label(if one { "Its recording ends there; everything else keeps recording." } else { "Their recordings end there; everything else keeps recording." });
                }
                if !check.restarting.is_empty() {
                    ui.label(format!("{} also {} for a moment.", list_names(&check.restarting), if check.restarting.len() == 1 { "restarts" } else { "restart" }));
                }
            }
            if !check.on_air.is_empty() {
                ui.add_space(4.0);
                let on = list_names(&check.on_air);
                let is = if check.on_air.len() == 1 { "is" } else { "are" };
                ui.label(RichText::new(format!("⚠ {on} {is} on the program right now: take another scene first, or that part of the picture goes empty.")).color(AMBER));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let go = if check.names.is_empty() { "Restart and apply" } else { "Remove and apply" };
                if ui.button(RichText::new(go).color(ERROR)).clicked() {
                    action = Some(SettingsAction::Apply { cfg: self.draft.clone(), base: Box::new(self.original.clone()), close: check.close });
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

    /// Changes not applied yet on this page.
    pub fn section_dirty(&self, s: Section) -> bool {
        if live::is_live(s) {
            return self.held(s);
        }
        let (a, b) = (&self.draft, &self.original);
        match s {
            Section::General => a.ui != b.ui,
            Section::Sources => a.sources != b.sources,
            Section::Audio => a.audio != b.audio,
            Section::Mixing => a.mix != b.mix,
            Section::Pads => a.pads != b.pads,
            Section::Syrinscape => a.syrinscape != b.syrinscape,
            Section::Rundown => a.rundown != b.rundown,
            Section::Scenes => a.scenes != b.scenes,
            Section::Macros => a.macros != b.macros,
            Section::Prompter => a.prompter != b.prompter,
            Section::Comments => a.comments != b.comments,
            Section::Guests => a.guests != b.guests,
            Section::Program => a.program != b.program || a.style != b.style,
            Section::Switching => a.transition != b.transition || a.auto_switch != b.auto_switch || a.replay != b.replay,
            Section::Graphics => a.graphics != b.graphics || a.rotations != b.rotations,
            Section::Captions => a.captions != b.captions,
            Section::Recording => a.output != b.output || a.podcast != b.podcast,
            Section::Streaming => a.streaming != b.streaming || a.schedule != b.schedule,
            Section::Vertical => a.vertical != b.vertical,
            Section::Upload => a.upload != b.upload,
            Section::Outputs => a.display != b.display || a.ndi_out != b.ndi_out,
            Section::Control => a.remote != b.remote || a.midi != b.midi || a.osc != b.osc,
            Section::Shortcuts => a.keys != b.keys,
            _ => false,
        }
    }

    // ---- General ------------------------------------------------------------------

    fn general(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "General & shows", "The window, safety checks, and saved shows.");
        egui::Grid::new("general").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Interface size");
            ui.add(egui::Slider::new(&mut self.draft.ui.scale, 0.75..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            ui.end_row();
            // Stopping a recording always takes a hold (the REC button) or a
            // yes (keys), so only the stream has a choice here.
            label(ui, "Ask before");
            ui.checkbox(&mut self.draft.ui.confirm_end_stream, "ending a live stream");
            ui.end_row();
            label(ui, "Pre-show check");
            ui.checkbox(&mut self.draft.ui.check_before_start, "before recording or going live");
            ui.end_row();
            label(ui, "Updates");
            ui.vertical(|ui| {
                ui.checkbox(&mut self.draft.ui.check_updates, "look for a newer Cutaway when it starts");
                let can = self.draft.ui.check_updates;
                ui.add_enabled(can, egui::Checkbox::new(&mut self.draft.ui.auto_update, "download it by itself, and put it in when I quit"));
            });
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
                    self.message = Some(match self.draft.save_without_secrets(&path) {
                        Ok(()) => (format!("Exported to {} (without stream keys and passwords)", path.display()), false),
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

    fn audio_inputs(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
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
            // (Changes waiting for Apply first: it measures what's running.)
            let unsaved = self.unapplied();
            let b = ui.add_enabled(!unsaved, egui::Button::new("Measure lip sync…"));
            if b.on_hover_text(if unsaved { "Apply the changes waiting below first" } else { "Clap once: Cutaway works out the audio and video delays" }).clicked() {
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
                    let level = input_level(&self.original, &info.input_meters, a.uid);
                    let b = egui::Button::selectable(
                        self.sel_audio == i,
                        RichText::new(format!("{}\n{} · {} ch", a.name, short_audio_device(&a.device), a.channels)).size(TEXT_STATE),
                    )
                    .min_size(vec2(220.0, 52.0));
                    let r = ui.add(b);
                    // A live level along the bottom of the row.
                    let strip = egui::Rect::from_min_max(egui::pos2(r.rect.left() + 8.0, r.rect.bottom() - 9.0), egui::pos2(r.rect.right() - 8.0, r.rect.bottom() - 5.0));
                    match level {
                        Some(db) => paint_level(ui, strip, db),
                        None if self.original.audio.iter().all(|o| o.uid != a.uid || a.uid == 0) => {
                            ui.painter().text(egui::pos2(strip.right(), strip.center().y), egui::Align2::RIGHT_CENTER, "Apply to hear it", egui::FontId::proportional(TEXT_SMALL), TEXT_DIM);
                        }
                        None => {}
                    }
                    if r.clicked() {
                        self.sel_audio = i;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if let Some(edit) = list_buttons(ui, n, self.sel_audio, Some("＋ Add")) {
                        let audio = &self.audio;
                        apply_list_edit(
                            &mut self.draft.audio,
                            &mut self.sel_audio,
                            edit,
                            false,
                            |list| {
                                // A mic not used yet, a USB one before the
                                // computer's own (guide #2).
                                let used: Vec<&str> = list.iter().map(|a| a.device.as_str()).collect();
                                let free: Vec<DeviceInfo> = audio.iter().filter(|d| !used.contains(&d.name.as_str())).cloned().collect();
                                let device = devices::best_mic(&free).or(audio.first()).map(|d| d.name.clone()).unwrap_or("default".into());
                                let name = unique(list.iter().map(|a| a.name.clone()).collect(), &crate::config::audio_input_name(&device));
                                Some(AudioConfig { name, device, ..Default::default() })
                            },
                            |a, list| AudioConfig { uid: 0, name: unique(list.iter().map(|a| a.name.clone()).collect(), &format!("{} copy", a.name)), ..a.clone() },
                        );
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
            let automix_on = self.draft.mix.automix;
            ui.vertical(|ui| {
                let Some(a) = self.draft.audio.get_mut(self.sel_audio) else {
                    hint(ui, "No audio inputs. Add your microphone or audio interface.");
                    return;
                };
                let devices = &self.audio;
                let max = devices.iter().find(|d| d.name == a.device).map(|d| d.max_channels.max(1)).unwrap_or(64).min(64);
                let level = input_level(&self.original, &info.input_meters, a.uid);
                let unapplied = self.original.audio.iter().all(|o| o.uid != a.uid || a.uid == 0);
                egui::Grid::new("audio_detail").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                    label(ui, "Name");
                    ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut a.name));
                    ui.end_row();
                    label(ui, "Level");
                    match level {
                        Some(db) => {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(vec2(240.0, 10.0), egui::Sense::hover());
                                paint_level(ui, rect, db);
                                ui.label(RichText::new(format!("{:>4.0} dB", db.max(-99.0))).monospace().color(TEXT_DIM));
                            });
                        }
                        None if unapplied => hint(ui, "Apply (below) to hear it"),
                        None => hint(ui, "no sound yet (the input isn't open)"),
                    }
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
                    gain_slider(ui, &mut a.gain_db);
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
                            hint(ui, "(ducking is off: Sound › Mixer)");
                        }
                    });
                    ui.end_row();
                    label(ui, "Auto-mix");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut a.automix, "a mic: up while talking, down while others talk");
                        if a.automix && !automix_on {
                            hint(ui, "(automatic mic mixing is off: Sound › Mixer)");
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
        egui::Grid::new("program").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Size");
            size_combo(ui, "prog_size", &mut p.width, &mut p.height, (7680, 4320));
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
        egui::Grid::new("style").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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

    fn recording(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Recording", "Where takes go, how they're encoded, and what's saved with them for editing.");
        let scan = self.encoders.lock_ok().clone();
        let show = self.draft.ui.profile.clone();
        let o = &mut self.draft.output;
        let (pw, ph, pfps) = (self.draft.program.width, self.draft.program.height, self.draft.program.fps);
        egui::Grid::new("recording").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
                let named = crate::config::take_prefix_for(&o.take_prefix, &show);
                let mut h = format!("→ {named}_{}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
                if o.take_prefix.trim().is_empty() || o.take_prefix.trim().eq_ignore_ascii_case("take") {
                    h.push_str(if show.trim().is_empty() { "  (choose a show and takes are named after it)" } else { "  (named after the show)" });
                }
                hint(ui, &h);
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
        });
        {
            let cfg = &self.draft;
            // The take's measured rate only for the settings it's recording
            // with; for any others, the estimate for them (numbers audit #13).
            let draft_rate = crate::engine::health::estimate_for(cfg);
            let measured = info.disk_rate.filter(|(_, take)| *take == draft_rate).map(|(r, _)| r);
            let rate = measured.unwrap_or(draft_rate);
            let free = crate::engine::health::disk_space(&cfg.output.folder).map(|(f, _)| f);
            ui.add_space(4.0);
            hint(ui, &format!(
                "These settings write about {}/hour ({}){}.",
                crate::util::human_size((rate * 3600.0) as u64),
                if measured.is_some() { "measured on this take" } else { "estimated" },
                free.map(|f| format!(" — the drive has room for about {}", crate::engine::health::fmt_left(f as f64 / rate.max(1.0)))).unwrap_or_default()
            ));
            hint(ui, "Takes are written so that after a crash or a power cut they keep everything up to the last second; Cutaway mends them next time it opens.");
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
            egui::Grid::new("rec_quality_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
                        ui.label(RichText::new("ProRes needs a Mac").color(TEXT_DIM).size(TEXT_BODY));
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
            (o.clean_program, "program without graphics"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| n)
        .collect();
        let state = if extras.is_empty() { "nothing extra".to_string() } else { extras.join(", ") };
        part(ui, "rec_editing", "For editing", &state, false, |ui| {
            egui::Grid::new("rec_editing_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
                                Ok(p) => {
                                    // The script builds from what each take saves.
                                    o.resolve_project = true;
                                    (format!("Installed in {}. Takes from now on can be built in Resolve: Workspace → Scripts → Cutaway - Build project from a take.", p.display()), false)
                                }
                                Err(e) => (format!("Couldn't install it: {e:#}"), true),
                            });
                        }
                    });
                    if let Some((note, err)) = &self.resolve_note {
                        ui.label(RichText::new(note).color(if *err { AMBER } else { GREEN }).size(TEXT_BODY));
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
                label(ui, "Program without graphics");
                ui.checkbox(&mut o.clean_program, "Program_Clean, to redo titles and logos in the edit");
                ui.end_row();
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
            egui::Grid::new("podcast").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
        let scheduled = self.draft.schedule.start_time().is_some() || !self.draft.schedule.youtube_id.is_empty();
        if self.draft.streaming.destinations.is_empty() && !scheduled {
            self.streaming_empty(ui);
            return;
        }
        title(ui, "Streaming", "Go live to any number of places at once: the program is encoded once and sent to each.");
        let scan = self.encoders.lock_ok().clone();
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
                        ui.checkbox(&mut d.enabled, "").on_hover_text("Go live here: include this destination when you go live. Untick it to keep it set up but skip it.");
                        let warn = if d.problem().is_some() { "  ⚠" } else { "" };
                        let b = egui::Button::selectable(
                            self.sel_dest == i,
                            RichText::new(format!("{}{warn}\n{}", d.name, d.service.label())).size(TEXT_STATE),
                        )
                        .min_size(vec2(186.0, 40.0));
                        if ui.add(b).clicked() {
                            self.sel_dest = i;
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("add_dest").selected_text("＋ Add").width(90.0).show_ui(ui, |ui| {
                        for s in Service::ALL {
                            if ui.selectable_label(false, s.label()).clicked() {
                                self.add_destination(s);
                            }
                        }
                    });
                    if let Some(edit) = list_buttons(ui, n, self.sel_dest, None) {
                        let changed = apply_list_edit(
                            &mut self.draft.streaming.destinations,
                            &mut self.sel_dest,
                            edit,
                            false,
                            |_| None,
                            |d, list| StreamDestination { name: unique(list.iter().map(|d| d.name.clone()).collect(), &format!("{} copy", d.name)), ..d.clone() },
                        );
                        if changed {
                            // Which keys are showing is by place.
                            self.show_keys.clear();
                        }
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let sel = self.sel_dest;
                let Some(d) = self.draft.streaming.destinations.get_mut(sel) else {
                    hint(ui, "Add YouTube, Twitch, Facebook, Kick, any RTMP server, or an SRT receiver. Once there's one, Show → Viewer comments appears too, to put your stream's chat on screen.");
                    return;
                };
                egui::Grid::new("dest_detail").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
        // The frame rate it's really sent at: never faster than the program's
        // (numbers audit #20).
        let program_fps = self.draft.program.fps;
        let sent_fps = s.fps.min(program_fps).max(1);
        let state = format!("{}×{} · {} fps · {:.1} Mbps", s.width, s.height, sent_fps, up);
        part(ui, "stream_quality", "Quality", &state, false, |ui| {
            egui::Grid::new("stream_quality_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Size");
                size_combo(ui, "stream_size", &mut s.width, &mut s.height, (3840, 2160));
                ui.end_row();
                label(ui, "Frame rate");
                ui.horizontal(|ui| {
                    fps_picker(ui, "stream_fps", &mut s.fps, None);
                    if s.fps > program_fps {
                        hint(ui, &format!("sent at {program_fps} fps: the program's own rate (Scenes › Program picture)"));
                    }
                });
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
            egui::Grid::new("stream_rel_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
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
        self.scheduled(ui);
    }

    /// A new destination of this service, picked.
    fn add_destination(&mut self, s: Service) {
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

    /// Record › Streaming for a show that only records: what streaming
    /// would add, and one way to start (nothing about it shows on the desk
    /// until then).
    fn streaming_empty(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(520.0);
            ui.label(RichText::new("Nowhere to stream to yet").size(TEXT_HEADING).color(Color32::WHITE));
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new("This show records only, so the desk shows nothing about streaming. Add where to go live (YouTube, Twitch, Facebook, Kick, any RTMP server or an SRT receiver) and GO LIVE appears beside REC.").color(TEXT_DIM)).wrap());
            ui.add_space(14.0);
            let add = ui.add(egui::Button::new(RichText::new("＋ Add a destination").size(TEXT_STATE).color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(220.0, 40.0)));
            let mut picked = None;
            egui::Popup::from_toggle_button_response(&add).show(|ui| {
                ui.set_min_width(220.0);
                for s in Service::ALL {
                    if ui.add(egui::Button::new(RichText::new(s.label()).size(TEXT_BODY)).frame(false).min_size(vec2(210.0, 30.0))).clicked() {
                        picked = Some(s);
                        ui.close();
                    }
                }
            });
            if let Some(s) = picked {
                self.add_destination(s);
            }
        });
    }

    // ---- Diagnostics ----------------------------------------------------------------

    fn save_report(&mut self, info: &SettingsContext) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Zip", &["zip"])
            .set_file_name(crate::report::file_name())
            .save_file()
        else {
            return;
        };
        if self.video.is_empty() && self.audio.is_empty() {
            self.video = devices::list(Kind::Video);
            self.audio = devices::list(Kind::Audio);
        }
        let devices: Vec<String> = self
            .video
            .iter()
            .map(|d| format!("Video: {} {}", d.name, d.summary))
            .chain(self.audio.iter().map(|d| format!("Audio: {} {}", d.name, d.summary)))
            .collect();
        let report = crate::report::Report { cfg: &self.draft, facts: &info.engine_facts, devices: &devices, log: crate::logging::path() };
        self.message = Some(match report.write(&path) {
            Ok(()) => {
                crate::logging::reveal_file(&path);
                (format!("Saved {}. Send it by email or attach it to your message.", path.display()), false)
            }
            Err(e) => (format!("Couldn't save the report: {e:#}"), true),
        });
    }

    fn diagnostics(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Diagnostics", "What the engine is using, connected devices, and the log.");
        egui::Grid::new("facts").num_columns(2).spacing(TABLE_GAP).show(ui, |ui| {
            label(ui, "Version");
            ui.label(format!("Cutaway {}", env!("CARGO_PKG_VERSION")));
            ui.end_row();
            label(ui, "Log file");
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::logging::path().display().to_string()).monospace().color(TEXT_DIM));
                if ui.small_button("Show").clicked() {
                    crate::logging::reveal();
                }
            });
            ui.end_row();
            label(ui, "Problem report");
            ui.vertical(|ui| {
                if ui.button("Save a problem report…").clicked() {
                    self.save_report(info);
                }
                hint(ui, "One .zip to send when something goes wrong: the log, the last take's note, your settings and devices. Stream keys and passwords are taken out.");
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
                    ui.label(RichText::new(format!("    {}", d.summary)).size(TEXT_BODY).color(TEXT_DIM));
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
                    LogLevel::Error => ERROR,
                };
                ui.label(RichText::new(format!("{}  {}", l.time, l.text)).monospace().size(TEXT_BODY).color(color));
            }
        });
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
    job.append(name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(TEXT_HEADING), Color32::WHITE));
    if !state.is_empty() {
        job.append(&format!("   {state}"), 0.0, egui::TextFormat::simple(egui::FontId::proportional(TEXT_BODY), TEXT_DIM));
    }
    let mut header = egui::CollapsingHeader::new(job).default_open(open);
    if help::take_open_part(&format!("{id:?}")) {
        header = header.open(Some(true));
    }
    let r = header.id_salt(id).show(ui, |ui| {
        ui.add_space(4.0);
        let r = body(ui);
        ui.add_space(6.0);
        r
    });
    ui.add_space(4.0);
    r.body_returned
}

pub(super) fn subtitle(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(TEXT_BODY).strong().color(TEXT_DIM).extra_letter_spacing(1.0));
    ui.add_space(2.0);
}

/// The width every setting's name takes, so rows line up down a page.
const LABEL_W: f32 = 196.0;

/// How much larger Settings' text is than the main window's.
const TEXT_SCALE: f32 = 1.12;

/// A setting's name, with an ⓘ that explains it (on hover) when there's
/// more to say.
pub(super) fn label(ui: &mut egui::Ui, text: &str) {
    let help = help::get(text);
    ui.allocate_ui_with_layout(vec2(LABEL_W, 20.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_width(LABEL_W);
        ui.spacing_mut().item_spacing.x = 5.0;
        let r = ui.add(egui::Label::new(RichText::new(text).color(Color32::from_rgb(200, 205, 214))).selectable(false).sense(egui::Sense::hover()));
        if let Some((glow, first)) = help::lit(text) {
            // Found with ⌘K search: scrolled to, and lit for a moment.
            if first {
                r.scroll_to_me(Some(Align::Center));
            }
            ui.painter().rect_filled(r.rect.expand2(vec2(6.0, 3.0)), 4.0, ACCENT.gamma_multiply(0.45 * glow));
            ui.ctx().request_repaint();
        }
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
    ui.set_max_width(360.0);
    ui.label(RichText::new(text).size(TEXT_STATE));
}

/// A table's column heading, explained on hover like a setting's name.
pub(super) fn column_head(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(text).color(TEXT_DIM).size(TEXT_BODY));
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
    let (rect, resp) = ui.allocate_exact_size(vec2(15.0, 15.0), egui::Sense::hover());
    let c = if resp.hovered() { ACCENT } else { Color32::from_rgb(120, 128, 142) };
    let p = ui.painter();
    p.circle_stroke(rect.center(), 6.5, egui::Stroke::new(1.1, c));
    p.text(rect.center() + vec2(0.0, 0.5), egui::Align2::CENTER_CENTER, "i", egui::FontId::proportional(TEXT_SMALL), c);
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
    ui.add(egui::Label::new(RichText::new(text).size(TEXT_BODY).color(TEXT_DIM)).wrap());
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
        d if crate::platform::capture::is_screen(d) => crate::platform::capture::ScreenTarget::parse(d).map(|t| t.describe()).unwrap_or_default(),
        "default" => "System default".into(),
        "media" => "Media player".into(),
        "pads" => "Sound pads".into(),
        d if d.starts_with("source:") => format!("Sound from {}", d[7..].trim()),
        d if d.starts_with("app:") => format!("Sound from {}", d[4..].trim()),
        d if d.starts_with("ndi:") => format!("NDI {}", d[4..].trim()),
        d if d.starts_with("frame:") => format!("Auto-framed from {}", d[6..].trim()),
        d if d.starts_with("vptz:") => format!("Virtual PTZ in {}", d[5..].trim()),
        d if d.starts_with("guest:") => "Remote guest (link)".into(),
        d if d.starts_with("guest-screen:") => "Guest's screen".into(),
        d if d.starts_with("phone:") => "Phone camera".into(),
        d if d.starts_with("srt-in:") => format!("SRT input :{}", &d[7..]),
        d if d.starts_with("rtmp-in:") => format!("RTMP input :{}", d[8..].split('/').next().unwrap_or("")),
        d => d.chars().take(28).collect(),
    }
}

/// The gaps in a page's settings (name on the left, setting on the right)
/// and in its tables: the same on every page.
pub(super) const FORM_GAP: [f32; 2] = [16.0, 10.0];
pub(super) const TABLE_GAP: [f32; 2] = [12.0, 8.0];

/// What the buttons on a row of a table can ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RowEdit {
    Up,
    Down,
    Remove,
}

/// The same small buttons at the end of every row that can move or go
/// (row `i` of `n`): ↑, ↓ (when `movable`) and ✕.
pub(super) fn row_buttons(ui: &mut egui::Ui, i: usize, n: usize, movable: bool) -> Option<RowEdit> {
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if movable {
            if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Move up").clicked() {
                edit = Some(RowEdit::Up);
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).on_hover_text("Move down").clicked() {
                edit = Some(RowEdit::Down);
            }
        }
        if ui.add(egui::Button::new("✕").small()).on_hover_text("Remove").clicked() {
            edit = Some(RowEdit::Remove);
        }
    });
    edit
}

/// Do what a row's buttons asked to `list` (`i`: the row).
pub(super) fn apply_row_edit<T>(list: &mut Vec<T>, i: usize, edit: RowEdit) {
    match edit {
        RowEdit::Up if i > 0 && i < list.len() => list.swap(i, i - 1),
        RowEdit::Down if i + 1 < list.len() => list.swap(i, i + 1),
        RowEdit::Remove if i < list.len() => {
            list.remove(i);
        }
        _ => {}
    }
}

/// What the buttons under a list can ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ListEdit {
    Add,
    Duplicate,
    Remove,
    Up,
    Down,
}

/// The same buttons under every list in Settings, in the same order:
/// ＋ Add (`add`: its label, or None when the list has buttons of its own
/// for that), Duplicate, Remove, ↑, ↓. Put a list's special buttons after.
pub(super) fn list_buttons(ui: &mut egui::Ui, n: usize, sel: usize, add: Option<&str>) -> Option<ListEdit> {
    let mut edit = None;
    if let Some(label) = add {
        if ui.button(label).clicked() {
            edit = Some(ListEdit::Add);
        }
    }
    if ui.add_enabled(n > 0, egui::Button::new("Duplicate")).clicked() {
        edit = Some(ListEdit::Duplicate);
    }
    if ui.add_enabled(n > 0, egui::Button::new("Remove")).clicked() {
        edit = Some(ListEdit::Remove);
    }
    if ui.add_enabled(n > 0 && sel > 0, egui::Button::new("↑")).on_hover_text("Move up").clicked() {
        edit = Some(ListEdit::Up);
    }
    if ui.add_enabled(n > 0 && sel + 1 < n, egui::Button::new("↓")).on_hover_text("Move down").clicked() {
        edit = Some(ListEdit::Down);
    }
    edit
}

/// Do what a list's buttons asked, keeping `sel` on the same item. `new`
/// makes the item for ＋ Add (None: nothing), `copy` the copy for Duplicate
/// (given the item and the list, to name it); an added item goes after the
/// picked one if `add_after`, otherwise last. True if the list changed.
pub(super) fn apply_list_edit<T>(list: &mut Vec<T>, sel: &mut usize, edit: ListEdit, add_after: bool, new: impl FnOnce(&[T]) -> Option<T>, copy: impl FnOnce(&T, &[T]) -> T) -> bool {
    let n = list.len();
    let at = (*sel).min(n.saturating_sub(1));
    match edit {
        ListEdit::Add => {
            let Some(item) = new(list) else { return false };
            let to = if add_after && n > 0 { at + 1 } else { n };
            list.insert(to, item);
            *sel = to;
        }
        ListEdit::Duplicate if n > 0 => {
            let item = copy(&list[at], list);
            list.insert(at + 1, item);
            *sel = at + 1;
        }
        ListEdit::Remove if n > 0 => {
            list.remove(at);
            *sel = at.min(list.len().saturating_sub(1));
        }
        ListEdit::Up if at > 0 => {
            list.swap(at, at - 1);
            *sel = at - 1;
        }
        ListEdit::Down if at + 1 < n => {
            list.swap(at, at + 1);
            *sel = at + 1;
        }
        _ => return false,
    }
    true
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
            ui.label(RichText::new("Sound from an app").color(TEXT_DIM).size(TEXT_BODY));
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

/// Picture size: a list of common sizes, and "Custom…" for typing width and height.
/// The number boxes only show for a custom size (or after picking Custom…).
pub(super) fn size_combo(ui: &mut egui::Ui, id: &str, w: &mut u32, h: &mut u32, max: (u32, u32)) {
    let key = ui.make_persistent_id(("custom_size", id));
    let asked: Option<(u32, u32)> = ui.data(|d| d.get_temp(key));
    let preset = RESOLUTIONS.iter().find(|r| r.0 == *w && r.1 == *h);
    let custom = preset.is_none() || asked == Some((*w, *h));
    let text = preset.filter(|_| !custom).map(|r| r.2.to_string()).unwrap_or_else(|| "Custom…".into());
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id).selected_text(text).width(190.0).show_ui(ui, |ui| {
            for &(rw, rh, label) in RESOLUTIONS {
                if ui.selectable_label(!custom && *w == rw && *h == rh, label).clicked() {
                    *w = rw;
                    *h = rh;
                    ui.data_mut(|d| d.remove_temp::<(u32, u32)>(key));
                }
            }
            if ui.selectable_label(custom, "Custom…").clicked() {
                ui.data_mut(|d| d.insert_temp(key, (*w, *h)));
            }
        });
        if custom {
            let before = (*w, *h);
            ui.add(egui::DragValue::new(w).range(160..=max.0).speed(2));
            ui.label("×");
            ui.add(egui::DragValue::new(h).range(90..=max.1).speed(2));
            if (*w, *h) != before && asked.is_some() {
                ui.data_mut(|d| d.insert_temp(key, (*w, *h)));
            }
        }
    });
}

pub(super) fn fps_picker(ui: &mut egui::Ui, id: &str, fps: &mut u32, allowed: Option<&[u32]>) {
    ui.horizontal(|ui| {
        let options: Vec<u32> = allowed.map(|a| a.to_vec()).unwrap_or_else(|| RATES.to_vec());
        let key = ui.make_persistent_id(("custom_fps", id));
        let asked: Option<u32> = ui.data(|d| d.get_temp(key));
        let custom = !options.contains(fps) || asked == Some(*fps);
        let text = if custom && allowed.is_none() { "Custom…".to_string() } else { format!("{fps} fps") };
        egui::ComboBox::from_id_salt(id).selected_text(text).width(90.0).show_ui(ui, |ui| {
            for r in &options {
                if ui.selectable_label(!custom && fps == r, format!("{r} fps")).clicked() {
                    *fps = *r;
                    ui.data_mut(|d| d.remove_temp::<u32>(key));
                }
            }
            if ui.selectable_label(custom, "Custom…").clicked() {
                ui.data_mut(|d| d.insert_temp(key, *fps));
            }
        });
        if custom {
            let before = *fps;
            ui.add(egui::DragValue::new(fps).range(1..=120).suffix(" fps"));
            if *fps != before && asked.is_some() {
                ui.data_mut(|d| d.insert_temp(key, *fps));
            }
        }
        if let Some(a) = allowed {
            if !a.contains(fps) {
                ui.label(RichText::new("(converted from the device's rate)").size(TEXT_BODY).color(TEXT_DIM));
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

/// Wherever NDI is chosen: when it isn't installed, offer to install NDI's
/// own free runtime (downloaded from NDI, its installer opened), and credit
/// NDI as its licence asks.
/// The displays drawn as they're arranged (like the system's Displays
/// settings), each with its number, name and size; click one to choose it.
/// `ours` is the one Cutaway's window is on. Returns the one clicked.
pub(crate) fn display_map(ui: &mut egui::Ui, displays: &[crate::platform::displays::Display], selected: Option<usize>, ours: Option<usize>, height: f32) -> Option<usize> {
    display_map_with(ui, displays, selected, ours, height, &[])
}

/// [`display_map`] with a picture of what's on each display (by index).
pub(crate) fn display_map_with(ui: &mut egui::Ui, displays: &[crate::platform::displays::Display], selected: Option<usize>, ours: Option<usize>, height: f32, pictures: &[Option<egui::TextureId>]) -> Option<usize> {
    use eframe::egui::{pos2, Align2, FontId, Rect, Sense, Stroke, StrokeKind};
    if displays.is_empty() {
        return None;
    }
    let (min_x, min_y) = displays.iter().fold((f32::MAX, f32::MAX), |(x, y), d| (x.min(d.x), y.min(d.y)));
    let (max_x, max_y) = displays.iter().fold((f32::MIN, f32::MIN), |(x, y), d| (x.max(d.x + d.w), y.max(d.y + d.h)));
    let width = ui.available_width().min(560.0);
    let scale = (width / (max_x - min_x)).min(height / (max_y - min_y));
    let size = vec2((max_x - min_x) * scale, (max_y - min_y) * scale);
    let (area, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(area.expand(2.0));
    let mut clicked = None;
    for (i, d) in displays.iter().enumerate() {
        let r = Rect::from_min_size(pos2(area.left() + (d.x - min_x) * scale, area.top() + (d.y - min_y) * scale), vec2(d.w * scale, d.h * scale)).shrink(3.0);
        let resp = ui.interact(r, ui.scope_id().with(("display", i)), Sense::click());
        let chosen = selected == Some(i);
        let fill = if chosen { ACCENT.gamma_multiply(0.35) } else if resp.hovered() { RAISED.gamma_multiply(1.4) } else { RAISED };
        painter.rect_filled(r, 6.0, fill);
        if let Some(Some(tex)) = pictures.get(i) {
            let pic = r.shrink(4.0);
            painter.image(*tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            // Darker, so the number and name read over it.
            painter.rect_filled(pic, 3.0, Color32::from_black_alpha(if chosen { 70 } else { 120 }));
        }
        painter.rect_stroke(r, 6.0, Stroke::new(if chosen { 2.5 } else { 1.0 }, if chosen { ACCENT } else { LINE }), StrokeKind::Inside);
        if d.main {
            // The menu bar / taskbar side.
            let bar = if cfg!(windows) { Rect::from_min_max(pos2(r.left() + 6.0, r.bottom() - 9.0), pos2(r.right() - 6.0, r.bottom() - 5.0)) } else { Rect::from_min_max(pos2(r.left() + 6.0, r.top() + 5.0), pos2(r.right() - 6.0, r.top() + 9.0)) };
            painter.rect_filled(bar, 2.0, Color32::from_white_alpha(60));
        }
        let small = r.height() < 70.0 || r.width() < 110.0;
        painter.text(r.center() - vec2(0.0, if small { 0.0 } else { 14.0 }), Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(if small { 16.0 } else { 22.0 }), Color32::WHITE);
        if !small {
            let mut name = d.name.clone();
            let max_chars = ((r.width() - 12.0) / 6.5).max(4.0) as usize;
            if name.chars().count() > max_chars {
                name = name.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…";
            }
            painter.text(r.center() + vec2(0.0, 8.0), Align2::CENTER_CENTER, name, FontId::proportional(TEXT_SMALL), Color32::from_gray(225));
            let note = if ours == Some(i) { format!("{}×{} · Cutaway is here", d.pixels.0, d.pixels.1) } else { format!("{}×{}", d.pixels.0, d.pixels.1) };
            painter.text(r.center() + vec2(0.0, 23.0), Align2::CENTER_CENTER, note, FontId::proportional(TEXT_SMALL), TEXT_DIM);
        }
        let resp = resp.on_hover_text(format!("{} {}: {}×{}{}", i + 1, d.name, d.pixels.0, d.pixels.1, if d.main { " (main display)" } else { "" }));
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// The display Cutaway's main window is on, if known.
pub(crate) fn our_display(ctx: &egui::Context, displays: &[crate::platform::displays::Display]) -> Option<usize> {
    use crate::platform::displays;
    let (outer, native) = ctx.input_for(egui::ViewportId::ROOT, |i| (i.viewport().outer_rect, i.viewport().native_pixels_per_point.unwrap_or(1.0)));
    let zoom = ctx.zoom_factor();
    outer.and_then(|r| displays::containing(displays, displays::window_units([r.center().x * zoom, r.center().y * zoom], native)))
}

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
        ui.label(RichText::new(crate::net::ndi_install::TRADEMARK).size(TEXT_BODY).color(TEXT_DIM));
        ui.hyperlink_to(RichText::new("ndi.video").size(TEXT_BODY), crate::net::ndi_install::SITE);
    });
}

/// The place in the settings in effect of the input with this identity
/// (the running inputs, their levels and ON are by place).
fn running_input(applied: &Config, uid: u64) -> Option<usize> {
    if uid == 0 {
        return None;
    }
    applied.audio.iter().position(|a| a.uid == uid)
}

/// The live level (dB, as its meter on the desk shows it) of the applied
/// input with this identity, if it's open.
fn input_level(applied: &Config, meters: &[Option<f32>], uid: u64) -> Option<f32> {
    meters.get(running_input(applied, uid)?).copied().flatten()
}

/// Each channel's level (dB) of the applied input with this identity, as
/// the automatic switcher hears it.
pub(super) fn input_channels<'a>(applied: &Config, levels: &'a [Vec<f32>], uid: u64) -> Option<&'a Vec<f32>> {
    levels.get(running_input(applied, uid)?)
}

/// A level meter (dB) filling `rect` as far as the level reaches: green, amber near the top, red at the limit.
fn paint_level(ui: &egui::Ui, rect: egui::Rect, db: f32) {
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_black_alpha(90));
    let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        let colour = if db > -3.0 { ERROR } else if db > -12.0 { AMBER } else { GREEN };
        p.rect_filled(egui::Rect::from_min_size(rect.min, vec2(rect.width() * frac, rect.height())), 2.0, colour);
    }
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
            disk_rate: None,
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
            input_meters: Vec::new(),
            input_on: Vec::new(),
            talkback_key: String::new(),
            syrin: Default::default(),
            stinger: None,
            guest_reach: Default::default(),
            guest_base: None,
            guest_status: Vec::new(),
            ndi_watchers: None,
            on_air: Vec::new(),
            on_air_scene: None,
            undo: None,
            redo: None,
            pictures: Vec::new(),
            virtual_ptz: Vec::new(),
        }
    }

    /// Apply hands over the settings it was opened on with the edit, so the
    /// app puts on only what was changed here: a fader moved on the desk
    /// while Settings was open keeps its level (numbers audit #1), and a
    /// level changed here does apply. After a save, the next Apply starts
    /// from there.
    #[test]
    fn apply_keeps_desk_changes_made_meanwhile() {
        let mut cfg = Config::default();
        cfg.audio = vec![
            crate::config::AudioConfig { name: "Mic".into(), channels: 1, in_program: true, ..Default::default() },
            crate::config::AudioConfig { name: "Guest".into(), channels: 1, in_program: true, ..Default::default() },
        ];
        cfg.assign_uids();
        let mut live = cfg.clone();
        let mut win = SettingsUi::new(cfg);
        win.draft.audio[1].delay_ms = 40;
        // On the desk, while Settings is open.
        live.audio[0].gain_db = -18.0;
        live.transition.duration_ms = 1200;
        let SettingsAction::Apply { cfg, base, .. } = win.apply(false, &ctx(false)) else { panic!("applies") };
        let now = live.with_edits(&base, &cfg);
        assert_eq!(now.audio[0].gain_db, -18.0, "the fader moved on the desk stays");
        assert_eq!(now.transition.duration_ms, 1200);
        assert_eq!(now.audio[1].delay_ms, 40, "the change made in Settings applies");
        win.applied(Ok(()), &now);
        assert!(!win.dirty());
        // A level changed in Settings applies too, and the desk's other
        // change from before isn't undone by it.
        win.draft.audio[1].gain_db = -6.0;
        let SettingsAction::Apply { cfg, base, .. } = win.apply(false, &ctx(false)) else { panic!("applies") };
        let next = now.with_edits(&base, &cfg);
        assert_eq!((next.audio[0].gain_db, next.audio[1].gain_db), (-18.0, -6.0));
        assert_eq!(next.transition.duration_ms, 1200);
    }

    /// Just showing a level that isn't on a 0.5 dB step, or is below −40 dB,
    /// leaves it as it is, so the page doesn't look edited.
    #[test]
    fn showing_a_level_doesnt_change_it() {
        let ctx = egui::Context::default();
        for start in [-3.3, -55.0, 0.0, 12.0] {
            let mut g = start;
            for _ in 0..3 {
                let mut out = ctx.run_ui(Default::default(), |ui| {
                    gain_slider(ui, &mut g);
                });
                out.textures_delta.clear();
            }
            assert_eq!(g, start);
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

    /// Splitting a camera makes a close-up per person, side by side, cut
    /// from it and placed after it.
    #[test]
    fn splits_a_camera_into_close_ups() {
        let mut cfg = Config::default();
        cfg.sources = vec![
            SourceConfig { name: "Wide".into(), device: "test:bars".into(), width: 3840, height: 2160, ..Default::default() },
            SourceConfig { name: "Screen".into(), device: "test:gradient".into(), ..Default::default() },
        ];
        let mut win = SettingsUi::new(cfg).with_split(0, 2);
        let names: Vec<&str> = win.draft.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Wide", "Wide left", "Wide right", "Screen"]);
        assert_eq!(win.sel_source, 1);
        let right = &win.draft.sources[2];
        assert_eq!(right.virtual_ptz_of(), Some("Wide"));
        assert_eq!((right.ptz.presets[0].x, right.ptz.presets[0].zoom), (0.75, 2.0));
        // Pixel for pixel: half a 4K picture is 1080p.
        assert_eq!((right.width, right.height), (1920, 1080));
        assert!(win.draft.validate().is_ok(), "{:?}", win.draft.validate());
        // Again: three more, after the first two, with names of their own.
        win.split_source(0, 3);
        let names: Vec<&str> = win.draft.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names[3..6], ["Wide left 2", "Wide middle", "Wide right 2"]);
        // A close-up isn't split again.
        let before = win.draft.sources.len();
        win.split_source(1, 2);
        assert_eq!(win.draft.sources.len(), before);
    }

    /// The buttons under every list act the same way.
    #[test]
    fn list_buttons_add_copy_remove_and_move() {
        let mut v = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut sel = 1;
        let new = |l: &[String]| Some(format!("new{}", l.len()));
        let copy = |x: &String, _: &[String]| format!("{x} copy");
        assert!(apply_list_edit(&mut v, &mut sel, ListEdit::Add, false, new, copy));
        assert_eq!((v.as_slice(), sel), (&["a".to_string(), "b".into(), "c".into(), "new3".into()][..], 3));
        sel = 1;
        assert!(apply_list_edit(&mut v, &mut sel, ListEdit::Add, true, new, copy));
        assert_eq!((v[2].as_str(), sel), ("new4", 2));
        sel = 0;
        apply_list_edit(&mut v, &mut sel, ListEdit::Duplicate, false, new, copy);
        assert_eq!((v[1].as_str(), sel), ("a copy", 1));
        apply_list_edit(&mut v, &mut sel, ListEdit::Down, false, new, copy);
        assert_eq!((v[2].as_str(), sel), ("a copy", 2));
        apply_list_edit(&mut v, &mut sel, ListEdit::Up, false, new, copy);
        apply_list_edit(&mut v, &mut sel, ListEdit::Up, false, new, copy);
        assert_eq!((v[0].as_str(), sel), ("a copy", 0));
        // Not past the ends.
        assert!(!apply_list_edit(&mut v, &mut sel, ListEdit::Up, false, new, copy));
        // Remove keeps a valid pick, also for the last; an empty list stays so.
        sel = v.len() - 1;
        apply_list_edit(&mut v, &mut sel, ListEdit::Remove, false, new, copy);
        assert_eq!(sel, v.len() - 1);
        let mut none: Vec<String> = Vec::new();
        let mut s0 = 0;
        for e in [ListEdit::Remove, ListEdit::Duplicate, ListEdit::Up, ListEdit::Down] {
            assert!(!apply_list_edit(&mut none, &mut s0, e, false, new, copy));
        }
        // A list with its own add buttons: ＋ Add does nothing here.
        assert!(!apply_list_edit(&mut v, &mut sel, ListEdit::Add, false, |_| None, copy));
    }

    /// An input's live level is found by its identity in what's running,
    /// wherever it now sits in the list; new ones have none.
    #[test]
    fn input_levels_follow_the_input_not_its_place() {
        let mut cfg = Config::default();
        cfg.audio = vec![AudioConfig { name: "A".into(), uid: 11, ..Default::default() }, AudioConfig { name: "B".into(), uid: 12, ..Default::default() }];
        let meters = vec![Some(-20.0), Some(-6.0)];
        assert_eq!(input_level(&cfg, &meters, 11), Some(-20.0));
        assert_eq!(input_level(&cfg, &meters, 12), Some(-6.0));
        assert_eq!(input_level(&cfg, &meters, 99), None);
        assert_eq!(input_level(&cfg, &meters, 0), None);
        assert_eq!(input_level(&cfg, &[], 11), None);
        assert_eq!(input_level(&cfg, &[None, Some(-6.0)], 11), None, "not open yet");
        let levels = vec![vec![-30.0, -20.0], vec![-6.0]];
        assert_eq!(input_channels(&cfg, &levels, 12), Some(&vec![-6.0]));
        assert_eq!(input_channels(&cfg, &levels, 0), None);
    }

    /// While recording, a change that restarts a source asks first (with
    /// the close-ups cut from it); a new name title doesn't.
    #[test]
    fn asks_before_restarting_while_recording() {
        let mut cfg = Config::default();
        cfg.sources = vec![
            SourceConfig { name: "Wide".into(), device: "test:bars".into(), ..Default::default() },
            SourceConfig { name: "Alex".into(), device: "vptz:Wide".into(), ..Default::default() },
        ];
        cfg.assign_uids();
        let mut win = SettingsUi::new(cfg.clone());
        win.draft.sources[1].person = "Alex Smith".into();
        win.draft.sources[1].role = "Host".into();
        assert!(matches!(win.apply(false, &ctx(true)), SettingsAction::Apply { .. }));
        let mut win = SettingsUi::new(cfg.clone());
        win.draft.sources[0].delay_ms = 80;
        assert!(matches!(win.apply(false, &ctx(true)), SettingsAction::None));
        let check = win.confirm_remove.take().expect("asks");
        assert!(check.names.is_empty());
        assert_eq!(check.restarting, vec!["Wide".to_string(), "Alex".to_string()]);
        assert!(matches!(win.apply(false, &ctx(false)), SettingsAction::Apply { .. }));
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
