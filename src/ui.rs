//! Director's desk: program (and preview) monitors, multiview, scene bank,
//! graphics, audio strips with loudness, and everything around them:
//! markers, health, the pre-show check, a second-screen output, show
//! profiles, and commands from the phone remote and MIDI controllers.

use crate::util::LockExt;
use crate::config::{Config, DisplayOutput, TransitionMode};
use crate::control::state::{self as show, ShowState};
use crate::control::shortcuts::{self, Binding, Builtin, KeyCombo, Keymap};
use crate::control::{self, midi, osc, profiles, remote, Action, MediaCmd, Request, Toggle};
use crate::engine::player::PlayerCmd;
use crate::engine::precheck::{self, CheckItem, Level};
use crate::engine::streamer::{self, DestState};
use crate::engine::{Engine, FrameSlot, LogLevel, RecState, Waker};
use crate::scene::{Layer, Scene};
use crate::settings::{Section, SettingsAction, SettingsContext, SettingsUi};
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Key, Rect, RichText, Sense, Stroke, StrokeKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod panels;
mod production;
mod starting;
mod undo;
mod uploads;
mod waiting;

pub mod theme {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(14, 16, 20);
    pub const PANEL: Color32 = Color32::from_rgb(22, 25, 31);
    pub const RAISED: Color32 = Color32::from_rgb(34, 38, 46);
    pub const LINE: Color32 = Color32::from_rgb(52, 57, 68);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 147, 160);
    pub const RED: Color32 = Color32::from_rgb(232, 52, 64);
    pub const RED_DIM: Color32 = Color32::from_rgb(120, 30, 38);
    pub const AMBER: Color32 = Color32::from_rgb(245, 166, 35);
    pub const GREEN: Color32 = Color32::from_rgb(58, 196, 112);
    pub const GREEN_DIM: Color32 = Color32::from_rgb(28, 96, 58);
    pub const ACCENT: Color32 = Color32::from_rgb(90, 160, 255);
    /// "On air" for streaming, so it reads differently from recording red.
    pub const LIVE: Color32 = Color32::from_rgb(214, 40, 120);
}
use theme::*;

/// Cutaway's icon for its windows and, while it runs, the Dock or taskbar
/// (drawn by assets/logo/make_logo.py). Without it they'd show egui's.
fn app_icon() -> Arc<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../assets/logo/window-mac.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../assets/logo/window-square.png");
    Arc::new(eframe::icon_data::from_png_bytes(png).unwrap_or_default())
}

/// The logo's letters (assets/logo/mark.png), for the top bar and the
/// About and update windows.
fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let image = match image::load_from_memory(include_bytes!("../assets/logo/mark.png")) {
        Ok(i) => {
            let i = i.into_rgba8();
            egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw())
        }
        Err(_) => egui::ColorImage::filled([1, 1], Color32::TRANSPARENT),
    };
    ctx.load_texture("cutaway-logo", image, egui::TextureOptions::LINEAR)
}

pub fn run(cfg: Config, save_path: Option<PathBuf>, self_test: bool) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Cutaway").with_icon(app_icon()).with_inner_size([1480.0, 920.0]).with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Cutaway",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc, cfg, save_path);
            if self_test {
                app.self_test = Some((Instant::now(), 0));
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

struct Drag {
    layer: usize,
    resize: bool,
    /// Moving the picture inside its frame (Alt/Option held) rather than
    /// the frame.
    pan: bool,
}

#[derive(PartialEq, Clone)]
enum Confirm {
    StopRecording,
    EndStream,
    SwitchShow(String),
    /// Closing the window while recording or live.
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
enum Then {
    Record,
    Live,
}

struct Precheck {
    items: Vec<CheckItem>,
    rx: Option<Receiver<CheckItem>>,
    waiting: usize,
    then: Option<Then>,
}

/// The lip-sync measurement dialog.
#[derive(Default)]
struct SyncWizard {
    job: Option<Arc<crate::engine::lipsync::SyncJob>>,
    /// Which suggestions to apply (video then audio).
    apply: Vec<bool>,
}

struct PeakHold {
    db: Vec<f32>,
    at: Instant,
}

pub struct App {
    cfg: Config,
    save_path: Option<PathBuf>,
    engine: Option<Engine>,
    engine_error: Option<String>,
    /// The engine being started in the background (the window carries on).
    starting: Option<starting::Starting>,
    /// Settings changed again while it was starting: start once more.
    restart_again: bool,
    /// Turn the virtual camera on once the engine is up.
    vcam_wanted: bool,
    /// Engine starts this session (the slate's take number).
    starts_count: u32,
    scenes: Vec<Scene>,
    scenes_dirty: bool,
    cfg_dirty: bool,
    textures: HashMap<String, (u64, egui::TextureHandle)>,
    waker: Waker,
    edit_mode: bool,
    selected_layer: Option<usize>,
    /// Undo and redo for scene layouts and settings.
    history: undo::History,
    drag: Option<Drag>,
    settings: Option<SettingsUi>,
    confirm: Option<Confirm>,
    /// Closing was confirmed (or nothing was running): let the window go.
    quit_ok: bool,
    log_open: bool,
    health_open: bool,
    peak_hold: HashMap<String, PeakHold>,
    toast: Option<(String, Instant, bool)>,
    /// Commands from the phone remote and MIDI.
    requests_tx: Sender<Request>,
    requests_rx: Receiver<Request>,
    remote_shared: Arc<remote::RemoteShared>,
    /// Syrinscape Online control.
    syrinscape: control::syrinscape::Syrinscape,
    /// Where the show is in its running order.
    rundown: control::rundown::Rundown,
    /// Commands waiting after a "wait" in a macro or cues.
    queued: Vec<production::Queued>,
    /// The PTZ panel's camera (source index), and what it's doing.
    ptz_cam: usize,
    ptz_drive: Option<[f32; 2]>,
    ptz_zoom: Option<f32>,
    /// The last replay problem shown.
    replay_problem: Option<String>,
    /// Slow motion ticked in the Replay button's menu.
    replay_slow: bool,
    /// A phone camera's QR code on show: its link id.
    qr_for: Option<String>,
    /// A newer Cutaway, once GitHub has said so; and its window.
    update: Arc<std::sync::Mutex<Option<crate::net::update::Release>>>,
    update_open: bool,
    /// The logo beside the name in the top bar; clicking it opens About.
    logo: egui::TextureHandle,
    about_open: bool,
    /// When this (just updated) version counts as started properly.
    settle_at: Option<Instant>,
    /// Uploads to YouTube and Google Drive.
    uploads: uploads::Uploads,
    /// The live chat, the comment on screen, and the newest one shown.
    chat: Option<crate::net::chat::Chat>,
    comment_on: Option<u64>,
    comment_at: Option<Instant>,
    comment_cursor: u64,
    /// The teleprompter.
    prompter: production::Prompter,
    /// The pad held down with the mouse, and the talkback button.
    pad_held: Option<usize>,
    talk_held: bool,
    remote: Option<remote::RemoteServer>,
    midi: Option<midi::Midi>,
    osc: Option<osc::Osc>,
    last_publish: Instant,
    precheck: Option<Precheck>,
    /// Title being edited live: (graphic, title, subtitle).
    title_edit: Option<(usize, String, String)>,
    display_fullscreen: bool,
    /// The output window's corner button shows until then (after the
    /// mouse moves over the window), like a video player's controls.
    display_controls_until: Option<Instant>,
    /// Frames left in which to give the output window the keyboard (so Esc
    /// and F reach it once it opens or goes full screen).
    display_focus_frames: u8,
    display_was_open: bool,
    /// Where the output window was put when it opened: the display (by
    /// index, for full screen) and whether it opened full screen. Fixed
    /// while it's open; choosing another screen opens a new window
    /// (`display_generation`).
    display_monitor: Option<usize>,
    display_open_full: bool,
    display_screen_applied: String,
    display_generation: u32,
    sync: Option<SyncWizard>,
    /// The transition T toggles back to from Cut.
    last_transition: TransitionMode,
    /// The keyboard shortcuts, made from the settings they came from.
    keymap: Option<(crate::config::KeysConfig, Keymap)>,
    /// Time on screen for the graphics panel's hover: worked out once a
    /// second, not every frame. (When, from when, what.)
    on_air_cache: Option<(Instant, i64, Vec<crate::engine::graphics::OnAir>)>,
    /// Keys held down for something that stops when they're let go.
    held_keys: Vec<(Key, Action)>,
    /// Takes left unfinished by a crash, being rescued.
    rescue: Option<waiting::Rescue>,
    /// Scripted actions for automated screenshots (hidden --ui-test flag).
    self_test: Option<(Instant, u8)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, mut cfg: Config, save_path: Option<PathBuf>) -> Self {
        // Every source and input gets its identity for this session.
        cfg.assign_uids();
        // The displays' names (macOS only gives them on this thread; screen
        // captures opened later find their display by name).
        crate::platform::displays::list();
        style(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(cfg.ui.scale.clamp(0.5, 3.0));
        let ctx = cc.egui_ctx.clone();
        let waker: Waker = Arc::new(move || ctx.request_repaint());
        let (requests_tx, requests_rx) = unbounded();
        let remote_shared = remote::RemoteShared::new(requests_tx.clone(), waker.clone());
        let syrinscape = control::syrinscape::Syrinscape::new(&cfg.syrinscape, waker.clone());
        // A crash mid-take: put the files back together while the window opens.
        let rescue = crate::recovery::start_rescue(&crate::recovery::folders(&cfg), waker.clone()).map(waiting::Rescue::new);
        let mut app = App {
            rescue,
            syrinscape,
            rundown: Default::default(),
            queued: Vec::new(),
            ptz_cam: 0,
            ptz_drive: None,
            ptz_zoom: None,
            replay_problem: None,
            replay_slow: false,
            qr_for: None,
            update: Arc::new(std::sync::Mutex::new(None)),
            update_open: false,
            logo: logo_texture(&cc.egui_ctx),
            about_open: false,
            settle_at: None,
            uploads: Default::default(),
            chat: None,
            comment_on: None,
            comment_at: None,
            comment_cursor: 0,
            prompter: production::Prompter { wpm: cfg.prompter.words_per_minute, ..Default::default() },
            pad_held: None,
            talk_held: false,
            scenes: cfg.effective_scenes(),
            display_fullscreen: cfg.display.fullscreen,
            display_controls_until: None,
            display_focus_frames: 0,
            display_was_open: false,
            display_monitor: None,
            display_open_full: false,
            display_screen_applied: String::new(),
            display_generation: 0,
            sync: None,
            keymap: None,
            on_air_cache: None,
            held_keys: Vec::new(),
            last_transition: if cfg.transition.mode == TransitionMode::Cut { TransitionMode::Mix } else { cfg.transition.mode },
            cfg,
            save_path,
            engine: None,
            engine_error: None,
            starting: None,
            restart_again: false,
            vcam_wanted: false,
            starts_count: 0,
            scenes_dirty: false,
            cfg_dirty: false,
            textures: HashMap::new(),
            waker,
            edit_mode: false,
            selected_layer: None,
            history: undo::History::default(),
            drag: None,
            settings: None,
            confirm: None,
            quit_ok: false,
            log_open: false,
            health_open: false,
            peak_hold: HashMap::new(),
            toast: None,
            requests_tx,
            requests_rx,
            remote_shared,
            remote: None,
            midi: None,
            osc: None,
            last_publish: Instant::now(),
            precheck: None,
            title_edit: None,
            self_test: None,
        };
        app.vcam_wanted = app.cfg.display.virtual_camera;
        app.start_engine();
        app.apply_control_settings();
        // How the last update went (and the old version let go of, once
        // this one has run a while).
        if let Ok(me) = crate::net::self_update::Install::here() {
            use crate::net::self_update::Outcome;
            match crate::net::self_update::after_start(&me, env!("CARGO_PKG_VERSION")) {
                Some(Outcome::Updated { from }) => {
                    let mut text = format!("Updated to {} (from {from})", env!("CARGO_PKG_VERSION"));
                    if crate::platform::vcam::needs_reinstall() {
                        text.push_str(". The virtual camera changed too: reinstall it in Settings → Screens & webcam");
                    }
                    app.toast(text, false);
                    app.settle_at = Some(Instant::now() + crate::net::self_update::SETTLE);
                }
                Some(Outcome::RolledBack { to }) => {
                    app.toast(format!("{to} didn't start properly, so Cutaway went back to {}. Its download page has it if you'd like to try again.", env!("CARGO_PKG_VERSION")), true);
                    app.cfg.ui.skip_version = to;
                    app.cfg_dirty = true;
                }
                Some(Outcome::NotInstalled { to, why }) => {
                    app.toast(format!("{to} wasn't put in: {why}"), true);
                }
                None => {}
            }
        }
        // For scripted screenshots: a pretend newer version, its window open.
        if let Ok(v) = std::env::var("CUTAWAY_UI_TEST_UPDATE") {
            use crate::net::self_update::{self as su, Finish, Stage, Staged};
            *app.update.lock_ok() = Some(crate::net::update::Release { version: v.clone(), url: "https://github.com".into(), notes: "• Something new\n• A fix".into(), assets: Vec::new() });
            app.update_open = true;
            match std::env::var("CUTAWAY_UI_TEST_UPDATE_STAGE").as_deref() {
                Ok("downloading") => su::show_stage(Stage::Downloading { got: 12_400_000, total: Some(36_700_000) }, Finish::Ask),
                Ok("ready") => su::show_stage(Stage::Ready(Staged { version: v, new_item: PathBuf::new() }), Finish::Ask),
                Ok("quit") => su::show_stage(Stage::Ready(Staged { version: v, new_item: PathBuf::new() }), Finish::OnQuit),
                Ok("failed") => su::show_stage(Stage::Failed("the download was cut off".into()), Finish::Ask),
                _ => {}
            }
        }
        if app.cfg.ui.check_updates {
            let (slot, waker) = (app.update.clone(), app.waker.clone());
            let (auto, skip) = (app.cfg.ui.auto_update, app.cfg.ui.skip_version.clone());
            let _ = std::thread::Builder::new().name("update check".into()).spawn(move || {
                // Not in the way of starting up.
                std::thread::sleep(Duration::from_secs(5));
                match crate::net::update::check(env!("CARGO_PKG_VERSION")) {
                    Ok(Some(r)) => {
                        // Fetched by itself, to go in when Cutaway closes.
                        if auto && r.version != skip && crate::net::self_update::Install::this().is_ok() {
                            crate::net::self_update::start(r.clone(), crate::net::self_update::Finish::OnQuit, waker.clone());
                        }
                        *slot.lock_ok() = Some(r);
                        waker();
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("update check: {e:#}"),
                }
            });
        }
        app
    }

    /// The update window: what's new, then getting it, then putting it in.
    /// About Cutaway: the logo, this version, and where to read more.
    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("About Cutaway").open(&mut open).collapsible(false).resizable(false).default_width(380.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(&self.logo).max_height(64.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Cutaway").size(22.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                    ui.label(RichText::new("Record, switch and stream your show.").color(TEXT_DIM));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let repo = format!("https://github.com/{}", crate::net::update::REPO);
                if ui.button("What's new").on_hover_text("Every version's release notes").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{repo}/releases")));
                }
                if ui.button("Help and source").on_hover_text("Cutaway's guide and code on GitHub").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&repo));
                }
            });
        });
        self.about_open &= open;
    }

    fn update_window(&mut self, ui: &mut egui::Ui, r: &crate::net::update::Release, live: bool, skip: &mut bool, restart: &mut bool) {
        use crate::net::self_update::{self as su, Finish, Stage};
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).max_height(32.0));
            ui.label(format!("Cutaway {} is out. You have {}.", r.version, env!("CARGO_PKG_VERSION")));
        });
        if !r.notes.is_empty() {
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.label(RichText::new(&r.notes).size(12.5).color(TEXT_DIM));
            });
        }
        ui.add_space(8.0);
        let page = |ui: &mut egui::Ui, text: &str| {
            if ui.button(text).on_hover_text(&r.url).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&r.url));
            }
        };
        let can = su::Install::this();
        match su::stage() {
            Stage::Idle => {
                ui.horizontal(|ui| match &can {
                    Ok(_) => {
                        if ui.button(RichText::new("Update now").strong()).on_hover_text("Download it here; Cutaway restarts to put it in when you say").clicked() {
                            su::start(r.clone(), Finish::Ask, self.waker.clone());
                        }
                        page(ui, "Download page");
                        if ui.button("Skip this version").clicked() {
                            *skip = true;
                        }
                    }
                    Err(_) => {
                        page(ui, "Download");
                        if ui.button("Skip this version").clicked() {
                            *skip = true;
                        }
                    }
                });
                if let Err(why) = &can {
                    ui.label(RichText::new(format!("Get it from the download page: {why}.")).size(11.5).color(TEXT_DIM));
                }
            }
            Stage::Downloading { got, total } => {
                let k = total.filter(|t| *t > 0).map(|t| got as f32 / t as f32).unwrap_or(0.0);
                ui.add(egui::ProgressBar::new(k).desired_width(300.0).text(format!("Downloading… {}", crate::net::ndi_install::progress_text(got, total))));
                ui.horizontal(|ui| {
                    if ui.button("Stop").clicked() {
                        su::cancel();
                    }
                    ui.label(RichText::new("Carry on with the show meanwhile.").size(11.5).color(TEXT_DIM));
                });
            }
            Stage::Preparing => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the download and getting it ready…");
                });
            }
            Stage::Ready(s) => {
                ui.label(RichText::new(format!("✓ {} is downloaded and checked.", s.version)).color(GREEN));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let b = ui.add_enabled(!live, egui::Button::new(RichText::new("Restart to update").strong()));
                    let b = if live { b.on_disabled_hover_text("Stop recording and streaming first (or choose When I quit)") } else { b.on_hover_text("Cutaway closes, updates and opens again") };
                    if b.clicked() {
                        *restart = true;
                    }
                    let on_quit = su::finish() == Finish::OnQuit;
                    if ui.add(egui::Button::selectable(on_quit, "When I quit")).on_hover_text("Update the next time Cutaway is closed").clicked() {
                        su::choose(if on_quit { Finish::Ask } else { Finish::OnQuit });
                    }
                });
                if su::finish() == Finish::OnQuit {
                    ui.label(RichText::new("It goes in when you close Cutaway.").size(11.5).color(TEXT_DIM));
                } else if live {
                    ui.label(RichText::new("Restarting waits until recording and streaming have stopped.").size(11.5).color(TEXT_DIM));
                }
            }
            Stage::Failed(e) => {
                ui.label(RichText::new(format!("⚠ Couldn't update: {e}")).color(AMBER));
                ui.horizontal(|ui| {
                    if ui.button("Try again").clicked() {
                        su::reset();
                        su::start(r.clone(), Finish::Ask, self.waker.clone());
                    }
                    page(ui, "Download page");
                });
            }
        }
        ui.label(RichText::new("Your settings and shows stay as they are when you update.").size(11.5).color(TEXT_DIM));
    }

    /// Start or stop the remote server and MIDI to match the settings.
    fn apply_control_settings(&mut self) {
        self.syrinscape.set_token(&self.cfg.syrinscape.token);
        crate::engine::guests::hub().apply(&self.cfg, self.waker.clone());
        self.apply_chat();
        let r = self.cfg.remote.clone();
        *self.remote_shared.pin.lock_ok() = r.pin.trim().to_string();
        let running_port = self.remote.as_ref().map(|s| s.port);
        if !r.enabled {
            self.remote = None;
        } else if running_port != Some(r.port) {
            self.remote = None;
            match remote::RemoteServer::start(r.port, self.remote_shared.clone()) {
                Ok(s) => self.remote = Some(s),
                Err(e) => self.log(LogLevel::Error, format!("Remote control: {e:#}")),
            }
        }
        self.midi = None;
        if self.cfg.midi.enabled && midi::available() {
            match midi::Midi::start(&self.cfg.midi, self.requests_tx.clone(), self.waker.clone()) {
                Ok(m) => self.midi = Some(m),
                Err(e) => self.log(LogLevel::Warning, format!("MIDI: {e:#}")),
            }
        }
        self.osc = None;
        if self.cfg.osc.enabled {
            match osc::Osc::start(&self.cfg.osc, self.requests_tx.clone(), self.waker.clone()) {
                Ok(o) => self.osc = Some(o),
                Err(e) => self.log(LogLevel::Warning, format!("OSC: {e:#}")),
            }
        }
    }

    fn log(&mut self, level: LogLevel, text: String) {
        match &self.engine {
            Some(e) => e.status.lock_ok().push(level, text),
            None => self.toast(text, level == LogLevel::Error),
        }
    }

    fn save_config(&mut self) {
        if self.scenes_dirty {
            self.cfg.scenes = self.scenes.clone();
            self.scenes_dirty = false;
            if let Some(e) = &self.engine {
                e.set_scenes(&self.cfg.scenes);
            }
        }
        self.cfg_dirty = false;
        if let Some(p) = &self.save_path {
            match self.cfg.save(p) {
                Ok(()) => {
                    // Keep the current show's profile in step.
                    if !self.cfg.ui.profile.trim().is_empty() {
                        let _ = profiles::save(&crate::config::profiles_dir(p), &self.cfg.ui.profile, &self.cfg);
                    }
                    self.toast(format!("Saved settings to {}", p.display()), false)
                }
                Err(e) => self.toast(format!("Couldn't save settings: {e:#}"), true),
            }
        }
    }

    /// Save without a message (small changes made from the desk, like a
    /// PTZ preset), and tell the engine.
    fn save_config_quietly(&mut self) {
        if let Some(e) = &self.engine {
            let _ = e.update_config(&self.cfg);
        }
        if let Some(p) = &self.save_path {
            if self.cfg.save(p).is_ok() && !self.cfg.ui.profile.trim().is_empty() {
                let _ = profiles::save(&crate::config::profiles_dir(p), &self.cfg.ui.profile, &self.cfg);
            }
        }
    }

    fn toast(&mut self, text: String, error: bool) {
        self.toast = Some((text, Instant::now(), error));
    }

    // ---- undo ---------------------------------------------------------------------------

    fn snapshot(&self, kind: undo::Kind, label: impl Into<String>) -> undo::Snapshot {
        let live = self.engine.as_ref().map(|e| e.switcher.live().name).unwrap_or_default();
        undo::Snapshot::new(kind, label, &self.cfg, &self.scenes, &live)
    }

    /// Remember how things are, before a change.
    fn remember(&mut self, kind: undo::Kind, label: impl Into<String>) {
        let s = self.snapshot(kind, label);
        self.history.record(s);
    }

    fn undo(&mut self, ctx: &egui::Context) {
        let now = self.snapshot(undo::Kind::Scenes, "");
        let Some(back) = self.history.undo(now) else {
            self.toast("Nothing to undo".into(), false);
            return;
        };
        match self.restore(&back, ctx) {
            Ok(()) => self.toast(format!("Undone: {}", back.label), false),
            Err(e) => {
                self.toast(format!("Can't undo {} now: {e}", back.label.to_lowercase()), true);
                self.history.put_back_undo(back);
            }
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        let now = self.snapshot(undo::Kind::Scenes, "");
        let Some(fwd) = self.history.redo(now) else {
            self.toast("Nothing to redo".into(), false);
            return;
        };
        match self.restore(&fwd, ctx) {
            Ok(()) => self.toast(format!("Redone: {}", fwd.label), false),
            Err(e) => {
                self.toast(format!("Can't redo {} now: {e}", fwd.label.to_lowercase()), true);
                self.history.put_back_redo(fwd);
            }
        }
    }

    /// Put back settings and scenes as they were. The scene on air keeps
    /// going; only its layout changes (or, if it was renamed, the old one
    /// takes its place).
    fn restore(&mut self, s: &undo::Snapshot, ctx: &egui::Context) -> Result<(), String> {
        match s.kind {
            undo::Kind::Settings if s.settings_differ(&self.cfg) => {
                self.apply_settings(s.cfg.clone(), ctx)?;
                if let Some(win) = self.settings.as_mut() {
                    win.applied(Ok(()), &self.cfg);
                }
            }
            undo::Kind::Titles => {
                for (i, g) in s.cfg.graphics.iter().enumerate() {
                    let Some(now) = self.cfg.graphics.get_mut(i) else { continue };
                    if (now.title != g.title || now.subtitle != g.subtitle) && now.name == g.name {
                        (now.title, now.subtitle) = (g.title.clone(), g.subtitle.clone());
                        if let Some(e) = &self.engine {
                            if now.kind == crate::config::GraphicKind::Ticker {
                                e.graphics.set_ticker_text(i, &g.title);
                            } else {
                                e.graphics.set_text(i, &g.title, &g.subtitle);
                            }
                        }
                    }
                }
                if let Some(e) = &self.engine {
                    let _ = e.update_config(&self.cfg);
                }
                self.cfg_dirty = true;
                return Ok(());
            }
            _ => {}
        }
        let live = self.engine.as_ref().map(|e| e.switcher.live().name);
        self.scenes = s.scenes.clone();
        // (Sources taken away since can't come back with a scene.)
        let n = self.cfg.sources.len();
        for sc in &mut self.scenes {
            sc.retain_sources(n);
        }
        self.scenes_dirty = true;
        let on_air = live.and_then(|n| self.scenes.iter().find(|x| x.name == n)).or_else(|| self.scenes.iter().find(|x| x.name == s.live)).cloned();
        if let (Some(e), Some(sc)) = (&self.engine, on_air) {
            e.switcher.set_live(sc);
        }
        if let Some(e) = &self.engine {
            e.set_scenes(&self.scenes);
        }
        self.selected_layer = None;
        Ok(())
    }

    fn mode(&self) -> TransitionMode {
        self.cfg.transition.mode
    }

    fn preview_mode(&self) -> bool {
        self.cfg.transition.preview_mode
    }

    /// A scene button or number key: to preview (in preview mode) or air.
    fn press_scene(&mut self, idx: usize, force_cut: bool) {
        let Some(scene) = self.scenes.get(idx).cloned() else { return };
        if self.preview_mode() && !force_cut {
            if let Some(e) = &self.engine {
                let mut s = scene;
                s.retain_sources(e.sources.len());
                e.switcher.set_preview(Some(s));
                (self.waker)();
            }
        } else {
            self.take_scene(&scene, force_cut);
        }
    }

    fn take_scene(&mut self, scene: &Scene, force_cut: bool) {
        if let Some(e) = &self.engine {
            let mode = if force_cut { TransitionMode::Cut } else { self.mode() };
            e.take_scene(scene, mode, self.cfg.transition.duration_ms);
            self.selected_layer = None;
        }
        self.run_cues(&scene.cues);
    }

    /// A scene's (or rundown segment's) cues: Syrinscape moods, pads,
    /// graphics… Switching commands are skipped, so one can't set off another.
    fn run_cues(&mut self, cues: &[String]) {
        self.run_sequence("Cue", cues, true);
    }

    /// Preview to program; the old program goes to preview.
    fn take_preview(&mut self, cut: bool) {
        let Some(e) = &self.engine else { return };
        let Some(next) = e.switcher.preview() else { return };
        let old = e.switcher.live();
        self.take_scene(&next, cut);
        if let Some(e) = &self.engine {
            e.switcher.set_preview(Some(old));
        }
    }

    fn toggle_record(&mut self) {
        let Some(e) = &self.engine else { return };
        if e.is_recording() {
            if self.cfg.ui.confirm_stop_recording && self.confirm.is_none() {
                self.confirm = Some(Confirm::StopRecording);
            } else {
                e.stop_recording();
            }
        } else if self.cfg.ui.check_before_start && self.precheck.is_none() {
            self.start_precheck(Some(Then::Record));
        } else {
            self.start_recording_now();
        }
    }

    fn start_recording_now(&mut self) {
        let Some(e) = &self.engine else { return };
        if let Err(err) = e.start_recording() {
            let msg = format!("Couldn't start recording: {err:#}");
            self.toast(msg, true);
        }
    }

    fn toggle_stream(&mut self) {
        let Some(e) = &self.engine else { return };
        if e.is_streaming() {
            if self.cfg.ui.confirm_end_stream && self.confirm.is_none() {
                self.confirm = Some(Confirm::EndStream);
            } else {
                e.stop_stream();
            }
            return;
        }
        if !self.cfg.streaming.destinations.iter().any(|d| d.enabled) {
            self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::Streaming));
            self.toast("Add a streaming destination first".into(), false);
            return;
        }
        if self.cfg.ui.check_before_start && self.precheck.is_none() {
            self.start_precheck(Some(Then::Live));
        } else {
            self.start_stream_now();
        }
    }

    fn start_stream_now(&mut self) {
        let Some(e) = &self.engine else { return };
        if let Err(err) = e.start_stream() {
            let msg = format!("Couldn't go live: {err:#}");
            self.toast(msg, true);
        }
    }

    fn add_marker(&mut self, name: Option<String>) -> Result<String, String> {
        let Some(e) = &self.engine else { return Err("the engine isn't running".into()) };
        e.add_marker(name).map_err(|e| format!("{e:#}"))
    }

    fn set_auto(&mut self, on: bool) {
        if let Some(e) = &self.engine {
            e.set_auto_switch(on);
        }
    }

    fn auto_on(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.auto_switch_enabled())
    }

    fn toggle_graphic(&mut self, i: usize, to: Option<bool>) {
        if let Some(e) = &self.engine {
            match to {
                Some(on) => e.graphics.set_visible(i, on),
                None => e.graphics.toggle(i),
            }
        }
    }

    fn set_input_on(&mut self, i: usize, on: bool) {
        if let Some(e) = &self.engine {
            e.set_in_program(i, on);
        }
        if let Some(a) = self.cfg.audio.get_mut(i) {
            a.in_program = on;
            self.cfg_dirty = true;
        }
    }

    fn set_virtual_camera(&mut self, on: bool) {
        let r = self.engine.as_ref().map(|e| e.set_virtual_camera(on));
        if let Some(Err(e)) = r {
            self.toast(format!("Virtual camera: {e:#}"), true);
        }
    }

    // ---- remote / MIDI commands ----------------------------------------------------

    fn run_action(&mut self, a: Action) -> Result<String, String> {
        let scene_names: Vec<String> = self.scenes.iter().map(|s| s.name.clone()).collect();
        let find_scene = |t: &control::Target| t.find(&scene_names).ok_or_else(|| "there's no such scene".to_string());
        let toggle = |t: &Toggle, now: bool| match t {
            Toggle::On => true,
            Toggle::Off => false,
            Toggle::Toggle => !now,
        };
        match a {
            Action::Scene(t) => {
                let i = find_scene(&t)?;
                let s = self.scenes[i].clone();
                self.take_scene(&s, false);
                Ok(format!("Took {}", s.name))
            }
            Action::CutTo(t) => {
                let i = find_scene(&t)?;
                let s = self.scenes[i].clone();
                self.take_scene(&s, true);
                Ok(format!("Cut to {}", s.name))
            }
            Action::Preview(t) => {
                let i = find_scene(&t)?;
                let s = self.scenes[i].clone();
                if let Some(e) = &self.engine {
                    e.switcher.set_preview(Some(s.clone()));
                }
                Ok(format!("{} in preview", s.name))
            }
            Action::Take | Action::Cut => {
                if self.engine.as_ref().is_none_or(|e| e.switcher.preview().is_none()) {
                    return Err("nothing in preview".into());
                }
                self.take_preview(a == Action::Cut);
                Ok("Taken".into())
            }
            Action::Transition(m) => {
                self.cfg.transition.mode = m;
                self.cfg_dirty = true;
                Ok(format!("Transition: {}", m.label()))
            }
            Action::Record(t) => {
                let now = self.engine.as_ref().is_some_and(|e| e.is_recording());
                let want = toggle(&t, now);
                if want != now {
                    let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                    if want {
                        e.start_recording().map_err(|e| format!("{e:#}"))?;
                    } else {
                        e.stop_recording();
                    }
                }
                Ok(if want { "Recording".into() } else { "Stopped recording".into() })
            }
            Action::Live(t) => {
                let now = self.engine.as_ref().is_some_and(|e| e.is_streaming());
                let want = toggle(&t, now);
                if want != now {
                    let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                    if want {
                        e.start_stream().map_err(|e| format!("{e:#}"))?;
                    } else {
                        e.stop_stream();
                    }
                }
                Ok(if want { "Live".into() } else { "Stream ended".into() })
            }
            Action::Marker(name) => self.add_marker(name),
            Action::Auto(t) => {
                let on = toggle(&t, self.auto_on());
                self.set_auto(on);
                Ok(format!("Auto switching {}", if on { "on" } else { "off" }))
            }
            Action::Graphic(t, tg) => {
                let names: Vec<String> = self.cfg.graphics.iter().map(|g| g.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such graphic")?;
                let now = self.engine.as_ref().is_some_and(|e| e.graphics.is_visible(i));
                let on = toggle(&tg, now);
                self.toggle_graphic(i, Some(on));
                Ok(format!("{} {}", names[i], if on { "on" } else { "off" }))
            }
            Action::Rotation(t, tg) => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                let (name, on) = e.rotations.set(&self.cfg.rotations, &t, tg, &e.graphics)?;
                Ok(format!("{name} {}", if on { "started" } else { "stopped" }))
            }
            Action::Audio(t, tg) => {
                let names: Vec<String> = self.cfg.audio.iter().map(|a| a.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such audio input")?;
                let now = self.engine.as_ref().is_some_and(|e| e.is_on(i));
                let on = toggle(&tg, now);
                self.set_input_on(i, on);
                Ok(format!("{} {}", names[i], if on { "on" } else { "off" }))
            }
            Action::Gain(t, db) => {
                let names: Vec<String> = self.cfg.audio.iter().map(|a| a.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such audio input")?;
                if let Some(e) = &self.engine {
                    e.set_gain(i, db);
                }
                self.cfg.audio[i].gain_db = db;
                self.cfg_dirty = true;
                Ok(format!("{} {db:+.1} dB", names[i]))
            }
            Action::Captions(t) => {
                let c = self.engine.as_ref().and_then(|e| e.captions()).ok_or("live captions are off (Settings → Captions)")?;
                let on = toggle(&t, c.showing());
                c.set_showing(on);
                Ok(format!("Captions {}", if on { "on" } else { "off" }))
            }
            Action::Source(t) => {
                let names: Vec<String> = self.cfg.sources.iter().map(|s| s.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such source")?;
                self.show_source(i, false);
                Ok(format!("{} full screen", names[i]))
            }
            Action::Pad(t, cmd) => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                let names = e.pads.names();
                let i = t.find(&names).ok_or("there's no such pad")?;
                match cmd {
                    control::PadCmd::Press => e.pads.press(i).map(|on| format!("{} {}", names[i], if on { "playing" } else { "stopped" })),
                    control::PadCmd::Play => e.pads.play(i).map(|_| format!("{} playing", names[i])),
                    control::PadCmd::Stop => {
                        e.pads.stop(i);
                        Ok(format!("{} stopped", names[i]))
                    }
                    control::PadCmd::Release => {
                        e.pads.release(i);
                        Ok("OK".into())
                    }
                }
            }
            Action::PadsStop => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                e.pads.stop_all();
                Ok("Pads stopped".into())
            }
            Action::Talkback(t) => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                if e.mix.talkback_input().is_none() {
                    return Err("there's no talkback mic (Settings → Mixing & headphones)".into());
                }
                let on = toggle(&t, e.talkback());
                e.set_talkback(on);
                Ok(format!("Talkback {}", if on { "on" } else { "off" }))
            }
            Action::Syrinscape(cmd) => {
                use control::syrinscape::SyrinCmd;
                if !self.cfg.syrinscape.enabled {
                    return Err("Syrinscape control is off (Settings → Syrinscape)".into());
                }
                let buttons = self.cfg.syrinscape.buttons.clone();
                let names: Vec<String> = buttons.iter().map(|b| b.name.clone()).collect();
                let s = &self.syrinscape;
                match cmd {
                    SyrinCmd::Button(t) => s.press(&buttons[t.find(&names).ok_or("there's no such Syrinscape button")?]),
                    SyrinCmd::StopButton(t) => {
                        let b = &buttons[t.find(&names).ok_or("there's no such Syrinscape button")?];
                        s.stop(b.kind, b.id).map(|_| format!("Syrinscape: stopped {}", b.name))
                    }
                    SyrinCmd::Play(k, id) => s.play(k, id).map(|_| "Syrinscape: playing".into()),
                    SyrinCmd::Stop(k, id) => s.stop(k, id).map(|_| "Syrinscape: stopped".into()),
                    SyrinCmd::StopAll => s.stop_all().map(|_| "Syrinscape: stopped everything".into()),
                    SyrinCmd::Volume(v) => s.set_volume(v).map(|_| format!("Syrinscape volume {:.0}%", v * 100.0)),
                }
            }
            Action::Clip(secs) => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                e.save_clip(secs).map(|n| format!("Saving {n}")).map_err(|e| format!("{e:#}"))
            }
            Action::Timer(t, cmd) => {
                use crate::config::GraphicKind;
                let names: Vec<String> = self.cfg.graphics.iter().map(|g| g.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such clock")?;
                let g = &self.cfg.graphics[i];
                if g.kind != GraphicKind::Clock || !g.clock.is_timer() {
                    return Err(format!("{} isn't a countdown or stopwatch", g.name));
                }
                let name = g.name.clone();
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                let timers = &e.graphics.timers;
                let what = match cmd {
                    control::TimerCmd::Start => {
                        timers.start(&name);
                        "running"
                    }
                    control::TimerCmd::Pause => {
                        timers.pause(&name);
                        "paused"
                    }
                    control::TimerCmd::Toggle => {
                        if timers.toggle(&name) {
                            "running"
                        } else {
                            "paused"
                        }
                    }
                    control::TimerCmd::Reset => {
                        timers.reset(&name);
                        "reset"
                    }
                    control::TimerCmd::Stop => {
                        timers.stop(&name);
                        "stopped"
                    }
                };
                Ok(format!("{name} {what}"))
            }
            Action::Rundown(cmd) => {
                let segs = &self.cfg.rundown.segments;
                if segs.is_empty() {
                    return Err("the rundown is empty (Settings → Rundown)".into());
                }
                let to = self.rundown.target(&cmd, segs).ok_or_else(|| match cmd {
                    control::rundown::RundownCmd::Next => "that was the last segment".to_string(),
                    control::rundown::RundownCmd::Previous => "that's the first segment".to_string(),
                    control::rundown::RundownCmd::Stop => "the rundown isn't running".to_string(),
                    _ => "there's no such segment".to_string(),
                })?;
                self.go_segment(to);
                Ok(match to {
                    Some(i) => format!("Segment {}: {}", i + 1, self.cfg.rundown.segments.get(i).map(|s| s.name.as_str()).unwrap_or("")),
                    None => "Rundown stopped".into(),
                })
            }
            Action::Media(t, cmd) => {
                let names: Vec<String> = self.cfg.sources.iter().map(|s| s.name.clone()).collect();
                let i = t.find(&names).ok_or("there's no such source")?;
                let p = self.engine.as_ref().and_then(|e| e.player(i)).ok_or("that source isn't a media player")?;
                p.send(match cmd {
                    MediaCmd::Play => PlayerCmd::Play,
                    MediaCmd::Pause => PlayerCmd::Pause,
                    MediaCmd::Toggle => PlayerCmd::TogglePause,
                    MediaCmd::Restart => PlayerCmd::Restart,
                    MediaCmd::Next => PlayerCmd::Next,
                    MediaCmd::Previous => PlayerCmd::Previous,
                });
                Ok("OK".into())
            }
            Action::Ftb(t) => {
                let e = self.engine.as_ref().ok_or("the engine isn't running")?;
                let on = toggle(&t, e.is_black());
                e.fade_to_black(Some(on));
                Ok(if on { "Fading to black" } else { "Fading up" }.into())
            }
            Action::Ptz(cam, cmd) => self.run_ptz(cam, cmd),
            Action::Replay(cmd) => self.run_replay(cmd),
            Action::Macro(m) => self.run_macro(m),
            Action::Wait(s) => Ok(format!("Waited {s} s")),
            Action::Prompter(cmd) => self.run_prompter(cmd),
            Action::Comment(cmd) => self.run_comment(cmd),
        }
    }

    /// Move the rundown to segment `to` (None: stop): its scene, cues,
    /// clock and marker.
    fn go_segment(&mut self, to: Option<usize>) {
        self.rundown.go(to);
        let seg = to.and_then(|i| self.cfg.rundown.segments.get(i).cloned());
        if let Some(e) = &self.engine {
            e.graphics.timers.set_segment(seg.as_ref().map(|s| (s.name.clone(), s.seconds)));
        }
        let Some(seg) = seg else { return };
        if !seg.scene.trim().is_empty() {
            match self.scenes.iter().position(|s| s.name.eq_ignore_ascii_case(seg.scene.trim())) {
                Some(i) => {
                    let s = self.scenes[i].clone();
                    self.take_scene(&s, false);
                }
                None => self.toast(format!("{}: there's no scene called \"{}\"", seg.name, seg.scene), true),
            }
        }
        self.run_cues(&seg.cues);
        if let Some(i) = to {
            self.prompter_to_segment(i);
        }
        if self.cfg.rundown.markers && self.engine.as_ref().is_some_and(|e| e.is_recording()) {
            let _ = self.add_marker(Some(seg.name.clone()));
        }
    }

    fn handle_requests(&mut self) {
        while let Ok(r) = self.requests_rx.try_recv() {
            let result = self.run_action(r.action);
            if let Some(reply) = r.reply {
                let _ = reply.send(result);
            } else if let Err(e) = result {
                self.toast(e, true);
            }
        }
    }

    /// The show as controllers see it.
    fn show_state(&self) -> Option<ShowState> {
        let e = self.engine.as_ref()?;
        let live = e.switcher.live();
        let preview = e.switcher.preview();
        let live_sources = live.sources();
        let preview_sources = preview.as_ref().map(|p| p.sources()).unwrap_or_default();
        let st = e.status.lock_ok();
        let rec_seconds = match &st.rec {
            RecState::Recording { since, .. } => since.elapsed().as_secs_f64(),
            _ => 0.0,
        };
        let live_seconds = st.stream.since.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        let mut problems = st.health.problems();
        if st.rec_failed.is_some() {
            problems.push("recording problem".into());
        }
        let disk = st.disk.as_ref().map(|d| d.describe()).unwrap_or_default();
        let offline = st.offline.clone();
        let markers = st.markers;
        drop(st);
        let tally = |i: usize| if live_sources.contains(&i) { "program" } else if preview_sources.contains(&i) { "preview" } else { "off" }.to_string();
        Some(ShowState {
            version: env!("CARGO_PKG_VERSION").into(),
            recording: e.is_recording(),
            rec_seconds,
            live: e.is_streaming(),
            live_seconds,
            program: live.name.clone(),
            preview: preview.as_ref().map(|p| p.name.clone()),
            preview_mode: self.cfg.transition.preview_mode,
            transition: self.cfg.transition.mode.name().into(),
            auto: e.auto_switch_enabled(),
            captions: e.captions().is_some_and(|c| c.showing()),
            captions_available: e.captions().is_some(),
            caption_text: e.captions().map(|c| c.text()).unwrap_or_default(),
            markers,
            scenes: self
                .scenes
                .iter()
                .map(|s| show::SceneState { name: s.name.clone(), program: s.name == live.name, preview: preview.as_ref().is_some_and(|p| p.name == s.name) })
                .collect(),
            sources: e.sources.iter().enumerate().map(|(i, s)| show::SourceState { name: s.name.clone(), tally: tally(i), offline: offline.contains(&s.name) }).collect(),
            graphics: e.graphics.list().into_iter().map(|(name, on, _)| show::GraphicState { name, on }).collect(),
            rotations: e.rotations.list().into_iter().map(|r| show::RotationState { name: r.name, running: r.running, showing: r.showing }).collect(),
            inputs: e
                .audio
                .iter()
                .enumerate()
                .map(|(i, a)| show::InputState {
                    name: a.name.clone(),
                    on: e.is_on(i),
                    level: (a.shared.levels_db().into_iter().fold(-120.0f32, f32::max) * 2.0).round() / 2.0,
                    gain_db: self.cfg.audio.get(i).map(|c| c.gain_db).unwrap_or(0.0),
                })
                .collect(),
            players: (0..e.sources.len())
                .filter_map(|i| {
                    let p = e.player(i)?;
                    let s = p.state.lock_ok().clone();
                    Some(show::PlayerState {
                        name: e.sources[i].name.clone(),
                        source: i + 1,
                        playing: s.playing,
                        item: s.items.get(s.index).cloned().unwrap_or_default(),
                        position: s.position as f64 / 1e9,
                        duration: s.duration as f64 / 1e9,
                    })
                })
                .collect(),
            disk,
            problems,
            pads: e.pads.state().into_iter().map(|p| show::PadInfo { name: p.name, color: p.color, playing: p.playing, progress: p.progress, remaining: p.remaining }).collect(),
            syrinscape: if self.cfg.syrinscape.enabled {
                self.cfg
                    .syrinscape
                    .buttons
                    .iter()
                    .map(|b| show::SyrinInfo { name: b.name.clone(), kind: format!("{:?}", b.kind).to_lowercase(), playing: self.syrinscape.is_playing(b) })
                    .collect()
            } else {
                Vec::new()
            },
            talkback: e.talkback(),
            talkback_available: e.mix.talkback_input().is_some(),
            clip_seconds: self.cfg.output.clip_seconds,
            black: e.is_black(),
            prompter: (!self.prompter.script.trim().is_empty()).then(|| show::PrompterState {
                version: self.prompter.version,
                playing: self.prompter.playing,
                // Rounded so a rolling prompter doesn't publish every frame.
                position: (self.prompter.pos * 2.0).round() / 2.0,
                speed: production::chars_per_second(self.prompter.wpm),
                words_per_minute: self.prompter.wpm,
                mirror: self.cfg.prompter.mirror,
                text_size: self.cfg.prompter.text_size,
                text_color: self.cfg.prompter.text_color.clone(),
                background: self.cfg.prompter.background.clone(),
            }),
            macros: self.cfg.macros.iter().map(|m| show::MacroInfo { name: m.name.clone(), color: m.color.clone() }).collect(),
            replaying: e.replay.playing(),
            ptz: self
                .cfg
                .sources
                .iter()
                .enumerate()
                .filter(|(_, s)| s.has_ptz())
                .map(|(i, s)| show::PtzState {
                    name: s.name.clone(),
                    source: i + 1,
                    presets: s.ptz.presets.iter().enumerate().map(|(k, p)| if p.name.trim().is_empty() { format!("Preset {}", k + 1) } else { p.name.clone() }).collect(),
                })
                .collect(),
            comments: self.chat.as_ref().map(|c| show::CommentsState { showing: self.comment_on.is_some(), waiting: c.messages().iter().filter(|m| m.id > self.comment_cursor).count() }),
            guests: if self.cfg.guests.enabled {
                crate::engine::guests::hub()
                    .guests()
                    .into_iter()
                    .map(|g| show::GuestState {
                        source: e.sources.iter().position(|s| s.name == g.source).map(|i| i + 1).unwrap_or(0),
                        name: g.source,
                        guest_name: g.guest_name,
                        connected: g.connected,
                        connecting: g.connecting,
                    })
                    .collect()
            } else {
                Vec::new()
            },
            clocks: e
                .graphics
                .list()
                .into_iter()
                .enumerate()
                .filter_map(|(i, (name, on, _))| {
                    let (text, over) = e.graphics.clock_text(i)?;
                    let timer = self.cfg.graphics.get(i).is_some_and(|g| g.clock.is_timer());
                    let running = timer && e.graphics.timers.running(&name);
                    Some(show::ClockState { name, text, on, timer, running, over })
                })
                .collect(),
            rundown: (!self.cfg.rundown.segments.is_empty()).then(|| {
                let st = self.rundown.status(&self.cfg.rundown.segments);
                let (left, over) = if st.index.is_some() { panels::left_text(st.elapsed, st.planned) } else { (String::new(), false) };
                show::RundownState {
                    number: st.index.map(|i| i + 1).unwrap_or(0),
                    count: st.count,
                    name: st.name.clone(),
                    next: st.next.clone(),
                    elapsed: st.elapsed,
                    planned: st.planned,
                    left,
                    over,
                    drift: st.drift,
                }
            }),
        })
    }

    /// State for the phone remote, Stream Deck, tally lights, OSC and MIDI.
    fn publish_state(&mut self) {
        if self.last_publish.elapsed() < Duration::from_millis(150) {
            return;
        }
        // Nobody to tell: the remote, OSC and MIDI are all off.
        if self.remote.is_none() && self.osc.is_none() && self.midi.is_none() {
            return;
        }
        self.last_publish = Instant::now();
        let Some(state) = self.show_state() else { return };
        if self.remote.is_some() {
            if let Some(e) = &self.engine {
                let bg = crate::config::parse_color(&self.cfg.program.background_color).unwrap_or(0x10131a);
                *self.remote_shared.keys.lock_ok() = control::keys::KeySources {
                    canvas: e.info.canvas,
                    background: [(bg >> 16) as u8, (bg >> 8) as u8, bg as u8],
                    scenes: self.scenes.clone(),
                    previews: e.sources.iter().map(|s| s.preview.clone()).collect(),
                };
            }
            self.remote_shared.publish(&state);
        }
        if let Some(o) = &mut self.osc {
            o.feedback(&state);
        }
        // MIDI pad lights.
        if let Some(m) = &mut self.midi {
            let scene_names: Vec<String> = state.scenes.iter().map(|s| s.name.clone()).collect();
            let gnames: Vec<String> = state.graphics.iter().map(|g| g.name.clone()).collect();
            let anames: Vec<String> = state.inputs.iter().map(|a| a.name.clone()).collect();
            let snames: Vec<String> = state.sources.iter().map(|a| a.name.clone()).collect();
            let lit = |b: bool| if b { 127 } else { 0 };
            let mut lights = Vec::new();
            for map in &self.cfg.midi.mappings {
                let Some(msg) = midi::Msg::parse(&map.message) else { continue };
                let Ok(action) = Action::parse(&map.action) else { continue };
                let value = match &action {
                    Action::Scene(t) | Action::CutTo(t) | Action::Preview(t) => match t.find(&scene_names).map(|i| &state.scenes[i]) {
                        Some(sc) if sc.program => 127,
                        Some(sc) if sc.preview => 32,
                        _ => 0,
                    },
                    Action::Source(t) => match t.find(&snames).map(|i| state.tally(i)) {
                        Some("program") => 127,
                        Some("preview") => 32,
                        _ => 0,
                    },
                    Action::Record(_) => lit(state.recording),
                    Action::Live(_) => lit(state.live),
                    Action::Auto(_) => lit(state.auto),
                    Action::Graphic(t, _) => lit(t.find(&gnames).is_some_and(|i| state.graphics[i].on)),
                    Action::Rotation(t, _) => {
                        let names: Vec<String> = state.rotations.iter().map(|r| r.name.clone()).collect();
                        lit(t.find(&names).is_some_and(|i| state.rotations[i].running))
                    }
                    Action::Audio(t, _) => lit(t.find(&anames).is_some_and(|i| state.inputs[i].on)),
                    Action::Transition(mode) => lit(state.transition == mode.name()),
                    Action::Captions(_) => lit(state.captions),
                    Action::Pad(t, _) => {
                        let names: Vec<String> = state.pads.iter().map(|p| p.name.clone()).collect();
                        lit(t.find(&names).is_some_and(|i| state.pads[i].playing))
                    }
                    Action::Talkback(_) => lit(state.talkback),
                    Action::Syrinscape(control::syrinscape::SyrinCmd::Button(t)) => {
                        let names: Vec<String> = state.syrinscape.iter().map(|b| b.name.clone()).collect();
                        lit(t.find(&names).is_some_and(|i| state.syrinscape[i].playing))
                    }
                    Action::Timer(t, _) => {
                        // Timers are named by graphic (number or name).
                        let name = t.find(&gnames).map(|i| gnames[i].clone());
                        lit(state.clocks.iter().any(|c| Some(&c.name) == name.as_ref() && c.running))
                    }
                    _ => continue,
                };
                lights.push((msg, value));
            }
            m.feedback(&lights);
        }
    }

    // ---- keys ------------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Let go of held shortcuts (talkback, sound pads), even if a window
        // opened while the key was down.
        let ups: Vec<Key> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Key { key, pressed: false, .. } = e { Some(*key) } else { None }).collect());
        for k in ups {
            let (go, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held_keys).into_iter().partition(|(h, _)| *h == k);
            self.held_keys = keep;
            for (_, a) in go {
                self.act(a);
            }
        }
        if ctx.egui_wants_keyboard_input() || self.settings.is_some() || self.title_edit.is_some() || self.confirm.is_some() || self.precheck.is_some() || self.sync.is_some() {
            return;
        }
        if self.keymap.as_ref().is_none_or(|(c, _)| *c != self.cfg.keys) {
            self.keymap = Some((self.cfg.keys.clone(), Keymap::new(&self.cfg.keys)));
        }
        let downs: Vec<(KeyCombo, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    // (Control on a Mac isn't ⌘, so it's left to the system.)
                    egui::Event::Key { key, pressed: true, repeat, modifiers, .. } if !(modifiers.ctrl && !modifiers.command) => Some((KeyCombo::new(*key, *modifiers), *repeat)),
                    _ => None,
                })
                .collect()
        });
        for (combo, repeat) in downs {
            // Shift held with a plain key that has nothing of its own on
            // Shift does what the key does (as it always has: Shift+Space
            // still takes).
            let binding = self.keymap.as_ref().and_then(|(_, m)| {
                m.find(combo).or_else(|| (combo.shift && !combo.command && !combo.alt && !combo.is_numbered()).then(|| m.find(KeyCombo { shift: false, ..combo })).flatten()).cloned()
            });
            match binding {
                // Only undo and redo repeat while the key is held.
                Some(Binding::Builtin(b)) if !repeat || matches!(b, Builtin::Undo | Builtin::Redo) => self.run_builtin(ctx, b, combo.key),
                Some(Binding::Command(c)) if !repeat => {
                    match Action::parse(&c) {
                        Ok(a) => {
                            self.run_key_command(a);
                            if let Some(up) = shortcuts::release_of(&c) {
                                self.held_keys.push((combo.key, up));
                            }
                        }
                        Err(e) => self.toast(e, true),
                    }
                }
                Some(_) => {}
                None if !repeat => self.numbered_key(combo),
                None => {}
            }
        }
    }

    /// A command on one of your keys. Recording and going live take the same
    /// path as their buttons, so the checks and "are you sure?" still come up.
    fn run_key_command(&mut self, a: Action) {
        let flip = |t: &Toggle, now: bool| match t {
            Toggle::On => !now,
            Toggle::Off => now,
            Toggle::Toggle => true,
        };
        match &a {
            Action::Record(t) => {
                if flip(t, self.engine.as_ref().is_some_and(|e| e.is_recording())) {
                    self.toggle_record();
                }
            }
            Action::Live(t) => {
                if flip(t, self.engine.as_ref().is_some_and(|e| e.is_streaming())) {
                    self.toggle_stream();
                }
            }
            _ => self.act(a),
        }
    }

    /// " (M)": the key for a built-in shortcut, for a button's hover text
    /// ("" when it has none).
    fn key_hint(&self, b: Builtin) -> String {
        let label = match &self.keymap {
            Some((_, m)) => m.label_of(b),
            None => Keymap::new(&self.cfg.keys).label_of(b),
        };
        if label.is_empty() { label } else { format!(" ({label})") }
    }

    /// 1–9 take a scene (Shift cuts straight to it), Alt/Option + 1–9 show a
    /// graphic, F1–F12 play sound pads.
    fn numbered_key(&mut self, combo: KeyCombo) {
        if combo.command {
            return;
        }
        if let Some(i) = shortcuts::DIGITS.iter().position(|k| *k == combo.key) {
            match (combo.shift, combo.alt) {
                (_, false) => self.press_scene(i, combo.shift),
                (false, true) => self.toggle_graphic(i, None),
                (true, true) => {}
            }
        } else if let Some(i) = shortcuts::PAD_KEYS.iter().position(|k| *k == combo.key) {
            if !combo.alt && !combo.shift && i < self.cfg.pads.len() {
                self.act(Action::Pad(control::Target::Index(i), control::PadCmd::Press));
                self.held_keys.push((combo.key, Action::Pad(control::Target::Index(i), control::PadCmd::Release)));
            }
        }
    }

    fn run_builtin(&mut self, ctx: &egui::Context, b: Builtin, key: Key) {
        match b {
            Builtin::Settings => self.open_settings(),
            Builtin::Undo => self.undo(ctx),
            Builtin::Redo => self.redo(ctx),
            Builtin::Record => self.toggle_record(),
            Builtin::Transition => {
                // Cut <-> the last transition used.
                self.cfg.transition.mode = match self.mode() {
                    TransitionMode::Cut => self.last_transition,
                    _ => TransitionMode::Cut,
                };
                self.cfg_dirty = true;
            }
            Builtin::Edit => self.edit_mode = !self.edit_mode,
            Builtin::Marker => match self.add_marker(None) {
                Ok(n) => self.toast(format!("{n} added"), false),
                Err(err) => self.toast(err, true),
            },
            Builtin::Auto => {
                let on = !self.auto_on();
                self.set_auto(on);
            }
            Builtin::Take | Builtin::CutPreview => {
                if self.preview_mode() {
                    self.take_preview(b == Builtin::CutPreview);
                }
            }
            Builtin::Captions => {
                if let Some(c) = self.engine.as_ref().and_then(|e| e.captions()) {
                    c.set_showing(!c.showing());
                }
            }
            Builtin::NextSegment | Builtin::PreviousSegment => {
                if !self.cfg.rundown.segments.is_empty() {
                    self.act(Action::Rundown(if b == Builtin::PreviousSegment { crate::control::rundown::RundownCmd::Previous } else { crate::control::rundown::RundownCmd::Next }));
                }
            }
            Builtin::Clip => match self.run_action(Action::Clip(None)) {
                Ok(m) => self.toast(m, false),
                Err(e) => self.toast(e, true),
            },
            Builtin::Talkback => {
                if self.engine.as_ref().is_some_and(|e| e.mix.talkback_input().is_some()) {
                    self.act(Action::Talkback(Toggle::On));
                    self.held_keys.push((key, Action::Talkback(Toggle::Off)));
                }
            }
        }
    }

    fn run_self_test(&mut self, ctx: &egui::Context) {
        let Some((start, step)) = self.self_test else { return };
        if self.engine.is_none() && step == 0 {
            // The script runs from when the engine is up.
            self.self_test = Some((Instant::now(), 0));
            return;
        }
        let t = start.elapsed().as_secs_f32();
        ctx.request_repaint_after(Duration::from_millis(100));
        let pip = self.scenes.iter().position(|s| s.name == "Picture in picture").unwrap_or(0);
        let next = match step {
            0 if t > 2.0 => {
                self.press_scene(pip, false);
                if std::env::var_os("CUTAWAY_UI_TEST_PREVIEW").is_some() {
                    // Picture in picture on air, the third scene waiting in preview.
                    self.take_preview(true);
                    self.press_scene(2, false);
                }
                1
            }
            1 if t > 3.0 => {
                self.toggle_record();
                if std::env::var_os("CUTAWAY_UI_TEST_LIVE").is_some() {
                    self.toggle_stream();
                }
                if std::env::var_os("CUTAWAY_UI_TEST_GRAPHICS").is_some() {
                    for i in 0..self.cfg.graphics.len() {
                        self.toggle_graphic(i, Some(true));
                    }
                }
                2
            }
            2 if t > 4.0 => {
                if std::env::var_os("CUTAWAY_UI_TEST_EDIT").is_some() {
                    self.edit_mode = true;
                    self.selected_layer = Some(1);
                }
                if std::env::var_os("CUTAWAY_UI_TEST_CHECK").is_some() {
                    self.start_precheck(None);
                }
                if std::env::var_os("CUTAWAY_UI_TEST_HEALTH").is_some() {
                    self.health_open = true;
                }
                if std::env::var_os("CUTAWAY_UI_TEST_LOG").is_some() {
                    self.log_open = true;
                }
                if std::env::var_os("CUTAWAY_UI_TEST_SYNC").is_some() {
                    let job = self.engine.as_ref().map(|e| e.measure_sync(3.0));
                    self.sync = Some(SyncWizard { job, apply: Vec::new() });
                }
                let _ = self.add_marker(None);
                // Commands to set the scene for a screenshot, ";"-separated.
                if let Ok(cmds) = std::env::var("CUTAWAY_UI_TEST_COMMANDS") {
                    for c in cmds.split(';').filter(|c| !c.trim().is_empty()) {
                        if let Ok(a) = Action::parse(c) {
                            if let Err(e) = self.run_action(a) {
                                eprintln!("ui test: {c}: {e}");
                            }
                        }
                    }
                }
                3
            }
            3 if t > 6.0 && std::env::var_os("CUTAWAY_UI_TEST_SETTINGS").is_some() => {
                let name = std::env::var("CUTAWAY_UI_TEST_SETTINGS").unwrap_or_default();
                let section = Section::from_name(&name).unwrap_or(Section::Sources);
                let source = std::env::var("CUTAWAY_UI_TEST_SOURCE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(section).select_source(source));
                4
            }
            s => s,
        };
        self.self_test = Some((start, next));
    }

    fn poll_status(&mut self) {
        if self.cfg.transition.mode != TransitionMode::Cut {
            self.last_transition = self.cfg.transition.mode;
        }
        let Some(e) = &self.engine else { return };
        let failed = e.status.lock_ok().rec_failed.take();
        if let Some(f) = failed {
            if e.is_recording() {
                e.stop_recording();
            }
            self.toast(format!("Recording stopped: {f}"), true);
        }
        let Some(e) = &self.engine else { return };
        // Scene audio and remotes can turn inputs on/off: keep the settings in step.
        for (i, a) in e.audio.iter().enumerate() {
            let on = a.shared.is_on();
            if let Some(c) = self.cfg.audio.get_mut(i) {
                if c.in_program != on {
                    c.in_program = on;
                    self.cfg_dirty = true;
                }
            }
        }
        // A sharper monitor picture while a second screen shows it.
        let want = if self.cfg.display.output != DisplayOutput::Off { e.info.canvas.0.min(1920) } else { self.cfg.program.preview_width };
        e.preview_width.store(want, std::sync::atomic::Ordering::Relaxed);
    }

    fn texture(&mut self, ctx: &egui::Context, key: &str, slot: &FrameSlot) -> Option<egui::TextureId> {
        let (generation, frame) = slot.latest()?;
        let entry = self.textures.get_mut(key);
        match entry {
            Some((g, tex)) if *g == generation => Some(tex.id()),
            Some((g, tex)) => {
                tex.set(egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba), egui::TextureOptions::LINEAR);
                *g = generation;
                Some(tex.id())
            }
            None => {
                let tex = ctx.load_texture(key, egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba), egui::TextureOptions::LINEAR);
                let id = tex.id();
                self.textures.insert(key.to_string(), (generation, tex));
                Some(id)
            }
        }
    }

    // ---- header ----------------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.header_right(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| self.header_left(ui));
            });
        });
    }

    fn header_right(&mut self, ui: &mut egui::Ui) {
        if ui.button("Settings").on_hover_text("Ctrl/Cmd+,").clicked() {
            self.open_settings();
        }
        let update = self.update.lock_ok().clone().filter(|r| r.version != self.cfg.ui.skip_version);
        if let Some(r) = &update {
            use crate::net::self_update::{self as su, Finish, Stage};
            let text = match (su::stage(), su::finish()) {
                (Stage::Downloading { .. } | Stage::Preparing, _) => format!("⬆ Getting {}…", r.version),
                (Stage::Ready(_), Finish::OnQuit) => format!("⬆ {} installs when you quit", r.version),
                (Stage::Ready(_), _) => format!("⬆ {} is ready", r.version),
                _ => format!("⬆ {} is out", r.version),
            };
            if ui.add(egui::Button::selectable(self.update_open, RichText::new(text).color(ACCENT))).on_hover_text("A newer Cutaway").clicked() {
                self.update_open = !self.update_open;
            }
        }
        if let (true, Some(r)) = (self.update_open, update) {
            let mut open = true;
            let mut skip = false;
            let live = self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming());
            let mut restart = false;
            egui::Window::new(format!("Cutaway {}", r.version)).open(&mut open).collapsible(false).resizable(false).default_width(440.0).show(ui.ctx(), |ui| {
                self.update_window(ui, &r, live, &mut skip, &mut restart);
            });
            if skip {
                self.cfg.ui.skip_version = r.version.clone();
                self.cfg_dirty = true;
                open = false;
            }
            if restart {
                crate::net::self_update::choose(crate::net::self_update::Finish::Restart);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            self.update_open = open;
        }
        let (warn_count, problems) = self
            .engine
            .as_ref()
            .map(|e| {
                let st = e.status.lock_ok();
                (st.log.iter().filter(|l| l.level != LogLevel::Info).count(), st.health.problems().len())
            })
            .unwrap_or((0, 0));
        let log_label = if warn_count > 0 { format!("Log ({warn_count})") } else { "Log".into() };
        ui.toggle_value(&mut self.log_open, log_label);
        let health_text = if problems > 0 { RichText::new(format!("⚠ Health ({problems})")).color(AMBER) } else { RichText::new("Health") };
        ui.toggle_value(&mut self.health_open, health_text).on_hover_text("Sources, files, inputs and disk at a glance");
        if ui.button("Check").on_hover_text("Pre-show check: sources, sound, disk, encoder and stream servers").clicked() {
            self.start_precheck(None);
        }
        let screen_on = self.cfg.display.output != DisplayOutput::Off;
        if ui.add(egui::Button::selectable(screen_on, "Screen")).on_hover_text("Program, multiview or prompter on another screen (Settings → Screens & webcam). Esc closes it.").clicked() {
            self.cfg.display.output = if screen_on { DisplayOutput::Off } else { DisplayOutput::Program };
            self.cfg_dirty = true;
        }
        if crate::platform::vcam::available() {
            let on = self.engine.as_ref().is_some_and(|e| e.virtual_camera_on());
            if ui.add(egui::Button::selectable(on, "Webcam")).on_hover_text("Send the program to the \"Cutaway\" camera for Zoom, Teams, Meet…").clicked() {
                if !on && !crate::platform::vcam::installed() {
                    self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::Outputs));
                } else {
                    self.set_virtual_camera(!on);
                }
            }
        }
    }

    fn header_left(&mut self, ui: &mut egui::Ui) {
        let pill = |text: RichText, fill: Color32| egui::Button::new(text).fill(fill).min_size(vec2(150.0, 34.0)).corner_radius(17.0);
        let name = egui::Button::image_and_text(
            egui::Image::new(&self.logo).max_height(20.0),
            RichText::new("CUTAWAY").size(17.0).strong().color(Color32::WHITE).extra_letter_spacing(3.0),
        )
        .frame(false);
        if ui.add(name).on_hover_text("About Cutaway").clicked() {
            self.about_open = !self.about_open;
        }
        if self.about_open {
            self.about_window(ui.ctx());
        }
        self.show_menu(ui);
        ui.add_space(10.0);
        let Some(engine) = &self.engine else { return };
        let (rec, last, stream, markers, disk, background) = {
            let st = engine.status.lock_ok();
            (st.rec.clone(), st.last_take.clone(), st.stream.clone(), st.markers, st.disk.clone(), st.background.clone())
        };
        let streaming = engine.is_streaming();
        let mut status_line: Option<(String, Color32)> = None;
        let mut marker_clicked = false;
        let mut clip_clicked = false;
        // Some(Some((slow, angle))): replay; Some(None): stop it.
        let mut replay_clicked: Option<Option<(bool, Option<control::ReplayAngle>)>> = None;

        // ---- record
        match &rec {
            RecState::Recording { since, folder } => {
                let t = since.elapsed();
                let blink = (t.as_millis() / 600) % 2 == 0;
                let btn = pill(RichText::new(format!("■  REC  {}", fmt_clock(t))).size(15.0).strong().color(Color32::WHITE), if blink { RED } else { Color32::from_rgb(200, 40, 52) });
                if ui.add(btn).on_hover_text(format!("Stop recording (Ctrl/Cmd+R)\nRecording to {}", folder.display())).clicked() {
                    self.toggle_record();
                }
                let mtext = if markers > 0 { format!("◆ Marker ({markers})") } else { "◆ Marker".into() };
                if ui.add(egui::Button::new(mtext).min_size(vec2(0.0, 30.0))).on_hover_text(format!("Mark this moment{}: becomes a chapter and a timeline marker", self.key_hint(Builtin::Marker))).clicked() {
                    marker_clicked = true;
                }
                if ui
                    .add(egui::Button::new("✂ Clip").min_size(vec2(0.0, 30.0)))
                    .on_hover_text(format!("Save the last {} seconds as a clip in the take's Clips folder{}", self.cfg.output.clip_seconds, self.key_hint(Builtin::Clip)))
                    .clicked()
                {
                    clip_clicked = true;
                }
                // Instant replay: click to replay (or stop it); right-click for slow motion.
                let replaying = self.engine.as_ref().is_some_and(|e| e.replay.playing());
                let rb = egui::Button::new(RichText::new(if replaying { "⟲ Replaying" } else { "⟲ Replay" }).color(if replaying { Color32::WHITE } else { ui.visuals().text_color() }))
                    .fill(if replaying { RED_DIM } else { ui.visuals().widgets.inactive.weak_bg_fill })
                    .min_size(vec2(0.0, 30.0));
                let r = ui.add(rb).on_hover_text(format!("Play the last {:.0} seconds again, on air. Right-click for slow motion or another camera.", self.cfg.replay.seconds));
                if r.clicked() {
                    replay_clicked = Some(if replaying { None } else { Some((false, None)) });
                }
                let angles: Vec<String> = self.engine.as_ref().map(|e| e.sources.iter().filter(|s| s.record).map(|s| s.name.clone()).collect()).unwrap_or_default();
                let mut slow = self.replay_slow;
                r.context_menu(|ui| {
                    ui.checkbox(&mut slow, "Slow motion");
                    ui.separator();
                    let mut pick = |ui: &mut egui::Ui, text: &str, angle: control::ReplayAngle| {
                        if ui.button(text).clicked() {
                            replay_clicked = Some(Some((slow, Some(angle))));
                            ui.close();
                        }
                    };
                    pick(ui, "Replay the program", control::ReplayAngle::Program);
                    if !angles.is_empty() {
                        ui.label(RichText::new("From one camera").size(11.0).color(TEXT_DIM));
                        for a in &angles {
                            pick(ui, &format!("   {a}"), control::ReplayAngle::Source(control::Target::Name(a.clone())));
                        }
                    }
                    if angles.len() >= 2 {
                        pick(ui, &format!("Every angle at once ({})", angles.len().min(4)), control::ReplayAngle::Quad);
                    }
                    if replaying {
                        ui.separator();
                        if ui.button("Stop the replay").clicked() {
                            replay_clicked = Some(None);
                            ui.close();
                        }
                    }
                });
                self.replay_slow = slow;
                if let Some(d) = &disk {
                    let left = d.seconds_left().map(crate::engine::health::fmt_left).unwrap_or_default();
                    status_line = Some((format!("{} free · {left} left", crate::util::human_size(d.free)), if d.seconds_left().is_some_and(|s| s < 1800.0) { AMBER } else { TEXT_DIM }));
                }
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            RecState::Finalizing { folder, step, done } => {
                let t = ui.input(|i| i.time) as f32;
                let r = ui.add_enabled(false, pill(RichText::new(format!("Saving files  {:.0}%", done * 100.0)).size(15.0).color(Color32::WHITE), RAISED));
                // A bar along the bottom of the button fills as it goes.
                let bar = Rect::from_min_size(r.rect.left_bottom() - vec2(0.0, 3.0), vec2(r.rect.width(), 3.0));
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * done.clamp(0.02, 1.0), bar.height())), 1.5, ACCENT);
                status_line = Some((format!("{step}{}", waiting::ellipsis(t)), ACCENT));
                r.on_disabled_hover_text(format!("Finishing the take in {}", folder.display()));
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            RecState::Idle => {
                let btn = pill(RichText::new("●  RECORD").size(15.0).strong().color(RED), RAISED).stroke(Stroke::new(1.5, RED_DIM));
                if ui.add(btn).on_hover_text("Record every source, the program and the audio (Ctrl/Cmd+R)").clicked() {
                    self.toggle_record();
                }
                if let Some(t) = &last {
                    let total: u64 = t.files.iter().map(|f| f.1).sum();
                    status_line = Some((format!("Last take: {} files, {} — {}", t.files.len(), crate::util::human_size(total), t.folder.display()), TEXT_DIM));
                } else if let Some(d) = &disk {
                    status_line = Some((d.describe(), TEXT_DIM));
                }
                if let Some((what, f)) = &background {
                    status_line = Some((format!("{what}… {:.0}%", f * 100.0), ACCENT));
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
            }
        }

        // ---- stream
        if stream.stopping {
            ui.add_enabled(false, pill(RichText::new("Ending stream…").size(15.0).color(Color32::WHITE), RAISED));
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else if streaming {
            let t = stream.since.map(|s| s.elapsed()).unwrap_or_default();
            let live = stream.destinations.iter().filter(|d| d.state == DestState::Live).count();
            let total = stream.destinations.iter().filter(|d| d.state != DestState::Off).count();
            let healthy = live == total && total > 0;
            // Nothing's live yet, but it's trying: say so, not "LIVE".
            let connecting: Vec<&str> = stream.destinations.iter().filter(|d| matches!(d.state, DestState::Connecting | DestState::Retrying { .. })).map(|d| d.name.as_str()).collect();
            let going = live == 0 && !connecting.is_empty();
            if going {
                let pulse = ((ui.input(|i| i.time) as f32 * 3.0).sin() + 1.0) * 0.5;
                let fill = Color32::from_rgb(60, 24, 44).lerp_to_gamma(Color32::from_rgb(150, 30, 90), pulse);
                let btn = pill(RichText::new(format!("◉  GOING LIVE{}", waiting::ellipsis(ui.input(|i| i.time) as f32))).size(15.0).strong().color(Color32::WHITE), fill).min_size(vec2(150.0, 0.0));
                if ui.add(btn).on_hover_text("Connecting. Click to stop.").clicked() {
                    self.toggle_stream();
                }
                status_line = Some((format!("Connecting to {}", connecting.join(", ")), Color32::from_rgb(230, 120, 170)));
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            } else {
                let fill = if healthy { LIVE } else { Color32::from_rgb(170, 90, 20) };
                let btn = pill(RichText::new(format!("◉  LIVE  {}", fmt_clock(t))).size(15.0).strong().color(Color32::WHITE), fill);
                if ui.add(btn).on_hover_text("End the stream").clicked() {
                    self.toggle_stream();
                }
            }
            if !healthy && !going {
                status_line = Some((format!("{live} of {total} destinations live — see Stream panel"), AMBER));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            let has_dest = self.cfg.streaming.destinations.iter().any(|d| d.enabled);
            let btn = pill(RichText::new("◉  GO LIVE").size(15.0).strong().color(if has_dest { LIVE } else { TEXT_DIM }), RAISED)
                .stroke(Stroke::new(1.5, if has_dest { Color32::from_rgb(110, 30, 70) } else { LINE }));
            let names: Vec<String> = self.cfg.streaming.destinations.iter().filter(|d| d.enabled).map(|d| d.name.clone()).collect();
            let tip = if names.is_empty() { "Set up streaming destinations".to_string() } else { format!("Stream to {}", names.join(", ")) };
            if ui.add(btn).on_hover_text(tip).clicked() {
                self.toggle_stream();
            }
        }
        let (text, color) = status_line.unwrap_or_else(|| (self.cfg.output.folder.display().to_string(), TEXT_DIM));
        ui.add(egui::Label::new(RichText::new(text).color(color).size(12.0)).truncate());
        if marker_clicked {
            match self.add_marker(None) {
                Ok(n) => self.toast(format!("{n} added"), false),
                Err(e) => self.toast(e, true),
            }
        }
        if clip_clicked {
            match self.run_action(Action::Clip(None)) {
                Ok(m) => self.toast(m, false),
                Err(e) => self.toast(e, true),
            }
        }
        if let Some(r) = replay_clicked {
            let cmd = match r {
                Some((slow, angle)) => control::ReplayCmd::Play { seconds: None, slow, angle },
                None => control::ReplayCmd::Stop,
            };
            if let Err(e) = self.run_action(Action::Replay(cmd)) {
                self.toast(e, true);
            }
        }
        if let Some(p) = self.engine.as_ref().and_then(|e| e.replay.problem()) {
            if self.replay_problem.as_deref() != Some(p.as_str()) {
                self.replay_problem = Some(p.clone());
                self.toast(p, true);
            }
        }
    }

    /// The show (profile) picker.
    fn show_menu(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.save_path.clone() else { return };
        let dir = crate::config::profiles_dir(&path);
        let current = self.cfg.ui.profile.clone();
        let text = if current.is_empty() { "No show ▾".to_string() } else { format!("{current} ▾") };
        ui.menu_button(RichText::new(text).color(TEXT_DIM), |ui| {
            let list = profiles::list(&dir);
            if list.is_empty() {
                ui.label(RichText::new("No saved shows yet").color(TEXT_DIM));
            }
            for name in list {
                if ui.selectable_label(name == current, &name).clicked() && name != current {
                    self.confirm = Some(Confirm::SwitchShow(name));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Save as a show…").clicked() {
                self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::General));
                ui.close();
            }
        });
    }

    fn switch_show(&mut self, name: &str, ctx: &egui::Context) {
        let Some(path) = &self.save_path else { return };
        let dir = crate::config::profiles_dir(path);
        if self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) {
            self.toast("Stop recording and streaming before switching shows".into(), true);
            return;
        }
        // Keep the current show's latest changes first.
        if !self.cfg.ui.profile.trim().is_empty() {
            self.cfg.scenes = self.scenes.clone();
            let _ = profiles::save(&dir, &self.cfg.ui.profile, &self.cfg);
        }
        match profiles::load(&dir, name) {
            Ok(cfg) => {
                let before = self.snapshot(undo::Kind::Settings, format!("Switch to \"{name}\""));
                self.scenes = cfg.effective_scenes();
                match self.apply_settings(cfg, ctx) {
                    Ok(()) => {
                        self.history.record(before);
                        self.toast(format!("Switched to \"{name}\""), false)
                    }
                    Err(e) => self.toast(e, true),
                }
            }
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    // ---- monitors --------------------------------------------------------------

    fn monitors(&mut self, ui: &mut egui::Ui) {
        if self.is_starting() {
            self.starting_screen(ui);
            return;
        }
        if let Some(err) = self.engine_error.clone() {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.label(RichText::new("The engine couldn't start").size(22.0).color(Color32::WHITE));
                ui.add_space(8.0);
                ui.label(RichText::new(err).color(AMBER));
                ui.add_space(16.0);
                if ui.button("Open settings").clicked() {
                    self.open_settings();
                }
                if ui.button("Try again").clicked() {
                    self.start_engine();
                }
            });
            return;
        }
        let Some(engine) = &self.engine else { return };
        let (cw, ch) = engine.info.canvas;
        let canvas_aspect = cw as f32 / ch as f32;
        let n = engine.sources.len();
        if self.edit_mode {
            self.edit_toolbar(ui);
            ui.add_space(6.0);
        }
        let avail = ui.available_rect_before_wrap();
        ui.allocate_rect(avail, Sense::hover());
        let preview_mode = self.preview_mode() && !self.edit_mode;
        let top_h = (avail.height() * if preview_mode { 0.5 } else { 0.62 }).max(120.0);
        // The vertical version's monitor sits to the right of the program.
        let tall_aspect = self.cfg.vertical.enabled.then(|| {
            let (w, h) = self.cfg.vertical.size();
            w as f32 / h as f32
        });
        let gap = 14.0;
        let tall_gap = if tall_aspect.is_some() { gap } else { 0.0 };
        let mv_top;
        let tall_rect;
        if preview_mode {
            // Preview (left) and program (right).
            let ta = tall_aspect.unwrap_or(0.0);
            let mut h = top_h;
            let width = |h: f32| h * canvas_aspect * 2.0 + gap + h * ta + tall_gap;
            if width(h) > avail.width() {
                h = (avail.width() - gap - tall_gap) / (canvas_aspect * 2.0 + ta);
            }
            let w = h * canvas_aspect;
            let left = avail.center().x - width(h) / 2.0;
            let pv = Rect::from_min_size(pos2(left, avail.top()), vec2(w, h));
            let pg = Rect::from_min_size(pos2(left + w + gap, avail.top()), vec2(w, h));
            self.preview_monitor(ui, pv);
            self.program_monitor(ui, pg);
            tall_rect = Rect::from_min_size(pos2(pg.right() + tall_gap, avail.top()), vec2(h * ta, h));
            mv_top = pg.bottom() + 12.0;
        } else {
            let ta = tall_aspect.unwrap_or(0.0);
            let mut prog_h = top_h;
            if prog_h * (canvas_aspect + ta) + tall_gap > avail.width() {
                prog_h = (avail.width() - tall_gap) / (canvas_aspect + ta);
            }
            let prog_w = prog_h * canvas_aspect;
            let total = prog_w + tall_gap + prog_h * ta;
            let prog_rect = Rect::from_min_size(pos2(avail.center().x - total / 2.0, avail.top()), vec2(prog_w, prog_h));
            self.program_monitor(ui, prog_rect);
            tall_rect = Rect::from_min_size(pos2(prog_rect.right() + tall_gap, avail.top()), vec2(prog_h * ta, prog_h));
            mv_top = prog_rect.bottom() + 12.0;
        }
        if tall_aspect.is_some() && !self.edit_mode {
            self.tall_monitor(ui, tall_rect);
        }
        let mv = Rect::from_min_max(pos2(avail.left(), mv_top), pos2(avail.right(), avail.bottom()));
        if n > 0 && mv.height() > 40.0 {
            self.multiview(ui, mv);
        }
    }

    fn preview_monitor(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect, 4.0, Color32::BLACK);
        let engine = self.engine.as_ref().unwrap();
        let slot = engine.preview_bus.clone();
        let pv = engine.switcher.preview();
        if pv.is_some() {
            if let Some(tex) = self.texture(&ctx, "previewbus", &slot) {
                painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        } else {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Click a scene to preview it", FontId::proportional(14.0), TEXT_DIM);
        }
        painter.rect_stroke(rect, 4.0, Stroke::new(2.0, GREEN), StrokeKind::Outside);
        let badge = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(74.0, 22.0));
        painter.rect_filled(badge, 3.0, GREEN_DIM);
        painter.text(badge.center(), Align2::CENTER_CENTER, "PREVIEW", FontId::proportional(11.5), Color32::WHITE);
        if let Some(p) = pv {
            label_chip(&painter, badge.right_center() + vec2(12.0, 0.0), &p.name);
        }
        // Take / cut under the preview.
        let take = Rect::from_min_size(pos2(rect.right() - 170.0, rect.bottom() - 46.0), vec2(80.0, 36.0));
        let cut = Rect::from_min_size(pos2(rect.right() - 86.0, rect.bottom() - 46.0), vec2(76.0, 36.0));
        let rt = ui.interact(take, ui.scope_id().with("take_btn"), Sense::click());
        let rc = ui.interact(cut, ui.scope_id().with("cut_btn"), Sense::click());
        painter.rect_filled(take, 6.0, if rt.hovered() { Color32::from_rgb(250, 70, 80) } else { RED });
        painter.text(take.center(), Align2::CENTER_CENTER, "TAKE", FontId::proportional(14.0), Color32::WHITE);
        painter.rect_filled(cut, 6.0, if rc.hovered() { Color32::from_rgb(60, 66, 80) } else { RAISED });
        painter.text(cut.center(), Align2::CENTER_CENTER, "CUT", FontId::proportional(14.0), Color32::WHITE);
        if rt.on_hover_text(format!("{} to program{}", self.cfg.transition.mode.label(), self.key_hint(Builtin::Take))).clicked() {
            self.take_preview(false);
        }
        if rc.on_hover_text("Straight to program (Enter)").clicked() {
            self.take_preview(true);
        }
    }

    /// The vertical version, as it's recorded and streamed.
    fn tall_monitor(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect, 4.0, Color32::BLACK);
        let engine = self.engine.as_ref().unwrap();
        let slot = engine.tall_preview.clone();
        let recording = engine.is_recording() && self.cfg.vertical.record;
        if let Some(tex) = self.texture(&ctx, "tall", &slot) {
            painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        painter.rect_stroke(rect, 4.0, Stroke::new(if recording { 2.0 } else { 1.0 }, if recording { RED } else { RED_DIM }), StrokeKind::Outside);
        let badge = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(26.0, 18.0));
        painter.rect_filled(badge, 3.0, Color32::from_black_alpha(170));
        painter.text(badge.center(), Align2::CENTER_CENTER, "9:16", FontId::proportional(10.0), Color32::WHITE);
        let r = ui.interact(rect, ui.scope_id().with("tall_monitor"), Sense::click());
        if r.on_hover_text("The vertical version (click to set its layouts and framing)").clicked() {
            self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::Vertical));
        }
    }

    fn program_monitor(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect, 4.0, Color32::BLACK);
        let slot = self.engine.as_ref().unwrap().program_preview.clone();
        if let Some(tex) = self.texture(&ctx, "program", &slot) {
            painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        let engine = self.engine.as_ref().unwrap();
        let recording = engine.is_recording();
        painter.rect_stroke(rect, 4.0, Stroke::new(if recording { 3.0 } else { 1.5 }, if recording { RED } else { RED_DIM }), StrokeKind::Outside);
        let live = engine.switcher.live();
        if self.edit_mode {
            return self.edit_overlay(ui, rect, live);
        }
        let badge = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(74.0, 22.0));
        painter.rect_filled(badge, 3.0, RED);
        painter.text(badge.center(), Align2::CENTER_CENTER, "PROGRAM", FontId::proportional(11.5), Color32::WHITE);
        let name_chip = label_chip(&painter, badge.right_center() + vec2(12.0, 0.0), &live.name);
        // Status chips after the scene name (the top right is where logos usually sit).
        let mut x = name_chip.right() + 8.0;
        let auto = engine.auto_switch_enabled();
        let auto_st = engine.status.lock_ok().auto_switch.clone();
        let mut chips: Vec<(String, Color32)> = Vec::new();
        if auto {
            let talking = auto_st.talking.iter().filter(|t| **t).count();
            let text = if auto_st.paused_for > 0.0 { format!("AUTO (paused {:.0}s)", auto_st.paused_for.ceil()) } else if talking > 0 { format!("AUTO · {talking} talking") } else { "AUTO".into() };
            chips.push((text, ACCENT));
        }
        if engine.virtual_camera_on() {
            chips.push(("WEBCAM".into(), Color32::from_rgb(120, 120, 220)));
        }
        if self.cfg.display.output != DisplayOutput::Off {
            chips.push(("SCREEN".into(), Color32::from_rgb(100, 140, 160)));
        }
        for (text, color) in chips {
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
            let r = Rect::from_min_size(pos2(x, badge.center().y - 10.0), vec2(galley.size().x + 14.0, 20.0));
            painter.rect_filled(r, 3.0, color);
            painter.galley(r.min + vec2(7.0, 3.0), galley, Color32::WHITE);
            x = r.right() + 6.0;
        }
        // Captions that only go to the stream aren't in this picture:
        // show what's being sent, so the operator can follow along.
        if let Some(c) = engine.captions().filter(|c| c.showing() && !self.cfg.captions.on_recording) {
            let text = c.text();
            if !text.is_empty() {
                let font = FontId::proportional((rect.height() * 0.045).clamp(11.0, 18.0));
                let galley = painter.layout(text, font, Color32::WHITE, rect.width() * 0.8);
                let size = galley.size();
                let at = pos2(rect.center().x - size.x / 2.0, rect.bottom() - size.y - rect.height() * 0.07);
                painter.rect_filled(Rect::from_min_size(at, size).expand(6.0), 4.0, Color32::from_black_alpha(190));
                painter.galley(at, galley, Color32::WHITE);
                let tag = Rect::from_min_size(pos2(at.x - 6.0, at.y - 26.0), vec2(88.0, 16.0));
                painter.text(tag.left_center(), Align2::LEFT_CENTER, "CC · stream only", FontId::proportional(10.5), Color32::from_white_alpha(170));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        if engine.switcher.in_transition() || auto {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Frames over the program monitor that can be dragged and resized.
    fn edit_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, live: Scene) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect.expand(4.0));
        let engine = self.engine.as_ref().unwrap();
        let names: Vec<String> = engine.sources.iter().map(|s| s.name.clone()).collect();
        let mut scene = live;
        // Each source's shape (after its own crop), for moving its picture.
        let aspects: Vec<f32> = engine.sources.iter().enumerate().map(|(i, s)| self.cfg.sources.get(i).map(|c| c.look.cropped_aspect(s.size.0 as f32 / s.size.1.max(1) as f32)).unwrap_or(16.0 / 9.0)).collect();
        let canvas = engine.info.canvas;
        let to_screen = |l: &Layer| Rect::from_min_size(rect.min + vec2(l.x * rect.width(), l.y * rect.height()), vec2(l.w * rect.width(), l.h * rect.height()));
        let handle = 12.0;
        let id = ui.scope_id().with("program_edit");
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let hit_at = |layers: &[Layer], p: egui::Pos2| layers.iter().enumerate().rev().find(|(_, l)| to_screen(l).contains(p)).map(|(i, _)| i);

        // Scrolling over a frame zooms its picture; double-clicking it
        // switches between fitting and filling the frame.
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.1 {
            if let Some(i) = pointer.and_then(|p| hit_at(&scene.layers, p)) {
                self.remember(undo::Kind::Scenes, format!("Zoom {}", names.get(scene.layers[i].source).map(String::as_str).unwrap_or("frame")));
                let l = &mut scene.layers[i];
                l.zoom = (l.zoom * (scroll * 0.004).exp()).clamp(1.0, crate::scene::MAX_ZOOM);
                self.selected_layer = Some(i);
                self.apply_live_edit(scene.clone());
            }
        }
        if resp.double_clicked() {
            if let Some(i) = pointer.and_then(|p| hit_at(&scene.layers, p)) {
                let name = names.get(scene.layers[i].source).cloned().unwrap_or_default();
                self.remember(undo::Kind::Scenes, if scene.layers[i].fill { format!("Fit {name}") } else { format!("Fill with {name}") });
                scene.layers[i].fill = !scene.layers[i].fill;
                self.selected_layer = Some(i);
                self.apply_live_edit(scene.clone());
            }
        }

        if resp.drag_started() || resp.clicked() {
            if let Some(p) = pointer {
                let hit = scene.layers.iter().enumerate().rev().find_map(|(i, l)| {
                    let r = to_screen(l);
                    let corner = Rect::from_center_size(r.right_bottom(), vec2(handle * 2.0, handle * 2.0));
                    if corner.contains(p) {
                        Some((i, true))
                    } else if r.contains(p) {
                        Some((i, false))
                    } else {
                        None
                    }
                });
                self.selected_layer = hit.map(|h| h.0);
                if resp.drag_started() {
                    let pan = ui.input(|i| i.modifiers.alt);
                    self.drag = hit.map(|(layer, resize)| Drag { layer, resize: resize && !pan, pan });
                    if let Some(d) = &self.drag {
                        let name = scene.layers.get(d.layer).and_then(|l| names.get(l.source)).cloned().unwrap_or_default();
                        let label = if d.pan { format!("Move the picture of {name}") } else if d.resize { format!("Resize {name}") } else { format!("Move {name}") };
                        self.remember(undo::Kind::Scenes, label);
                    }
                }
            }
        }
        if let Some(d) = &self.drag {
            if resp.dragged() {
                let delta = resp.drag_delta();
                let (dx, dy) = (delta.x / rect.width(), delta.y / rect.height());
                let keep_aspect = ui.input(|i| i.modifiers.shift);
                if let Some(l) = scene.layers.get_mut(d.layer) {
                    if d.pan {
                        // The picture follows the pointer: the view moves
                        // the other way, by the part of it that shows.
                        let aspect = aspects.get(l.source).copied().unwrap_or(16.0 / 9.0);
                        let (frame, part) = crate::scene::place(l, canvas, aspect);
                        let k = rect.width() / canvas.0 as f32;
                        let (fw, fh) = ((frame.2 * k).max(1.0), (frame.3 * k).max(1.0));
                        l.focus[0] = (l.focus[0] - delta.x / fw * (part[2] - part[0])).clamp(0.0, 1.0);
                        l.focus[1] = (l.focus[1] - delta.y / fh * (part[3] - part[1])).clamp(0.0, 1.0);
                        // Kept where the view can actually be, so moving back
                        // responds at once.
                        let (pw, ph) = ((part[2] - part[0]) / 2.0, (part[3] - part[1]) / 2.0);
                        l.focus = [l.focus[0].clamp(pw, 1.0 - pw), l.focus[1].clamp(ph, 1.0 - ph)];
                    } else if d.resize {
                        let aspect = l.w / l.h.max(1e-4);
                        l.w = (l.w + dx).clamp(0.05, 1.5);
                        l.h = if keep_aspect { l.w / aspect } else { (l.h + dy).clamp(0.05, 1.5) };
                    } else {
                        l.x = snap(l.x + dx, l.w);
                        l.y = snap(l.y + dy, l.h);
                    }
                }
                self.apply_live_edit(scene.clone());
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
        }
        for (i, l) in scene.layers.iter().enumerate() {
            let r = to_screen(l);
            let selected = self.selected_layer == Some(i);
            let color = if selected { AMBER } else { ACCENT };
            painter.rect_stroke(r, 0.0, Stroke::new(if selected { 2.5 } else { 1.5 }, color), StrokeKind::Inside);
            let name = names.get(l.source).cloned().unwrap_or_else(|| format!("Source {}", l.source + 1));
            let tag = painter.text(r.min + vec2(6.0, 5.0), Align2::LEFT_TOP, &name, FontId::proportional(12.0), Color32::WHITE);
            painter.rect_filled(tag.expand(3.0), 2.0, Color32::from_black_alpha(150));
            painter.text(r.min + vec2(6.0, 5.0), Align2::LEFT_TOP, &name, FontId::proportional(12.0), Color32::WHITE);
            let corner = Rect::from_min_max(r.right_bottom() - vec2(handle, handle), r.right_bottom());
            painter.rect_filled(corner, 2.0, color);
            if l.crops() {
                let text = match (l.fill, l.zoom > 1.0001) {
                    (true, true) => format!("FILL · {:.1}×", l.zoom),
                    (true, false) => "FILL".to_string(),
                    _ => format!("{:.1}×", l.zoom),
                };
                let at = pos2(r.right() - 6.0, r.top() + 5.0);
                let tag = painter.text(at, Align2::RIGHT_TOP, &text, FontId::proportional(11.0), Color32::WHITE);
                painter.rect_filled(tag.expand(3.0), 2.0, Color32::from_black_alpha(150));
                painter.text(at, Align2::RIGHT_TOP, &text, FontId::proportional(11.0), Color32::WHITE);
            }
        }
        if resp.hovered() {
            let panning = self.drag.as_ref().is_some_and(|d| d.pan) || (self.drag.is_none() && ui.input(|i| i.modifiers.alt));
            ctx.set_cursor_icon(if panning {
                egui::CursorIcon::Move
            } else if self.drag.as_ref().is_some_and(|d| d.resize) {
                egui::CursorIcon::ResizeNwSe
            } else {
                egui::CursorIcon::Grab
            });
        }
    }

    fn apply_live_edit(&mut self, scene: Scene) {
        if let Some(e) = &self.engine {
            e.switcher.set_live(scene.clone());
        }
        if let Some(s) = self.scenes.iter_mut().find(|s| s.name == scene.name) {
            *s = scene;
            self.scenes_dirty = true;
        }
    }

    fn edit_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else { return };
        let mut scene = engine.switcher.live();
        let names: Vec<String> = engine.sources.iter().map(|s| s.name.clone()).collect();
        let inputs: Vec<String> = engine.audio.iter().map(|a| a.name.clone()).collect();
        let cue_suggestions = control::suggestions(&self.cfg);
        let mut changed = false;
        let mut new_scene: Option<Scene> = None;
        let mut delete = false;
        let (mut undo_clicked, mut redo_clicked) = (false, false);
        let old_name = scene.name.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("EDIT LAYOUT").color(AMBER).strong());
            ui.label(RichText::new(if cfg!(target_os = "macos") { "drag frames · corner to resize · Shift keeps shape · scroll to zoom · Option-drag moves the picture" } else { "drag frames · corner to resize · Shift keeps shape · scroll to zoom · Alt-drag moves the picture" }).color(TEXT_DIM).size(11.5));
            ui.separator();
            ui.label("Name");
            let mut name = scene.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(130.0)).changed() {
                scene.name = name;
                changed = true;
            }
            egui::ComboBox::from_id_salt("add_layer").selected_text("Add source").show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(false, n).clicked() {
                        scene.layers.push(Layer::at(i, 0.3, 0.3, 0.4, 0.4));
                        changed = true;
                    }
                }
            });
            let sel = self.selected_layer.filter(|&i| i < scene.layers.len());
            if ui.add_enabled(sel.is_some(), egui::Button::new("Remove")).clicked() {
                scene.layers.remove(sel.unwrap());
                self.selected_layer = None;
                changed = true;
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("To front")).clicked() {
                let l = scene.layers.remove(sel.unwrap());
                scene.layers.push(l);
                self.selected_layer = Some(scene.layers.len() - 1);
                changed = true;
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("Full frame")).clicked() {
                let l = &mut scene.layers[sel.unwrap()];
                (l.x, l.y, l.w, l.h) = (0.0, 0.0, 1.0, 1.0);
                changed = true;
            }
            // The picture in the selected frame: fitted or filling, and zoom.
            if let Some(i) = sel {
                ui.separator();
                let l = &mut scene.layers[i];
                if ui.selectable_label(!l.fill, "Fit").on_hover_text("Show the whole picture (bars where the frame is another shape)").clicked() {
                    l.fill = false;
                    changed = true;
                }
                if ui.selectable_label(l.fill, "Fill").on_hover_text("Fill the frame, cropping the picture to its shape (or double-click the frame)").clicked() {
                    l.fill = true;
                    changed = true;
                }
                ui.label("Zoom");
                if ui.add(egui::Slider::new(&mut l.zoom, 1.0..=crate::scene::MAX_ZOOM).step_by(0.05).fixed_decimals(1).suffix("×")).on_hover_text("Or scroll over the frame").changed() {
                    changed = true;
                }
                if ui.add_enabled(l.crops() || l.focus != [0.5, 0.5], egui::Button::new("Reset picture")).on_hover_text("Fit, no zoom, centred").clicked() {
                    (l.fill, l.zoom, l.focus) = (false, 1.0, [0.5, 0.5]);
                    changed = true;
                }
            }
            // Scene audio: inputs this scene turns on or off.
            let n_audio = scene.audio_on.len() + scene.audio_off.len();
            ui.menu_button(if n_audio > 0 { format!("Scene audio ({n_audio})") } else { "Scene audio".into() }, |ui| {
                ui.label(RichText::new("When this scene is taken:").strong());
                for input in &inputs {
                    let state = if scene.audio_on.iter().any(|a| a.eq_ignore_ascii_case(input)) {
                        1
                    } else if scene.audio_off.iter().any(|a| a.eq_ignore_ascii_case(input)) {
                        2
                    } else {
                        0
                    };
                    let mut new_state = state;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(input).strong());
                        ui.selectable_value(&mut new_state, 0, "leave");
                        ui.selectable_value(&mut new_state, 1, "turn on");
                        ui.selectable_value(&mut new_state, 2, "turn off");
                    });
                    if new_state != state {
                        scene.audio_on.retain(|a| !a.eq_ignore_ascii_case(input));
                        scene.audio_off.retain(|a| !a.eq_ignore_ascii_case(input));
                        match new_state {
                            1 => scene.audio_on.push(input.clone()),
                            2 => scene.audio_off.push(input.clone()),
                            _ => {}
                        }
                        changed = true;
                    }
                }
                ui.label(RichText::new("e.g. a break scene that mutes the mics and brings up the music. Changes land with the picture and fade with mixes.").size(11.5).color(TEXT_DIM));
            });
            // Scene cues: commands run when the scene is taken.
            let n_cues = scene.cues.len();
            ui.menu_button(if n_cues > 0 { format!("Scene cues ({n_cues})") } else { "Scene cues".into() }, |ui| {
                ui.set_min_width(300.0);
                ui.label(RichText::new("When this scene is taken, also:").strong());
                let label_of = |c: &str| cue_suggestions.iter().find(|(cmd, _)| cmd.eq_ignore_ascii_case(c)).map(|(_, l)| l.clone()).or_else(|| Action::parse(c).ok().map(|a| a.describe())).unwrap_or_else(|| c.to_string());
                let mut remove = None;
                for (i, c) in scene.cues.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(label_of(c));
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    scene.cues.remove(i);
                    changed = true;
                }
                ui.menu_button("＋ Add", |ui| {
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        let skip = |c: &str| ["scene:", "cut:", "preview:", "source:", "rundown:"].iter().any(|p| c.starts_with(p)) || c == "take" || c == "cut";
                        for (cmd, l) in cue_suggestions.iter().filter(|(c, _)| !skip(c)) {
                            if ui.button(l).clicked() {
                                scene.cues.push(cmd.clone());
                                changed = true;
                                ui.close();
                            }
                        }
                    });
                });
                ui.label(RichText::new("e.g. start a Syrinscape mood, play a pad, show a title. Any command works (Settings → Remote & MIDI lists them); edit them in the settings file.").size(11.5).color(TEXT_DIM));
            });
            ui.separator();
            if ui.button("Duplicate scene").clicked() {
                let mut s = scene.clone();
                s.name = unique_name(&self.scenes, &format!("{} copy", scene.name));
                new_scene = Some(s);
            }
            if ui.button("Delete scene").clicked() {
                delete = true;
            }
            let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
            if ui.add_enabled(self.history.can_undo().is_some(), egui::Button::new("↶ Undo")).on_hover_text(match self.history.can_undo() {
                Some(l) => format!("Undo: {l} ({cmd}Z)"),
                None => format!("Nothing to undo ({cmd}Z)"),
            }).clicked() {
                undo_clicked = true;
            }
            if ui.add_enabled(self.history.can_redo().is_some(), egui::Button::new("↷ Redo")).on_hover_text(match self.history.can_redo() {
                Some(l) => format!("Redo: {l} ({cmd}Shift+Z)"),
                None => format!("Nothing to redo ({cmd}Shift+Z)"),
            }).clicked() {
                redo_clicked = true;
            }
            if ui.add_enabled(self.scenes_dirty, egui::Button::new("Save scenes")).clicked() {
                self.save_config();
            }
            if ui.button("Done").clicked() {
                self.edit_mode = false;
            }
        });
        if changed || new_scene.is_some() || (delete && self.scenes.len() > 1) {
            let label = if delete {
                format!("Delete scene {old_name}")
            } else if new_scene.is_some() {
                format!("Duplicate scene {old_name}")
            } else if scene.name != old_name {
                format!("Rename scene {old_name}")
            } else {
                format!("Edit scene {old_name}")
            };
            self.remember(undo::Kind::Scenes, label);
        }
        if changed {
            if let Some(e) = &self.engine {
                e.switcher.set_live(scene.clone());
            }
            if let Some(s) = self.scenes.iter_mut().find(|s| s.name == old_name) {
                *s = scene;
            }
            self.scenes_dirty = true;
        }
        if let Some(s) = new_scene {
            self.scenes.push(s.clone());
            self.scenes_dirty = true;
            if let Some(e) = &self.engine {
                e.switcher.set_live(s);
            }
        }
        if delete && self.scenes.len() > 1 {
            self.scenes.retain(|s| s.name != old_name);
            self.scenes_dirty = true;
            if let Some(s) = self.scenes.first().cloned() {
                self.take_scene(&s, true);
            }
        }
        let ctx = ui.ctx().clone();
        if undo_clicked {
            self.undo(&ctx);
        }
        if redo_clicked {
            self.redo(&ctx);
        }
    }

    fn multiview(&mut self, ui: &mut egui::Ui, area: Rect) {
        let ctx = ui.ctx().clone();
        let engine = self.engine.as_ref().unwrap();
        let n = engine.sources.len();
        let live_sources = engine.switcher.live().sources();
        let preview_sources = engine.switcher.preview().map(|p| p.sources()).unwrap_or_default();
        let gap = 10.0;
        let label_h = 22.0;
        let mut best = (1usize, 0.0f32);
        let min_aspect = engine.sources.iter().map(|s| s.size.0 as f32 / s.size.1 as f32).fold(f32::MAX, f32::min);
        for cols in 1..=n {
            let rows = n.div_ceil(cols);
            let by_width = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let h_each = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32 - label_h;
            let w = by_width.min(h_each * min_aspect);
            if w > best.1 {
                best = (cols, w);
            }
        }
        let (cols, tile_w) = best;
        let total_w = cols as f32 * tile_w + gap * (cols as f32 - 1.0);
        let left = area.center().x - total_w / 2.0;
        let offline_set = engine.status.lock_ok().offline.clone();
        let infos: Vec<(String, Arc<FrameSlot>, f32, bool, Option<String>)> = engine
            .sources
            .iter()
            .map(|s| {
                let net = s.input.net.lock_ok();
                let detail = (!net.connected && !net.detail.is_empty()).then(|| net.detail.clone());
                (s.name.clone(), s.preview.clone(), s.size.1 as f32 / s.size.0 as f32, offline_set.contains(&s.name), detail)
            })
            .collect();
        // Guests whose connection is struggling, by source.
        let links: std::collections::HashMap<String, (crate::engine::guests::LinkQuality, String)> = if self.cfg.guests.enabled {
            crate::engine::guests::hub().guests().into_iter().filter_map(|g| Some((g.source, g.link.filter(|l| l.0 != crate::engine::guests::LinkQuality::Good)?))).collect()
        } else {
            Default::default()
        };
        let players: Vec<Option<Arc<crate::engine::player::PlayerControl>>> = (0..n).map(|i| engine.player(i)).collect();
        let framers: Vec<Option<Arc<crate::engine::framing::FramingShared>>> = engine.sources.iter().map(|s| s.input.framing().cloned()).collect();
        let mut clicked: Option<(usize, bool)> = None;
        let mut player_cmd: Option<(usize, PlayerCmd)> = None;
        let mut framing_change: Option<(usize, crate::config::FramingConfig)> = None;
        for (i, (name, slot, inv_aspect, offline, detail)) in infos.iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            let tile_h = tile_w * inv_aspect;
            let x = left + c as f32 * (tile_w + gap);
            let y = area.top() + r as f32 * (tile_h + label_h + gap);
            let pic = Rect::from_min_size(pos2(x, y), vec2(tile_w, tile_h));
            let full = Rect::from_min_size(pic.min, vec2(tile_w, tile_h + label_h));
            let resp = ui.interact(full, ui.scope_id().with(("tile", i)), Sense::click());
            let painter = ui.painter_at(full.expand(4.0));
            painter.rect_filled(pic, 3.0, Color32::BLACK);
            if let Some(tex) = self.texture(&ctx, &format!("src{i}"), slot) {
                painter.image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let wait = waiting::TileWait::of(slot, *offline, detail.is_some());
            if wait != waiting::TileWait::Fine {
                waiting::draw_tile_wait(&painter, pic, wait, detail.as_deref(), ctx.input(|i| i.time) as f32);
                if wait != waiting::TileWait::NoSignal {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
            if let (waiting::TileWait::Fine, Some((q, note))) = (wait, links.get(name)) {
                waiting::draw_link_warning(&painter, pic, *q);
                if resp.hovered() {
                    resp.clone().on_hover_text(format!("{name}'s connection: {note}"));
                }
            }
            let on_air = live_sources.contains(&i);
            let in_preview = preview_sources.contains(&i);
            let bar = Rect::from_min_size(pic.left_bottom(), vec2(tile_w, label_h));
            painter.rect_filled(bar, 0.0, if on_air { RED } else if in_preview { GREEN_DIM } else { RAISED });
            painter.text(bar.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, format!("{}  {}", i + 1, name), FontId::proportional(13.0), Color32::WHITE);
            // Media player transport.
            if let Some(p) = &players[i] {
                let st = p.state.lock_ok().clone();
                if st.duration > 0 {
                    let k = (st.position as f32 / st.duration as f32).clamp(0.0, 1.0);
                    let track = Rect::from_min_size(pic.left_bottom() - vec2(0.0, 4.0), vec2(tile_w, 4.0));
                    painter.rect_filled(track, 0.0, Color32::from_black_alpha(160));
                    painter.rect_filled(Rect::from_min_size(track.min, vec2(tile_w * k, 4.0)), 0.0, ACCENT);
                }
                let mut bx = bar.right() - 4.0;
                for (glyph, cmd, tip) in [("⏭", PlayerCmd::Next, "Next"), (if st.playing { "⏸" } else { "▶" }, PlayerCmd::TogglePause, "Play / pause"), ("⟲", PlayerCmd::Restart, "From the start")] {
                    let br = Rect::from_min_size(pos2(bx - 22.0, bar.top() + 1.0), vec2(22.0, label_h - 2.0));
                    let rr = ui.interact(br, ui.scope_id().with(("media", i, tip)), Sense::click());
                    painter.rect_filled(br, 3.0, if rr.hovered() { LINE } else { Color32::TRANSPARENT });
                    painter.text(br.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(13.0), Color32::WHITE);
                    if rr.on_hover_text(tip).clicked() {
                        player_cmd = Some((i, cmd));
                    }
                    bx -= 24.0;
                }
                if let Some(item) = st.items.get(st.index) {
                    painter.text(pic.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, item, FontId::proportional(11.0), Color32::from_white_alpha(200));
                }
            }
            // Auto-framing: who it's following.
            if let Some(f) = &framers[i] {
                let cfg = f.cfg.lock_ok().clone();
                let people = f.people.load(std::sync::atomic::Ordering::Relaxed);
                let text = if f.problem.lock_ok().is_some() {
                    "AUTO-FRAME OFF · whole picture".to_string()
                } else if people == 0 {
                    "AUTO · looking for people".to_string()
                } else {
                    format!("AUTO · {} · {}", cfg.shot.label(), cfg.follow.label())
                };
                let font = FontId::proportional(10.5);
                let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
                let chip = Rect::from_min_size(pic.left_top() + vec2(5.0, 5.0), galley.size() + vec2(10.0, 4.0));
                painter.rect_filled(chip, 3.0, Color32::from_black_alpha(150));
                painter.galley(chip.min + vec2(5.0, 2.0), galley, Color32::WHITE);
                let mut changed = cfg.clone();
                resp.context_menu(|ui| {
                    ui.label(RichText::new("Follow").strong());
                    for v in crate::config::FramingFollow::ALL {
                        if ui.selectable_label(changed.follow == v, v.label()).clicked() {
                            changed.follow = v;
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("Shot").strong());
                    for v in crate::config::FramingShot::ALL {
                        if ui.selectable_label(changed.shot == v, v.label()).clicked() {
                            changed.shot = v;
                            ui.close();
                        }
                    }
                });
                if changed != cfg {
                    framing_change = Some((i, changed));
                }
            }
            let stroke = if on_air {
                Stroke::new(3.0, RED)
            } else if in_preview {
                Stroke::new(3.0, GREEN)
            } else if resp.hovered() {
                Stroke::new(1.5, ACCENT)
            } else {
                Stroke::new(1.0, LINE)
            };
            painter.rect_stroke(full, 3.0, stroke, StrokeKind::Outside);
            if resp.clicked() {
                clicked = Some((i, ui.input(|inp| inp.modifiers.shift)));
            }
            if let Some(p) = &players[i] {
                let st = p.state.lock_ok().clone();
                resp.context_menu(|ui| {
                    ui.label(RichText::new("Playlist").strong());
                    for (k, item) in st.items.iter().enumerate() {
                        if ui.selectable_label(k == st.index, item).clicked() {
                            player_cmd = Some((i, PlayerCmd::Select(k)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Previous").clicked() {
                        player_cmd = Some((i, PlayerCmd::Previous));
                        ui.close();
                    }
                });
            }
            let base = if self.preview_mode() { "Click: preview this source full screen. Shift+click: cut to it." } else { "Click: this source full screen. Shift+click: cut." };
            let tip = if players[i].is_some() {
                format!("{base} Right-click: playlist.")
            } else if framers[i].is_some() {
                format!("{base} Right-click: who to follow, and how tight.")
            } else {
                base.to_string()
            };
            resp.on_hover_text(tip);
        }
        if let Some((i, f)) = framing_change {
            if let Some(s) = self.cfg.sources.get_mut(i) {
                s.framing = f;
                self.cfg_dirty = true;
            }
            if let Some(e) = &self.engine {
                let _ = e.update_config(&self.cfg);
            }
            clicked = None;
        }
        if let Some((i, cmd)) = player_cmd {
            if let Some(p) = &players[i] {
                p.send(cmd);
            }
            clicked = None;
        }
        if let Some((i, cut)) = clicked {
            self.show_source(i, cut);
        }
    }

    /// A source full screen: its own scene if there is one. To preview in
    /// preview mode (unless cutting).
    fn show_source(&mut self, i: usize, cut: bool) {
        let idx = self.scenes.iter().position(|s| s.layers.len() == 1 && s.layers[0].source == i && s.layers[0].w >= 0.999 && s.layers[0].h >= 0.999);
        match idx {
            Some(idx) => self.press_scene(idx, cut),
            None => {
                let name = self.cfg.sources.get(i).map(|s| s.name.clone()).unwrap_or_else(|| format!("Source {}", i + 1));
                let scene = Scene { name, layers: vec![Layer::at(i, 0.0, 0.0, 1.0, 1.0)], ..Default::default() };
                if self.preview_mode() && !cut {
                    if let Some(e) = &self.engine {
                        e.switcher.set_preview(Some(scene));
                    }
                } else {
                    self.take_scene(&scene, cut);
                }
            }
        }
    }

    // ---- scene bank -------------------------------------------------------------

    fn scene_bank(&mut self, ui: &mut egui::Ui) {
        let live_name = self.engine.as_ref().map(|e| e.switcher.live().name);
        let preview_name = self.engine.as_ref().and_then(|e| e.switcher.preview()).map(|p| p.name);
        let mut press: Option<(usize, bool)> = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("TRANSITION").color(TEXT_DIM).size(11.0));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let mut mode = self.cfg.transition.mode;
                    for m in TransitionMode::ALL {
                        let b = ui.add(egui::Button::selectable(mode == m, RichText::new(m.label().to_uppercase()).strong().size(12.0)).min_size(vec2(46.0, 26.0)));
                        if b.clicked() {
                            mode = m;
                        }
                    }
                    if mode != self.cfg.transition.mode {
                        self.cfg.transition.mode = mode;
                        self.cfg_dirty = true;
                    }
                    ui.add_space(6.0);
                    // Fade to black: flashes while the program is black.
                    let black = self.engine.as_ref().is_some_and(|e| e.is_black());
                    let lit = black && (ui.input(|i| i.time) * 2.0).fract() < 0.5;
                    let ftb = egui::Button::new(RichText::new("FTB").strong().size(12.0).color(if black { Color32::WHITE } else { TEXT_DIM }))
                        .fill(if lit { RED } else if black { RED_DIM } else { RAISED })
                        .min_size(vec2(46.0, 26.0));
                    if ui.add(ftb).on_hover_text(if black { "Fade back up from black" } else { "Fade to black (picture and sound; the length is in Settings → Switching)" }).clicked() {
                        self.act(Action::Ftb(Toggle::Toggle));
                    }
                    if black {
                        ui.ctx().request_repaint_after(Duration::from_millis(250));
                    }
                });
                ui.horizontal(|ui| {
                    let mut ms = self.cfg.transition.duration_ms;
                    if ui.add(egui::DragValue::new(&mut ms).range(0..=3000).speed(10).suffix(" ms")).on_hover_text("Transition length").changed() {
                        self.cfg.transition.duration_ms = ms;
                        self.cfg_dirty = true;
                    }
                    let mut pv = self.cfg.transition.preview_mode;
                    if ui.add(egui::Button::selectable(pv, "Preview")).on_hover_text(format!("Pick the next scene in preview, then Take{} or Cut{}", self.key_hint(Builtin::Take), self.key_hint(Builtin::CutPreview))).clicked() {
                        pv = !pv;
                        self.cfg.transition.preview_mode = pv;
                        self.cfg_dirty = true;
                        if !pv {
                            if let Some(e) = &self.engine {
                                e.switcher.set_preview(None);
                            }
                        }
                    }
                    let auto = self.auto_on();
                    let auto_btn = egui::Button::selectable(auto, RichText::new("AUTO").strong().color(if auto { Color32::WHITE } else { TEXT_DIM }));
                    if ui.add(auto_btn).on_hover_text(format!("Switch automatically to whoever is talking{}. Set up in Settings → Switching.", self.key_hint(Builtin::Auto))).clicked() {
                        self.set_auto(!auto);
                    }
                    if let Some(c) = self.engine.as_ref().and_then(|e| e.captions()) {
                        let on = c.showing();
                        let status = c.status();
                        let ok = status == "listening";
                        let color = if !ok { AMBER } else if on { Color32::WHITE } else { TEXT_DIM };
                        let b = egui::Button::selectable(on, RichText::new("CC").strong().color(color));
                        let tip = if ok { format!("Live captions on screen{}", self.key_hint(Builtin::Captions)) } else { format!("Live captions: {status}") };
                        if ui.add(b).on_hover_text(tip).clicked() {
                            c.set_showing(!on);
                        }
                    }
                    let label = if self.edit_mode { "Editing" } else { "Edit layout" };
                    if ui.add(egui::Button::selectable(self.edit_mode, label)).on_hover_text(format!("Move and resize frames{}", self.key_hint(Builtin::Edit))).clicked() {
                        self.edit_mode = !self.edit_mode;
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::horizontal().id_salt("bank").show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, s) in self.scenes.iter().enumerate() {
                        let is_live = live_name.as_deref() == Some(s.name.as_str());
                        let is_preview = preview_name.as_deref() == Some(s.name.as_str());
                        let key = if i < 9 { format!("{}", i + 1) } else { String::new() };
                        let (rect, resp) = ui.allocate_exact_size(vec2(132.0, 84.0), Sense::click());
                        let painter = ui.painter_at(rect);
                        let fill = if is_live {
                            RED
                        } else if is_preview {
                            GREEN_DIM
                        } else if resp.hovered() {
                            Color32::from_rgb(44, 49, 59)
                        } else {
                            RAISED
                        };
                        painter.rect_filled(rect, 6.0, fill);
                        if is_preview {
                            painter.rect_stroke(rect, 6.0, Stroke::new(2.0, GREEN), StrokeKind::Inside);
                        }
                        let thumb = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(48.0, 27.0));
                        painter.rect_filled(thumb, 2.0, Color32::from_black_alpha(120));
                        for l in &s.layers {
                            let r = Rect::from_min_size(thumb.min + vec2(l.x * thumb.width(), l.y * thumb.height()), vec2(l.w * thumb.width(), l.h * thumb.height())).intersect(thumb);
                            painter.rect_filled(r, 1.0, Color32::from_white_alpha(70));
                            painter.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::from_white_alpha(160)), StrokeKind::Inside);
                        }
                        if !s.audio_on.is_empty() || !s.audio_off.is_empty() {
                            painter.text(thumb.right_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, "♪", FontId::proportional(13.0), Color32::from_white_alpha(180));
                        }
                        painter.text(rect.right_top() + vec2(-10.0, 9.0), Align2::RIGHT_TOP, &key, FontId::monospace(13.0), if is_live { Color32::WHITE } else { TEXT_DIM });
                        let name: String = s.name.chars().take(18).collect();
                        painter.text(rect.left_bottom() + vec2(10.0, -10.0), Align2::LEFT_BOTTOM, name, FontId::proportional(13.5), Color32::WHITE);
                        if resp.clicked() {
                            press = Some((i, ui.input(|inp| inp.modifiers.shift)));
                        }
                        let tip = match (i < 9, self.cfg.transition.preview_mode) {
                            (true, true) => format!("Press {} to preview (Shift+{} cuts to program)", i + 1, i + 1),
                            (true, false) => format!("Press {} (Shift+{} to cut)", i + 1, i + 1),
                            _ => "Click to take".into(),
                        };
                        resp.on_hover_text(tip);
                    }
                });
            });
        });
        if let Some((i, cut)) = press {
            self.press_scene(i, cut);
        }
    }

    // ---- right panel -------------------------------------------------------------

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(which) = self.confirm.clone() else { return };
        let (title, body, button) = match &which {
            Confirm::EndStream => ("End the live stream?".to_string(), "Viewers on every destination will see the broadcast end.".to_string(), "End stream"),
            Confirm::StopRecording => ("Stop recording?".to_string(), "The files for this take will be finished and saved.".to_string(), "Stop recording"),
            Confirm::SwitchShow(n) => (format!("Switch to \"{n}\"?"), "Sources, scenes, audio and everything else change to that show's setup. The current show keeps its changes.".to_string(), "Switch"),
            Confirm::Quit => {
                let (rec, live) = self.engine.as_ref().map_or((false, false), |e| (e.is_recording(), e.is_streaming()));
                quit_text(rec, live)
            }
        };
        let mut close = false;
        let mut switch_to = None;
        let r = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(title);
            ui.label(body);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(button).color(Color32::WHITE)).fill(RED)).clicked() {
                    if let Some(e) = &self.engine {
                        match &which {
                            Confirm::EndStream => e.stop_stream(),
                            Confirm::StopRecording => e.stop_recording(),
                            Confirm::SwitchShow(n) => switch_to = Some(n.clone()),
                            Confirm::Quit => {}
                        }
                    }
                    if which == Confirm::Quit {
                        self.quit_ok = true;
                        ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || r.should_close() {
            self.confirm = None;
        }
        if let Some(n) = switch_to {
            self.switch_show(&n, ctx);
        }
    }

    fn stream_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("STREAM").color(TEXT_DIM).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Set up").clicked() {
                    self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::Streaming));
                }
            });
        });
        let Some(engine) = &self.engine else { return };
        let stream = engine.status.lock_ok().stream.clone();
        let dot = |ui: &mut egui::Ui, color: Color32| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.5, color);
        };
        if stream.destinations.is_empty() {
            let dests: Vec<_> = self.cfg.streaming.destinations.iter().filter(|d| d.enabled).cloned().collect();
            if dests.is_empty() {
                ui.label(RichText::new("No destinations yet.").color(TEXT_DIM).size(12.0));
            }
            for d in dests {
                ui.horizontal(|ui| {
                    let problem = d.problem();
                    dot(ui, if problem.is_some() { AMBER } else { LINE });
                    let r = ui.label(RichText::new(&d.name).size(12.5));
                    ui.label(RichText::new(if problem.is_some() { "needs setup" } else { "ready" }).size(11.5).color(TEXT_DIM));
                    if let Some(p) = problem {
                        r.on_hover_text(p);
                    }
                });
            }
            return;
        }
        if let Some(kbps) = stream.lowered_kbps {
            ui.label(RichText::new(format!("⚠ bitrate lowered to {:.1} Mbps while the connection catches up", kbps as f64 / 1000.0)).size(11.0).color(AMBER))
                .on_hover_text(format!("Set: {:.1} Mbps. It comes back by itself once every destination keeps up.", self.cfg.streaming.video_bitrate_kbps as f64 / 1000.0));
        }
        for d in &stream.destinations {
            if d.state == DestState::Off {
                continue;
            }
            let color = match d.state {
                DestState::Live => GREEN,
                DestState::Connecting | DestState::Retrying { .. } => AMBER,
                DestState::Failed => RED,
                DestState::Off => LINE,
            };
            let resp = ui
                .horizontal(|ui| {
                    dot(ui, color);
                    ui.label(RichText::new(&d.name).size(12.5).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if d.state == DestState::Live {
                            ui.label(RichText::new(format!("{:.1} Mbps", d.kbps / 1000.0)).size(11.5).monospace());
                        } else {
                            let mut text = streamer::describe(&d.state);
                            if d.attempts > 1 && matches!(d.state, DestState::Retrying { .. }) {
                                text = format!("{text} (try {})", d.attempts + 1);
                            }
                            ui.label(RichText::new(text).size(11.5).color(color));
                        }
                    });
                })
                .response;
            if d.congested {
                ui.label(RichText::new("   ⚠ connection too slow — dropping frames").size(11.0).color(AMBER));
            }
            if let Some(m) = &d.message {
                if d.state != DestState::Live {
                    ui.label(RichText::new(format!("   {m}")).size(11.0).color(TEXT_DIM));
                }
                resp.on_hover_text(format!("{} — {m}", d.service.label()));
            } else {
                resp.on_hover_text(d.service.label());
            }
        }
    }

    fn graphics_panel(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else { return };
        let list = engine.graphics.list();
        if list.is_empty() {
            return;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("GRAPHICS").color(TEXT_DIM).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Set up").clicked() {
                    self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(Section::Graphics));
                }
            });
        });
        let mut toggle = None;
        let mut edit = None;
        let mut clock_menus = Vec::new();
        let mut ticking = false;
        // Time on screen since going live (or since the recording started).
        let since = engine.stream_started_at().map(|t| (t, "since you went live")).or_else(|| match &engine.status.lock_ok().rec {
            crate::engine::RecState::Recording { since, .. } => Some((crate::media::clock::now() - since.elapsed().as_nanos() as i64, "in this take")),
            _ => None,
        });
        let on_air = match since {
            None => Vec::new(),
            Some((t, _)) => match &self.on_air_cache {
                Some((at, from, v)) if (*from - t).abs() < 1_000_000_000 && at.elapsed() < Duration::from_secs(1) => v.clone(),
                _ => {
                    let v = engine.graphics.on_air(t, crate::media::clock::now());
                    self.on_air_cache = Some((Instant::now(), t, v.clone()));
                    v
                }
            },
        };
        let rotations = engine.rotations.list();
        let mut rotate = None;
        ui.horizontal_wrapped(|ui| {
            for (i, (name, on, problem)) in list.iter().enumerate() {
                let clock = self.engine.as_ref().and_then(|e| e.graphics.clock_text(i));
                let shown = match &clock {
                    Some((t, _)) => {
                        ticking = true;
                        format!("{name}  {t}")
                    }
                    None => name.clone(),
                };
                let over = clock.as_ref().is_some_and(|c| c.1);
                let text = RichText::new(shown).strong().size(12.5).color(if *on { Color32::BLACK } else if over { RED } else { Color32::WHITE });
                let b = egui::Button::new(text).fill(if *on { AMBER } else { RAISED }).min_size(vec2(64.0, 28.0));
                let mut r = ui.add(b);
                let is_timer = self.cfg.graphics.get(i).is_some_and(|g| g.kind == crate::config::GraphicKind::Clock && g.clock.is_timer());
                let shown = on_air.iter().find(|o| o.name == *name).zip(since).map(|(o, (_, when))| {
                    let n = o.showings.len();
                    format!("\nOn screen {n} time{} ({}) {when}", if n == 1 { "" } else { "s" }, crate::engine::fmt_duration(Duration::from_secs_f64(o.seconds())))
                });
                r = r.on_hover_text(match problem {
                    Some(p) => format!("⚠ {p}"),
                    None if is_timer => "Show / hide. Right-click to start, pause or reset it.".into(),
                    None if i < 9 => format!("Show / hide (Alt+{})", i + 1),
                    None => "Show / hide".into(),
                } + &shown.unwrap_or_default());
                if r.clicked() {
                    toggle = Some(i);
                }
                if is_timer {
                    clock_menus.push((r.clone(), i));
                }
                let is_title = self.cfg.graphics.get(i).is_some_and(|g| matches!(g.kind, crate::config::GraphicKind::Title | crate::config::GraphicKind::Ticker));
                if is_title && ui.small_button("✎").on_hover_text("Change the text now").clicked() {
                    edit = Some(i);
                }
                let is_web = self.cfg.graphics.get(i).is_some_and(|g| g.kind == crate::config::GraphicKind::Web);
                if is_web && ui.small_button("⟳").on_hover_text("Load the page again (restarts its animations)").clicked() {
                    if let Some(e) = &self.engine {
                        e.graphics.reload(i);
                    }
                }
            }
            for (i, r) in rotations.iter().enumerate() {
                let Some(cfg) = self.cfg.rotations.iter().find(|c| c.name == r.name) else { continue };
                let text = RichText::new(format!("⟳ {}", r.name)).strong().size(12.5).color(if r.running { Color32::BLACK } else { Color32::WHITE });
                let b = egui::Button::new(text).fill(if r.running { AMBER } else { RAISED }).min_size(vec2(64.0, 28.0));
                let state = match (&r.showing, r.next_in) {
                    _ if !r.running => format!("Stopped. {}", match cfg.start {
                        crate::config::RotationStart::Manual => "Click to start it.".to_string(),
                        s => format!("Starts by itself {}, or click to start it now.", s.label().replace("I'm", "you're")),
                    }),
                    (Some(g), _) => format!("{g} is on now."),
                    (None, Some(d)) => format!("Next one in {}.", crate::engine::fmt_duration(d).trim_start_matches("00:")),
                    (None, None) => "Running.".into(),
                };
                let tip = format!("Takes turns: {}.\nOne every {} for {} s.\n{state}", cfg.graphics.join(", "), minutes(cfg.every_minutes), cfg.show_seconds);
                if ui.add(b).on_hover_text(tip).clicked() {
                    rotate = Some(i);
                }
                if r.running {
                    ticking = true;
                }
            }
        });
        if let Some(i) = rotate {
            self.act(Action::Rotation(control::Target::Index(i), Toggle::Toggle));
        }
        for (r, i) in clock_menus {
            self.clock_menu(&r, i);
        }
        if ticking {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        if let Some(i) = toggle {
            self.toggle_graphic(i, None);
        }
        if let Some(i) = edit {
            let g = &self.cfg.graphics[i];
            self.title_edit = Some((i, g.title.clone(), g.subtitle.clone()));
        }
        ui.add_space(4.0);
        ui.separator();
    }

    fn title_dialog(&mut self, ctx: &egui::Context) {
        let Some((i, mut title, mut subtitle)) = self.title_edit.clone() else { return };
        let ticker = self.cfg.graphics.get(i).is_some_and(|g| g.kind == crate::config::GraphicKind::Ticker);
        let mut done = false;
        let mut apply = false;
        let r = egui::Modal::new(egui::Id::unique("title_edit")).show(ctx, |ui| {
            ui.set_width(360.0);
            if ticker {
                ui.heading("Ticker text");
                ui.add(egui::TextEdit::multiline(&mut title).hint_text("One item per line").desired_width(340.0).desired_rows(4));
            } else {
                ui.heading("Title text");
                ui.add(egui::TextEdit::singleline(&mut title).hint_text("Name").desired_width(340.0));
                ui.add(egui::TextEdit::singleline(&mut subtitle).hint_text("Second line").desired_width(340.0));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let enter = !ticker && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.add(egui::Button::new(RichText::new("Update").color(Color32::WHITE)).fill(ACCENT)).clicked() || enter {
                    apply = true;
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        });
        if apply {
            let what = if ticker { "ticker" } else { "title" };
            self.remember(undo::Kind::Titles, format!("Change the {what} text"));
            if let Some(e) = &self.engine {
                if ticker {
                    e.graphics.set_ticker_text(i, &title);
                } else {
                    e.graphics.set_text(i, &title, &subtitle);
                }
            }
            if let Some(g) = self.cfg.graphics.get_mut(i) {
                g.title = title.clone();
                g.subtitle = subtitle.clone();
                self.cfg_dirty = true;
            }
            // Keep the engine's settings copy in step so it isn't redrawn back.
            if let Some(e) = &self.engine {
                let _ = e.update_config(&self.cfg);
            }
        }
        if done || r.should_close() {
            self.title_edit = None;
        } else {
            self.title_edit = Some((i, title, subtitle));
        }
    }

    fn audio_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("AUDIO").color(TEXT_DIM).size(11.0));
        let Some(engine) = &self.engine else { return };
        let (meters, offline, loud) = {
            let st = engine.status.lock_ok();
            (st.meters.clone(), st.offline.clone(), st.loudness.clone())
        };
        let strips: Vec<(String, String, bool, f32)> = engine.audio.iter().map(|a| (a.name.clone(), a.description(), a.shared.is_on(), a.shared.reduction_db())).collect();
        let automix: Vec<Option<f32>> = engine.audio.iter().map(|a| a.shared.automix_db()).collect();
        let monitoring = !self.cfg.mix.monitor_device.is_empty();
        let ducking = engine.mix.ducking_db();
        if strips.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("No audio inputs.\nAdd your interface in Settings → Audio inputs.").color(TEXT_DIM));
            return;
        }
        let mut gain_changes = Vec::new();
        let mut on_changes = Vec::new();
        let mut solo: Option<String> = None;
        ui.add_space(4.0);
        egui::ScrollArea::horizontal().id_salt("strips").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (i, (name, desc, on, reduction)) in strips.iter().enumerate() {
                    let key = format!("a{i}");
                    ui.vertical(|ui| {
                        ui.set_width(64.0);
                        ui.label(RichText::new(name).strong().size(12.0)).on_hover_text(desc);
                        let m = meters.get(&key).cloned().unwrap_or_default();
                        let hold = self.peak_hold_for(&key, &m.peak_db);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            draw_meter(ui, vec2(36.0, 170.0), &m.peak_db, &m.rms_db, &hold, offline.contains(name), !*on);
                            // Compressor gain reduction.
                            if let Some(ac) = self.cfg.audio.get(i) {
                                if ac.processing.enabled && ac.processing.compressor {
                                    draw_reduction(ui, vec2(5.0, 170.0), *reduction);
                                }
                            }
                        });
                        if let Some(ac) = self.cfg.audio.get_mut(i) {
                            let mut g = ac.gain_db as f32;
                            if ui.add(egui::Slider::new(&mut g, -40.0..=12.0).vertical().show_value(false)).on_hover_text(format!("Level {g:+.1} dB (double-click resets)")).double_clicked() {
                                g = 0.0;
                            }
                            if (g as f64 - ac.gain_db).abs() > 1e-3 {
                                ac.gain_db = g as f64;
                                gain_changes.push((i, ac.gain_db));
                            }
                            ui.label(RichText::new(format!("{:+.1} dB", ac.gain_db)).size(10.5).color(TEXT_DIM));
                            let btn = egui::Button::new(RichText::new("ON").strong().size(11.5).color(if *on { Color32::BLACK } else { TEXT_DIM }))
                                .fill(if *on { GREEN } else { RAISED })
                                .min_size(vec2(56.0, 22.0));
                            if ui.add(btn).on_hover_text("Heard in the mixes").clicked() {
                                on_changes.push((i, !*on));
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let soloed = monitoring && self.cfg.mix.monitor_source.eq_ignore_ascii_case(name);
                                let h = ui.add(egui::Button::selectable(soloed, RichText::new("🎧").size(11.0)).min_size(vec2(24.0, 18.0)));
                                if h.on_hover_text(if monitoring { "Solo in the headphones" } else { "Choose a headphone output in Settings → Mixing & headphones" }).clicked() {
                                    solo = Some(name.clone());
                                }
                                let mut tags = String::new();
                                if ac.processing.enabled {
                                    tags.push_str("FX ");
                                }
                                if !ac.to_recording {
                                    tags.push_str("S");
                                } else if !ac.to_stream {
                                    tags.push_str("R");
                                }
                                if ac.duck && self.cfg.mix.ducking {
                                    tags.push_str(" D");
                                }
                                if ac.automix && self.cfg.mix.automix {
                                    tags.push_str(" A");
                                }
                                if !tags.is_empty() {
                                    ui.label(RichText::new(tags.trim()).size(10.0).color(ACCENT)).on_hover_text("FX: voice processing on · R: recording only · S: stream only · D: ducks under voices · A: in the automatic mic mix");
                                }
                                if let Some(db) = automix.get(i).copied().flatten().filter(|d| *d < -0.5 && self.cfg.mix.automix && *on) {
                                    ui.label(RichText::new(format!("↓{:.0}", -db)).size(10.0).color(ACCENT)).on_hover_text("The automatic mic mixer has this mic down right now (dB), because someone else is talking");
                                }
                                if ac.duck && ducking < -0.5 {
                                    ui.label(RichText::new(format!("↓{:.0}", -ducking)).size(10.0).color(AMBER)).on_hover_text("Turned down under the voices right now (dB)");
                                }
                            });
                        }
                    });
                }
                ui.separator();
                for (key, label_text, l) in [("mix", "REC MIX", &loud.recording), ("stream", "STREAM", &loud.stream)] {
                    ui.vertical(|ui| {
                        ui.set_width(70.0);
                        ui.label(RichText::new(label_text).strong().size(12.0));
                        let m = meters.get(key).cloned().unwrap_or_default();
                        let hold = self.peak_hold_for(key, &m.peak_db);
                        draw_meter(ui, vec2(36.0, 170.0), &m.peak_db, &m.rms_db, &hold, false, false);
                        let fmt = |v: f32| if v < -99.0 { "—".to_string() } else { format!("{v:.0}") };
                        ui.label(RichText::new(format!("{} LUFS", fmt(l.short_term))).monospace().size(11.0))
                            .on_hover_text(format!("Short-term loudness (3 s). Momentary (0.4 s): {} LUFS", fmt(l.momentary)));
                        ui.label(RichText::new(format!("I {}", fmt(l.integrated))).monospace().size(10.5).color(TEXT_DIM))
                            .on_hover_text("Integrated loudness since the take / stream started");
                        if l.limiting > 0.5 {
                            ui.label(RichText::new(format!("lim −{:.0}", l.limiting)).size(10.5).color(AMBER));
                        }
                        if key == "stream" {
                            if let Some(g) = loud.stream_gain_db {
                                ui.label(RichText::new(format!("lev {g:+.1}")).size(10.5).color(ACCENT)).on_hover_text("What stream levelling is adding");
                            }
                        }
                    });
                }
            });
        });
        if monitoring {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🎧").size(12.0));
                let src = if self.cfg.mix.monitor_source == "stream" {
                    "stream mix".to_string()
                } else if self.cfg.mix.monitor_source == "program" || self.cfg.mix.monitor_source.is_empty() {
                    "recording mix".to_string()
                } else {
                    format!("{} solo", self.cfg.mix.monitor_source)
                };
                ui.label(RichText::new(src).size(11.5).color(TEXT_DIM));
                let mut v = self.cfg.mix.monitor_volume_db;
                if ui.add(egui::Slider::new(&mut v, -40.0..=12.0).show_value(false)).on_hover_text("Headphone volume").changed() {
                    self.cfg.mix.monitor_volume_db = v;
                    self.cfg_dirty = true;
                    if let Some(e) = &self.engine {
                        e.apply_monitor(&self.cfg.mix);
                    }
                }
            });
        }
        let engine = self.engine.as_ref().unwrap();
        for (i, db) in &gain_changes {
            engine.set_gain(*i, *db);
        }
        if !gain_changes.is_empty() {
            self.cfg_dirty = true;
        }
        for (i, on) in on_changes {
            self.set_input_on(i, on);
        }
        if let Some(name) = solo {
            let src = &mut self.cfg.mix.monitor_source;
            *src = if src.eq_ignore_ascii_case(&name) { "program".into() } else { name };
            self.cfg_dirty = true;
            if let Some(e) = &self.engine {
                e.apply_monitor(&self.cfg.mix);
            }
        }
    }

    fn peak_hold_for(&mut self, key: &str, peaks: &[f32]) -> Vec<f32> {
        let now = Instant::now();
        let entry = self.peak_hold.entry(key.to_string()).or_insert(PeakHold { db: vec![-100.0; peaks.len()], at: now });
        let dt = now.duration_since(entry.at).as_secs_f32();
        entry.at = now;
        entry.db.resize(peaks.len(), -100.0);
        for (h, &p) in entry.db.iter_mut().zip(peaks) {
            *h = (*h - 12.0 * dt).max(p);
        }
        entry.db.clone()
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else { return };
        let log: Vec<_> = engine.status.lock_ok().log.iter().cloned().collect();
        egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
            for l in log {
                let color = match l.level {
                    LogLevel::Info => TEXT_DIM,
                    LogLevel::Warning => AMBER,
                    LogLevel::Error => RED,
                };
                ui.label(RichText::new(format!("{}  {}", l.time, l.text)).color(color).monospace().size(11.5));
            }
        });
    }

    // ---- health ----------------------------------------------------------------------

    fn health_window(&mut self, ctx: &egui::Context) {
        if !self.health_open {
            return;
        }
        let Some(engine) = &self.engine else { return };
        let (h, disk, backup) = {
            let st = engine.status.lock_ok();
            (st.health.clone(), st.disk.clone(), st.backup_disk.clone())
        };
        // Auto-framing and background removal, per source.
        let extras: Vec<String> = engine
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut parts = Vec::new();
                if let Some(f) = s.input.framing() {
                    match f.problem.lock_ok().clone() {
                        Some(_) => parts.push("auto-framing off".to_string()),
                        None => parts.push(format!("following {}", match f.people.load(std::sync::atomic::Ordering::Relaxed) {
                            0 => "no one".to_string(),
                            1 => "1 person".to_string(),
                            n => format!("{n} people"),
                        })),
                    }
                }
                if let Some(rate) = engine.cutout_rate(i) {
                    parts.push(format!("background removal {rate:.0}/s"));
                }
                parts.join(" · ")
            })
            .collect();
        let mut open = true;
        egui::Window::new("Health").open(&mut open).default_width(520.0).collapsible(false).show(ctx, |ui| {
            let ok = |ui: &mut egui::Ui, good: bool, text: String| {
                ui.label(RichText::new(text).color(if good { Color32::WHITE } else { AMBER }));
            };
            ui.label(RichText::new("DRIVES").color(TEXT_DIM).size(11.0));
            match &disk {
                Some(d) => ok(ui, d.seconds_left().is_none_or(|s| s > 1800.0), format!("Recording: {} of {} ({})", d.describe(), crate::util::human_size(d.total), if d.measured { "rate measured" } else { "rate estimated" })),
                None => ok(ui, false, "Recording drive: unknown".into()),
            }
            if let Some(b) = &backup {
                ok(ui, b.free > crate::engine::health::MIN_START_BYTES, format!("Backup: {}", b.describe()));
            }
            ui.add_space(8.0);
            ui.label(RichText::new("SOURCES").color(TEXT_DIM).size(11.0));
            egui::Grid::new("h_src").num_columns(4).striped(true).show(ui, |ui| {
                for (i, s) in h.sources.iter().enumerate() {
                    ui.label(&s.name);
                    let good = s.alive && (s.expected_fps == 0 || s.fps >= s.expected_fps as f32 * 0.8);
                    ok(ui, good, if s.alive { format!("{:.1} fps", s.fps) } else { "no signal".into() });
                    ui.label(RichText::new(format!("set to {}", s.expected_fps)).color(TEXT_DIM));
                    ui.label(RichText::new(extras.get(i).cloned().unwrap_or_default()).color(TEXT_DIM));
                    ui.end_row();
                }
            });
            if !h.files.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("FILES BEING RECORDED").color(TEXT_DIM).size(11.0));
                egui::Grid::new("h_files").num_columns(5).striped(true).show(ui, |ui| {
                    for t in ["File", "Frames", "Repeated", "Dropped", "Waiting"] {
                        ui.label(RichText::new(t).color(TEXT_DIM).size(11.5));
                    }
                    ui.end_row();
                    for f in &h.files {
                        ui.label(&f.label);
                        let r = ui.label(RichText::new(if f.failed { "failed".into() } else { f.encoded.to_string() }).color(if f.failed { AMBER } else { Color32::WHITE }));
                        if f.resized > 0 {
                            r.on_hover_text(format!("{} frames were rescaled because the source changed size", f.resized));
                        }
                        ok(ui, f.repeated < 30, f.repeated.to_string());
                        ok(ui, f.dropped < 30, f.dropped.to_string());
                        ok(ui, f.backlog <= 30, f.backlog.to_string());
                        ui.end_row();
                    }
                });
                ui.label(RichText::new("Repeated: the source skipped a frame (filled in). Dropped: extra frames from a fast source. Waiting: frames queued for the encoder (should stay small).").size(11.0).color(TEXT_DIM));
            }
            ui.add_space(8.0);
            ui.label(RichText::new("AUDIO").color(TEXT_DIM).size(11.0));
            egui::Grid::new("h_audio").num_columns(3).striped(true).show(ui, |ui| {
                for a in &h.audio {
                    ui.label(&a.name);
                    ok(ui, a.alive, if a.alive { "OK".into() } else { "no signal".into() });
                    ok(ui, a.resyncs == 0, format!("{} re-syncs", a.resyncs));
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ok(ui, h.compositor_skips == 0, format!("Program frames skipped (computer too busy): {}", h.compositor_skips));
        });
        if !open {
            self.health_open = false;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    // ---- pre-show check -----------------------------------------------------------------

    fn start_precheck(&mut self, then: Option<Then>) {
        let Some(e) = &self.engine else {
            self.toast("The engine isn't running".into(), true);
            return;
        };
        let items = e.local_checks();
        let dests: Vec<_> = self.cfg.streaming.destinations.iter().filter(|d| d.enabled).cloned().collect();
        let check_streams = then != Some(Then::Record) && !dests.is_empty() && !e.is_streaming();
        let waiting = if check_streams { dests.len() } else { 0 };
        let rx = check_streams.then(|| precheck::check_destinations(dests));
        self.precheck = Some(Precheck { items, rx, waiting, then });
    }

    fn precheck_dialog(&mut self, ctx: &egui::Context) {
        let Some(pc) = self.precheck.as_mut() else { return };
        if let Some(rx) = &pc.rx {
            while let Ok(item) = rx.try_recv() {
                pc.items.push(item);
                pc.waiting = pc.waiting.saturating_sub(1);
            }
            if pc.waiting == 0 {
                pc.rx = None;
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let worst = pc.items.iter().map(|i| i.level).max().unwrap_or(Level::Ok);
        let done = pc.waiting == 0;
        let then = pc.then;
        // Nothing to worry about: carry on straight away.
        if done && worst == Level::Ok && then.is_some() {
            self.precheck = None;
            match then {
                Some(Then::Record) => self.start_recording_now(),
                Some(Then::Live) => self.start_stream_now(),
                None => {}
            }
            return;
        }
        let mut close = false;
        let mut proceed = false;
        let mut again = false;
        let mut open_sync = false;
        let items = pc.items.clone();
        let waiting = pc.waiting;
        egui::Modal::new(egui::Id::unique("precheck")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(match then {
                Some(Then::Record) => "Before recording",
                Some(Then::Live) => "Before going live",
                None => "Pre-show check",
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("checks").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                    for it in &items {
                        let (sym, color) = match it.level {
                            Level::Ok => ("✔", GREEN),
                            Level::Warn => ("⚠", AMBER),
                            Level::Fail => ("✖", RED),
                        };
                        ui.label(RichText::new(sym).color(color).strong());
                        ui.label(RichText::new(&it.what).strong());
                        ui.add(egui::Label::new(RichText::new(&it.detail).color(if it.level == Level::Ok { TEXT_DIM } else { color })).wrap());
                        ui.end_row();
                    }
                    if waiting > 0 {
                        ui.spinner();
                        ui.label("Stream servers");
                        ui.label(RichText::new(format!("contacting {waiting}…")).color(TEXT_DIM));
                        ui.end_row();
                    }
                });
                if items.iter().any(|i| i.what == precheck::NDI && i.level != Level::Ok) {
                    ui.add_space(6.0);
                    crate::settings::ndi_offer(ui);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if then.is_some() {
                    let label = if worst == Level::Fail { "Start anyway" } else { "Start" };
                    if ui.add_enabled(done, egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(if worst == Level::Fail { RED } else { ACCENT })).clicked() {
                        proceed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                } else if ui.button("Close").clicked() {
                    close = true;
                }
                if ui.button("Check again").clicked() {
                    again = true;
                }
                if then.is_none() && ui.button("Measure lip sync…").on_hover_text("Clap once: Cutaway works out the audio and video delays").clicked() {
                    close = true;
                    open_sync = true;
                }
            });
        });
        if again {
            self.precheck = None;
            self.start_precheck(then);
        } else if proceed {
            self.precheck = None;
            match then {
                Some(Then::Record) => self.start_recording_now(),
                Some(Then::Live) => self.start_stream_now(),
                None => {}
            }
        } else if close {
            self.precheck = None;
        }
        if open_sync {
            self.sync = Some(SyncWizard::default());
        }
    }

    // ---- lip sync ---------------------------------------------------------------------

    fn sync_dialog(&mut self, ctx: &egui::Context) {
        let Some(w) = self.sync.as_mut() else { return };
        let Some(engine) = &self.engine else {
            self.sync = None;
            return;
        };
        let result = w.job.as_ref().and_then(|j| j.result.lock_ok().clone());
        if let Some(r) = &result {
            let n = r.video.len() + r.audio.len();
            if w.apply.len() != n {
                w.apply = r.video.iter().chain(&r.audio).map(|i| i.event.is_some() && i.suggested_ms != i.current_ms).collect();
            }
        }
        let mut start = false;
        let mut close = false;
        let mut apply: Option<(Vec<(String, u32)>, Vec<(String, u32)>)> = None;
        let busy = engine.is_recording() || engine.is_streaming();
        egui::Modal::new(egui::Id::unique("lipsync")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Lip sync");
            match (&w.job, &result) {
                (None, _) => {
                    ui.label("Cutaway finds one clap in every camera and every input, and sets the delays that line them up.");
                    ui.add_space(6.0);
                    ui.label("1. Stand where every camera can see your hands (a clapperboard works too).");
                    ui.label("2. Press Start, then clap once, sharply, near a microphone.");
                    ui.label(RichText::new("Cutaway listens for 6 seconds. Keep other sounds and movement down.").color(TEXT_DIM));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Start").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                            start = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                (Some(job), None) => {
                    let p = job.progress();
                    ui.add_space(8.0);
                    ui.label(RichText::new(if p < 0.12 { "Get ready…" } else { "Clap now!" }).size(28.0).strong().color(if p < 0.12 { TEXT_DIM } else { AMBER }));
                    ui.add_space(6.0);
                    ui.add(egui::ProgressBar::new(p).desired_width(520.0));
                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                }
                (Some(_), Some(r)) => {
                    if let Some(p) = &r.problem {
                        ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
                        ui.add_space(4.0);
                    }
                    egui::Grid::new("sync_results").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                        for t in ["", "", "Clap", "Delay now", "Suggested"] {
                            ui.label(RichText::new(t).color(TEXT_DIM).size(11.5));
                        }
                        ui.end_row();
                        for (k, (kind, item)) in r.video.iter().map(|i| ("Video", i)).chain(r.audio.iter().map(|i| ("Audio", i))).enumerate() {
                            let can = item.event.is_some() && item.suggested_ms != item.current_ms;
                            ui.add_enabled(can, egui::Checkbox::without_text(&mut w.apply[k]));
                            ui.label(format!("{kind}: {}", item.name));
                            if item.event.is_some() {
                                ui.label(RichText::new("✔ found").color(GREEN));
                            } else {
                                ui.label(RichText::new("not found").color(TEXT_DIM));
                            }
                            ui.label(format!("{} ms", item.current_ms));
                            let color = if item.suggested_ms != item.current_ms && item.event.is_some() { Color32::WHITE } else { TEXT_DIM };
                            ui.label(RichText::new(format!("{} ms", item.suggested_ms)).color(color));
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("The latest camera or input sets the time; everything else is held back to match it. Sources and inputs restart for a moment when delays change.").size(11.5).color(TEXT_DIM));
                    if busy {
                        ui.label(RichText::new("Stop recording and streaming to apply.").color(AMBER));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let any = w.apply.iter().any(|a| *a);
                        if ui.add_enabled(any && !busy, egui::Button::new(RichText::new("Apply").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                            let pick = |items: &[crate::engine::lipsync::SyncItem], offset: usize| -> Vec<(String, u32)> {
                                items.iter().enumerate().filter(|(k, _)| w.apply[offset + k]).map(|(_, i)| (i.name.clone(), i.suggested_ms)).collect()
                            };
                            apply = Some((pick(&r.video, 0), pick(&r.audio, r.video.len())));
                        }
                        if ui.button("Try again").clicked() {
                            start = true;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if start {
            if let Some(w) = self.sync.as_mut() {
                w.job = Some(engine.measure_sync(6.0));
                w.apply.clear();
            }
        }
        if let Some((video, audio)) = apply {
            let mut cfg = self.cfg.clone();
            cfg.scenes = self.scenes.clone();
            for (name, ms) in &video {
                if let Some(s) = cfg.sources.iter_mut().find(|s| &s.name == name) {
                    s.delay_ms = *ms;
                }
            }
            for (name, ms) in &audio {
                if let Some(a) = cfg.audio.iter_mut().find(|a| &a.name == name) {
                    a.delay_ms = *ms;
                }
            }
            match self.apply_settings(cfg, ctx) {
                Ok(()) => self.toast(format!("Lip sync: {} delays set", video.len() + audio.len()), false),
                Err(e) => self.toast(e, true),
            }
            close = true;
        }
        if close {
            self.sync = None;
        }
    }

    // ---- second screen ------------------------------------------------------------------

    fn display_window(&mut self, ctx: &egui::Context) {
        if self.cfg.display.output == DisplayOutput::Off || self.engine.is_none() {
            self.display_was_open = false;
            return;
        }
        if self.display_was_open && self.display_screen_applied != self.cfg.display.screen {
            // Another screen chosen: a new window there.
            self.display_was_open = false;
            self.display_generation = self.display_generation.wrapping_add(1);
        }
        let mut position = None;
        if !self.display_was_open {
            // Just opened: on its screen, with the keyboard, showing its
            // buttons a moment.
            self.display_was_open = true;
            self.display_screen_applied = self.cfg.display.screen.clone();
            let place = output_placement(ctx, &self.cfg.display);
            self.display_monitor = place.monitor;
            self.display_open_full = place.fullscreen;
            self.display_fullscreen = place.fullscreen;
            position = place.position;
            self.display_focus_frames = 3;
            self.display_controls_until = Some(Instant::now() + Duration::from_secs(3));
        }
        let viewport = egui::ViewportId::from_hash_of(("cutaway_output", self.display_generation));
        let full_now = self.display_fullscreen;
        let controls_until = self.display_controls_until;
        let mut moved = false;
        let mut actual_full = None;
        let multiview = self.cfg.display.output == DisplayOutput::Multiview;
        let prompter = (self.cfg.display.output == DisplayOutput::Prompter).then(|| (self.cfg.prompter.clone(), self.prompter.script.clone(), self.prompter.pos));
        let mut builder = egui::ViewportBuilder::default().with_title("Cutaway — Output").with_icon(app_icon()).with_inner_size([960.0, 540.0]).with_fullscreen(self.display_open_full);
        // (How it opens stays the same while it's open: after that, full
        // screen goes on and off by command, following the window's real
        // state.)
        if let (true, Some(m)) = (self.display_open_full, self.display_monitor) {
            builder = builder.with_monitor(m);
        } else if let Some(p) = position {
            builder = builder.with_position(p);
        }
        if self.self_test.is_some() {
            // Screenshots: beside the main window rather than under it.
            builder = builder.with_position([1500.0, 60.0]);
        }
        let program = self.textures.get("program").map(|t| t.1.id());
        let preview = self.textures.get("previewbus").map(|t| t.1.id());
        let engine = self.engine.as_ref().unwrap();
        let tiles: Vec<(String, Option<egui::TextureId>, f32, bool, bool)> = {
            let live = engine.switcher.live().sources();
            let pv = engine.switcher.preview().map(|p| p.sources()).unwrap_or_default();
            engine
                .sources
                .iter()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), self.textures.get(&format!("src{i}")).map(|t| t.1.id()), s.size.0 as f32 / s.size.1 as f32, live.contains(&i), pv.contains(&i)))
                .collect()
        };
        let canvas_aspect = engine.info.canvas.0 as f32 / engine.info.canvas.1 as f32;
        let live_name = engine.switcher.live().name;
        let preview_name = engine.switcher.preview().map(|p| p.name);
        let recording = engine.is_recording();
        let clocks = self.cfg.display.clocks.then(|| {
            let st = self.rundown.status(&self.cfg.rundown.segments);
            let segment = st.index.map(|_| (st.name.clone(), st.elapsed, st.planned));
            let rec = match &engine.status.lock_ok().rec {
                RecState::Recording { since, .. } => Some(since.elapsed()),
                _ => None,
            };
            (segment, rec)
        });
        let mut close = false;
        let mut toggle_full = false;
        ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
            let ctx = ui.ctx().clone();
            // Full screen as the window really is (macOS can also leave it
            // with its own controls).
            actual_full = ctx.input(|i| i.viewport().fullscreen);
            let full_now = actual_full.unwrap_or(full_now);
            moved = ctx.input(|i| i.pointer.is_moving() || i.pointer.any_pressed());
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
                let area = ui.max_rect();
                let resp = ui.interact(area, ui.scope_id().with("out"), Sense::click());
                let painter = ui.painter_at(area);
                let fit = |r: Rect, aspect: f32| {
                    let (w, h) = if r.width() / r.height() > aspect { (r.height() * aspect, r.height()) } else { (r.width(), r.width() / aspect) };
                    Rect::from_center_size(r.center(), vec2(w, h))
                };
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                if let Some((pcfg, script, pos)) = &prompter {
                    production::draw_prompter(ui, area, pcfg, script, *pos);
                } else if !multiview {
                    if let Some(t) = program {
                        painter.image(t, fit(area, canvas_aspect), uv, Color32::WHITE);
                    }
                } else {
                    // Preview and program on top, sources below, with tally.
                    let top = Rect::from_min_size(area.min, vec2(area.width(), area.height() * 0.55));
                    let halves = [Rect::from_min_max(top.min, pos2(top.center().x - 4.0, top.bottom())), Rect::from_min_max(pos2(top.center().x + 4.0, top.top()), top.max)];
                    for (i, (r, tex, name, color)) in
                        [(halves[0], preview, preview_name.clone().unwrap_or_else(|| "PREVIEW".into()), GREEN), (halves[1], program, live_name.clone(), RED)].into_iter().enumerate()
                    {
                        let rr = fit(r.shrink(6.0), canvas_aspect);
                        painter.rect_filled(rr, 0.0, Color32::from_gray(12));
                        if let Some(t) = tex {
                            if i == 1 || preview_name.is_some() {
                                painter.image(t, rr, uv, Color32::WHITE);
                            }
                        }
                        painter.rect_stroke(rr, 0.0, Stroke::new(3.0, color), StrokeKind::Outside);
                        let label = if i == 1 && recording { format!("● {name}") } else { name };
                        painter.text(rr.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, FontId::proportional(18.0), Color32::WHITE);
                    }
                    let n = tiles.len().max(1);
                    let cols = (n as f32).sqrt().ceil().max(n.min(4) as f32) as usize;
                    let rows = n.div_ceil(cols);
                    let bottom = Rect::from_min_max(pos2(area.left(), top.bottom() + 8.0), area.max);
                    let cw = bottom.width() / cols as f32;
                    let chh = bottom.height() / rows as f32;
                    for (i, (name, tex, aspect, on_air, pv)) in tiles.iter().enumerate() {
                        let cell = Rect::from_min_size(bottom.min + vec2((i % cols) as f32 * cw, (i / cols) as f32 * chh), vec2(cw, chh)).shrink(5.0);
                        let rr = fit(cell, *aspect);
                        painter.rect_filled(rr, 0.0, Color32::from_gray(12));
                        if let Some(t) = tex {
                            painter.image(*t, rr, uv, Color32::WHITE);
                        }
                        let color = if *on_air { RED } else if *pv { GREEN } else { Color32::from_gray(70) };
                        painter.rect_stroke(rr, 0.0, Stroke::new(if *on_air || *pv { 3.0 } else { 1.0 }, color), StrokeKind::Outside);
                        painter.text(rr.left_bottom() + vec2(6.0, -6.0), Align2::LEFT_BOTTOM, format!("{} {}", i + 1, name), FontId::proportional(14.0), Color32::WHITE);
                    }
                }
                if let Some((segment, rec)) = &clocks {
                    App::draw_display_clocks(&painter, area, segment.clone(), *rec);
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
                if resp.double_clicked() {
                    toggle_full = true;
                }
                // The corner buttons, while the mouse is moving over the
                // window (or over them), like a video player's.
                let button = Rect::from_min_size(area.right_top() + vec2(-60.0, 16.0), vec2(44.0, 44.0));
                let close_button = button.translate(vec2(-54.0, 0.0));
                let b = ui.interact(button, ui.scope_id().with("full screen button"), Sense::click());
                let c = ui.interact(close_button, ui.scope_id().with("close button"), Sense::click());
                let showing = b.hovered() || c.hovered() || controls_until.is_some_and(|t| Instant::now() < t);
                if showing {
                    for (r, hovered) in [(button, b.hovered()), (close_button, c.hovered())] {
                        painter.rect_filled(r, 8.0, if hovered { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(150) });
                    }
                    full_screen_icon(&painter, button.shrink(12.0), !full_now, Color32::WHITE);
                    let x = close_button.shrink(14.0);
                    let s = Stroke::new(2.5, Color32::WHITE);
                    painter.line_segment([x.left_top(), x.right_bottom()], s);
                    painter.line_segment([x.right_top(), x.left_bottom()], s);
                    if b.clicked() {
                        toggle_full = true;
                    }
                    if c.clicked() {
                        close = true;
                    }
                    b.on_hover_text(if full_now { "Leave full screen (Esc)" } else { "Full screen (F, or double-click)" });
                    c.on_hover_text(if full_now { "Close this screen (Esc twice)" } else { "Close this screen (Esc)" });
                } else {
                    resp.on_hover_text(if full_now { "Esc: leave full screen (twice: close). F or double-click: full screen." } else { "Double-click (or F): full screen. Esc: close." });
                }
            });
            if ctx.input(|i| i.key_pressed(Key::F)) {
                toggle_full = true;
            }
            // Esc always gets you out: first out of full screen, then
            // closing the window. Cmd/Ctrl+W closes it straight away.
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                if full_now {
                    toggle_full = true;
                } else {
                    close = true;
                }
            }
            if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::W)) {
                close = true;
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        if let Some(f) = actual_full {
            self.display_fullscreen = f;
        }
        if moved {
            self.display_controls_until = Some(Instant::now() + Duration::from_secs(2));
        }
        if let Some(t) = self.display_controls_until {
            if Instant::now() < t {
                // Hide the button on time even if nothing else moves.
                ctx.request_repaint_after_for(t - Instant::now(), viewport);
            } else {
                self.display_controls_until = None;
                ctx.request_repaint_of(viewport);
            }
        }
        if toggle_full {
            self.display_fullscreen = !self.display_fullscreen;
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Fullscreen(self.display_fullscreen));
            if self.display_fullscreen {
                self.display_focus_frames = 3;
            }
        }
        if self.display_focus_frames > 0 {
            // Keys go to the focused window: without this, Esc pressed on a
            // freshly opened full-screen output went to the main window.
            self.display_focus_frames -= 1;
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
            ctx.request_repaint_of(viewport);
        }
        if close {
            self.cfg.display.output = DisplayOutput::Off;
            self.cfg_dirty = true;
            // The main window takes the keyboard back.
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }

    // ---- settings ---------------------------------------------------------------

    fn open_settings(&mut self) {
        if self.settings.is_none() {
            let mut cfg = self.cfg.clone();
            cfg.scenes = self.scenes.clone();
            self.settings = Some(SettingsUi::new(cfg));
        }
    }

    fn settings_context(&self) -> SettingsContext {
        let mut facts = Vec::new();
        let (mut recording, mut streaming, mut log, mut levels, mut vcam_on) = (false, false, Vec::new(), Vec::new(), false);
        if let Some(e) = &self.engine {
            recording = e.is_recording();
            streaming = e.is_streaming();
            vcam_on = e.virtual_camera_on();
            levels = e.audio.iter().map(|a| a.shared.levels_db()).collect();
            let st = e.status.lock_ok();
            log = st.log.iter().cloned().collect();
            facts.push(("Recording encoder".into(), e.encoder_description()));
            if !st.stream.encoder.is_empty() {
                facts.push(("Stream encoder".into(), st.stream.encoder.clone()));
            }
            facts.push(("Compositor".into(), e.info.compositor.clone()));
            facts.push(("Program".into(), format!("{}×{} @ {} fps", e.info.canvas.0, e.info.canvas.1, e.info.fps)));
            for s in &e.sources {
                facts.push((format!("Source: {}", s.name), s.description()));
            }
            for a in &e.audio {
                facts.push((format!("Audio: {}", a.name), a.description()));
            }
            facts.push(("NDI".into(), if crate::engine::ndi::available() { "installed".into() } else { "not installed".into() }));
        } else if let Some(err) = &self.engine_error {
            facts.push(("Engine".into(), format!("not running: {err}")));
        }
        let (midi_last, midi_connected) = match &self.midi {
            Some(m) => {
                let last = m.shared.last.lock_ok().filter(|(_, t)| t.elapsed() < Duration::from_secs(2)).map(|(msg, _)| msg.describe());
                (last, m.shared.connected.lock_ok().clone())
            }
            None => (None, Vec::new()),
        };
        SettingsContext {
            recording,
            streaming,
            engine_facts: facts,
            log,
            config_path: self.save_path.clone(),
            profiles_dir: self.save_path.as_ref().map(|p| crate::config::profiles_dir(p)),
            midi_last,
            midi_connected,
            remote_port: self.remote.as_ref().map(|r| r.port),
            osc_port: self.osc.as_ref().map(|o| o.port),
            captions_status: self.engine.as_ref().and_then(|e| e.captions()).map(|c| c.status()),
            vcam_on,
            input_levels: levels,
            stinger: self.engine.as_ref().map(|e| e.stinger.info()),
            guest_reach: crate::engine::guests::hub().reach(),
            guest_base: crate::engine::guests::hub().base_url(),
            guest_status: crate::engine::guests::hub().guests(),
            ndi_watchers: self.engine.as_ref().and_then(|e| e.ndi_watchers()),
            on_air: self.engine.as_ref().map(|e| e.switcher.live().layers.iter().filter_map(|l| self.cfg.sources.get(l.source).map(|s| s.uid)).collect()).unwrap_or_default(),
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let info = self.settings_context();
        let Some(win) = self.settings.as_mut() else { return };
        match win.show(ctx, &info) {
            SettingsAction::None => {}
            SettingsAction::Close => self.settings = None,
            SettingsAction::VirtualCamera(on) => self.set_virtual_camera(on),
            SettingsAction::MeasureSync => {
                self.settings = None;
                self.sync = Some(SyncWizard::default());
            }
            SettingsAction::Apply { cfg, close } => {
                let before = self.snapshot(undo::Kind::Settings, "Settings change");
                let result = self.apply_settings(cfg, ctx);
                let ok = result.is_ok();
                if ok && before.settings_differ(&self.cfg) {
                    self.history.record(before);
                }
                if let Some(win) = self.settings.as_mut() {
                    win.applied(result, &self.cfg);
                }
                if ok && close {
                    self.settings = None;
                }
            }
        }
        // Live levels in Settings → Switching.
        if self.settings.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// Apply new settings: live where possible, otherwise rebuild the engine.
    fn apply_settings(&mut self, mut new: Config, ctx: &egui::Context) -> Result<(), String> {
        new.validate().map_err(|e| e.to_string())?;
        new.assign_uids();
        // Only the program's size and rate rebuild everything; sources and
        // inputs change live, one by one.
        let restart = self.cfg.needs_restart(&new) || self.engine.is_none();
        let lineup = !restart && self.cfg.lineup_changed(&new);
        let sources_changed = new.sources.len() != self.cfg.sources.len() || new.sources.iter().zip(&self.cfg.sources).any(|(a, b)| a.name != b.name || a.uid != b.uid);
        let control_changed = new.remote != self.cfg.remote || new.midi != self.cfg.midi || new.osc != self.cfg.osc;
        if restart && self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) {
            return Err("Stop recording and streaming first: the program's size or frame rate changed".into());
        }
        // Scenes (the user's own, kept if the settings had none) follow the
        // sources that moved or went.
        if new.scenes.is_empty() {
            new.scenes = self.cfg.scenes.clone();
        }
        let old_uids: Vec<u64> = self.cfg.sources.iter().map(|s| s.uid).collect();
        let new_uids: Vec<u64> = new.sources.iter().map(|s| s.uid).collect();
        if old_uids != new_uids {
            let new_of = |old: usize| old_uids.get(old).and_then(|u| new_uids.iter().position(|n| n == u));
            for sc in &mut new.scenes {
                sc.remap_sources(&new_of);
            }
        }
        let mut notes = Vec::new();
        if !restart {
            if let Some(e) = self.engine.as_mut() {
                e.check_config(&new).map_err(|e| format!("{e:#}"))?;
                if lineup {
                    notes = e.apply_lineup(&new);
                }
                e.update_config(&new).map_err(|e| format!("{e:#}"))?;
            }
        }
        if lineup {
            // Pictures and meters are by number.
            self.textures.clear();
            self.peak_hold.clear();
        }
        let vcam_was = self.engine.as_ref().is_some_and(|e| e.virtual_camera_on()) || self.starting.as_ref().is_some_and(|s| s.vcam);
        self.cfg = new;
        self.scenes = if self.cfg.scenes.is_empty() { self.cfg.effective_scenes() } else { self.cfg.scenes.clone() };
        if sources_changed {
            self.reconcile_scenes();
            if let Some(e) = &self.engine {
                e.set_scenes(&self.cfg.scenes);
            }
        }
        ctx.set_zoom_factor(self.cfg.ui.scale.clamp(0.5, 3.0));
        if restart {
            // Starts in the background; the monitors show how it's going.
            self.vcam_wanted = vcam_was;
            self.start_engine();
        }
        if control_changed {
            self.apply_control_settings();
        }
        self.save_config();
        if let Some(e) = &self.engine {
            let mut st = e.status.lock_ok();
            for n in &notes {
                st.push(LogLevel::Info, n.clone());
            }
        }
        if let Some(first) = notes.first() {
            let more = if notes.len() > 1 { format!(" (and {} more in the log)", notes.len() - 1) } else { String::new() };
            self.toast(format!("{first}{more}"), false);
        }
        Ok(())
    }

    /// Keep user scenes valid after sources were added/removed/renamed.
    fn reconcile_scenes(&mut self) {
        let names: Vec<String> = self.cfg.sources.iter().map(|s| s.name.clone()).collect();
        if self.cfg.scenes.is_empty() {
            self.scenes = self.cfg.effective_scenes();
            return;
        }
        let presets = crate::scene::presets(&names);
        let mut scenes: Vec<Scene> = Vec::new();
        for mut s in self.cfg.scenes.clone() {
            s.retain_sources(names.len());
            if !s.layers.is_empty() {
                scenes.push(s);
            }
        }
        for (i, p) in presets.iter().take(names.len()).enumerate() {
            let has = scenes.iter().any(|s| s.layers.len() == 1 && s.layers[0].source == i);
            if !has {
                scenes.insert(i.min(scenes.len()), p.clone());
            }
        }
        self.scenes = scenes;
        self.cfg.scenes = self.scenes.clone();
    }
}

impl App {
    /// While NDI's installer is going, look for NDI now and then; when it
    /// turns up, start what was waiting for it.
    fn tick_ndi(&mut self, ctx: &egui::Context) {
        if crate::net::ndi_install::waiting() {
            let _ = crate::engine::ndi::available();
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if crate::engine::ndi::take_arrived() {
            crate::net::ndi_install::reset();
            if let Some(e) = &self.engine {
                e.ndi_arrived();
            }
            self.toast("NDI is installed and ready".into(), false);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Closing the window (or Cmd+Q / Alt+F4) mid-show asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_ok && self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Quit);
        }
        self.tick_starting(&ctx);
        self.handle_requests();
        self.run_queued();
        if let Some(d) = self.next_queued() {
            ctx.request_repaint_after(d);
        }
        self.tick_comments(&ctx);
        self.tick_uploads();
        self.tick_ndi(&ctx);
        if self.settle_at.is_some_and(|t| Instant::now() >= t) {
            self.settle_at = None;
            if let Ok(me) = crate::net::self_update::Install::here() {
                crate::net::self_update::settled(&me, env!("CARGO_PKG_VERSION"));
            }
        }
        self.tick_prompter(&ctx);
        self.handle_keys(&ctx);
        self.poll_status();
        self.run_self_test(&ctx);
        self.publish_state();
        let busy = self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming());
        if !busy && crate::platform::permissions_changed() {
            self.start_engine();
        }
        if self.remote.is_some() || self.midi.is_some() || self.osc.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        egui::Panel::top("header").exact_size(54.0).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6))).show(ui, |ui| self.header(ui));
        egui::Panel::bottom("bank").exact_size(104.0).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6))).show(ui, |ui| self.scene_bank(ui));
        if self.log_open {
            egui::Panel::bottom("log").resizable(true).default_size(150.0).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(8))).show(ui, |ui| self.log_panel(ui));
        }
        egui::Panel::right("audio").resizable(true).default_size(400.0).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| {
            ui.set_min_width(280.0);
            // Leave the audio strips at least half the height.
            let max_top = (ui.available_height() - 260.0).max(200.0);
            egui::ScrollArea::vertical().id_salt("right_top").max_height(max_top).show(ui, |ui| {
                self.stream_panel(ui);
                ui.add_space(4.0);
                ui.separator();
                self.uploads_panel(ui);
                self.rundown_panel(ui);
                self.guests_panel(ui);
                self.ptz_panel(ui);
                self.comments_panel(ui);
                self.prompter_panel(ui);
                self.macros_panel(ui);
                self.graphics_panel(ui);
                self.sound_panel(ui);
            });
            self.audio_panel(ui);
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(14))).show(ui, |ui| self.monitors(ui));

        self.display_window(&ctx);
        self.health_window(&ctx);
        if self.settings.is_some() {
            self.settings_window(&ctx);
        }
        self.confirm_dialog(&ctx);
        self.title_dialog(&ctx);
        self.precheck_dialog(&ctx);
        self.sync_dialog(&ctx);
        self.rescue_card(&ctx);
        if let Some((text, at, error)) = self.toast.clone() {
            if at.elapsed() < Duration::from_secs(6) {
                egui::Area::new(egui::Id::unique("toast")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -120.0)).show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(if error { RED_DIM } else { RAISED })
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| ui.label(RichText::new(text).color(Color32::WHITE)));
                });
                ctx.request_repaint_after(Duration::from_millis(500));
            } else {
                self.toast = None;
            }
        }
    }

    fn on_exit(&mut self) {
        self.finish_starting();
        self.remote = None;
        self.midi = None;
        self.osc = None;
        if let Some(mut e) = self.engine.take() {
            e.shutdown();
        }
        if self.cfg_dirty || self.scenes_dirty {
            self.save_config();
        }
        // An update that's ready and told to go in: the helper takes over
        // once Cutaway has closed.
        if let Some(e) = crate::net::self_update::on_exit() {
            eprintln!("update: {e}");
        }
    }
}

/// What the quit confirmation says, for what's running.
fn quit_text(recording: bool, live: bool) -> (String, String, &'static str) {
    let (title, body) = match (recording, live) {
        (true, true) => ("Quit while recording and live?", "Cutaway will finish and save this take, and the broadcast ends on every destination."),
        (false, true) => ("Quit while live?", "The broadcast ends on every destination."),
        _ => ("Quit while recording?", "Cutaway will finish and save this take."),
    };
    (title.to_string(), body.to_string(), "Quit")
}

// ---- drawing helpers -------------------------------------------------------------

fn label_chip(painter: &egui::Painter, left_center: egui::Pos2, text: &str) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(14.0), Color32::WHITE);
    let r = Rect::from_min_size(left_center - vec2(0.0, galley.size().y / 2.0), galley.size());
    let bg = r.expand2(vec2(8.0, 4.0));
    painter.rect_filled(bg, 3.0, Color32::from_black_alpha(170));
    painter.galley(r.min, galley, Color32::WHITE);
    bg
}

fn draw_meter(ui: &mut egui::Ui, size: egui::Vec2, peak: &[f32], rms: &[f32], hold: &[f32], offline: bool, muted: bool) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::from_rgb(8, 9, 12));
    let floor = -60.0f32;
    let to_y = |db: f32| {
        let t = ((db.max(floor) - floor) / -floor).clamp(0.0, 1.0);
        rect.bottom() - t * rect.height()
    };
    let ch = peak.len().max(2);
    let bar_w = (rect.width() - 6.0 - (ch as f32 - 1.0) * 2.0) / ch as f32;
    let dim = |c: Color32| if muted { c.gamma_multiply(0.35) } else { c };
    for c in 0..ch {
        let x = rect.left() + 3.0 + c as f32 * (bar_w + 2.0);
        let p = peak.get(c).copied().unwrap_or(-100.0);
        let r = rms.get(c).copied().unwrap_or(-100.0);
        let zones = [(-60.0, -18.0, GREEN), (-18.0, -6.0, AMBER), (-6.0, 0.0, RED)];
        for (lo, hi, color) in zones {
            if r > lo {
                let top = to_y(r.min(hi));
                let bottom = to_y(lo);
                painter.rect_filled(Rect::from_min_max(pos2(x, top), pos2(x + bar_w, bottom)), 0.0, dim(color));
            }
        }
        if p > floor {
            let y = to_y(p);
            painter.line_segment([pos2(x, y), pos2(x + bar_w, y)], Stroke::new(2.0, dim(Color32::from_white_alpha(200))));
        }
        if let Some(&h) = hold.get(c) {
            if h > floor {
                let y = to_y(h);
                let color = if h > -1.0 { RED } else { Color32::from_white_alpha(120) };
                painter.line_segment([pos2(x, y), pos2(x + bar_w, y)], Stroke::new(1.0, color));
            }
        }
    }
    for db in [-6.0, -18.0, -40.0] {
        let y = to_y(db);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.left() + 3.0, y)], Stroke::new(1.0, LINE));
    }
    if offline {
        painter.text(rect.center(), Align2::CENTER_CENTER, "OFF", FontId::proportional(11.0), AMBER);
    }
}

/// Compressor gain reduction, growing down from the top.
fn draw_reduction(ui: &mut egui::Ui, size: egui::Vec2, db: f32) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, Color32::from_rgb(8, 9, 12));
    let h = (db / 20.0).clamp(0.0, 1.0) * rect.height();
    painter.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), h)), 0.0, ACCENT);
    resp.on_hover_text(format!("Compressor: −{db:.1} dB"));
}

fn snap(v: f32, size: f32) -> f32 {
    let targets = [0.0, 0.5, 1.0];
    let threshold = 0.012;
    for t in targets {
        if (v - t).abs() < threshold {
            return t;
        }
        if (v + size - t).abs() < threshold {
            return t - size;
        }
        if (v + size / 2.0 - t).abs() < threshold {
            return t - size / 2.0;
        }
    }
    v.clamp(-size + 0.02, 0.98)
}

/// The full-screen icon of video players: four corners pointing out (go
/// full screen) or in (leave it).
/// Where the second-screen window opens.
struct OutputPlacement {
    /// The display (by index) to fill, when it opens full screen.
    monitor: Option<usize>,
    fullscreen: bool,
    /// Its top-left corner when it opens in a window.
    position: Option<egui::Pos2>,
}

/// The output goes on the chosen screen, or another one than Cutaway's.
/// It only opens full screen where that doesn't cover Cutaway's own window:
/// with one screen it opens in a window (F still fills the screen, and Esc
/// gets back out).
fn output_placement(ctx: &egui::Context, d: &crate::config::DisplayConfig) -> OutputPlacement {
    use crate::platform::displays;
    let list = displays::list();
    let (outer, native) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().native_pixels_per_point.unwrap_or(1.0)));
    let zoom = ctx.zoom_factor();
    let ours = outer.and_then(|r| displays::containing(&list, displays::window_units([r.center().x * zoom, r.center().y * zoom], native)));
    let target = displays::pick_output(&list, &d.screen, ours);
    let fullscreen = d.fullscreen && displays::output_can_fill(&list, target, ours);
    let position = target.map(|t| {
        let scale = if cfg!(windows) { native } else { 1.0 } * zoom;
        let c = list[t].center();
        pos2(c[0] / scale - 480.0, c[1] / scale - 270.0)
    });
    OutputPlacement { monitor: target, fullscreen, position }
}

fn full_screen_icon(p: &egui::Painter, r: Rect, enter: bool, color: Color32) {
    let s = Stroke::new(2.5, color);
    let arm = r.width() * 0.36;
    for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
        let corner = pos2(if right { r.right() } else { r.left() }, if bottom { r.bottom() } else { r.top() });
        // Towards the middle from this corner.
        let (dx, dy) = (if right { -1.0 } else { 1.0 }, if bottom { -1.0 } else { 1.0 });
        let elbow = if enter { corner } else { corner + vec2(dx * arm, dy * arm) };
        let (ax, ay) = if enter { (dx * arm, dy * arm) } else { (-dx * arm, -dy * arm) };
        p.line_segment([elbow, elbow + vec2(ax, 0.0)], s);
        p.line_segment([elbow, elbow + vec2(0.0, ay)], s);
    }
}

fn unique_name(scenes: &[Scene], base: &str) -> String {
    if !scenes.iter().any(|s| s.name == base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !scenes.iter().any(|s| &s.name == n)).unwrap()
}

fn fmt_clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn style(ctx: &egui::Context) {
    let mut style = egui::Style { visuals: egui::Visuals::dark(), ..(*ctx.global_style()).clone() };
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.widgets.inactive.weak_bg_fill = RAISED;
    style.visuals.widgets.inactive.bg_fill = RAISED;
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 54, 66);
    style.visuals.selection.bg_fill = Color32::from_rgb(40, 90, 160);
    style.visuals.window_stroke = Stroke::new(1.0, LINE);
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 5.0);
    // Cutaway is dark whatever the computer's appearance: its colours are
    // made for it, and on a light-mode computer windows and pop-ups would
    // otherwise turn white under light text.
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
    ctx.set_theme(egui::ThemePreference::Dark);
}

#[cfg(test)]
mod small_tests {
    #[test]
    fn quit_says_what_stops() {
        assert_eq!(super::quit_text(true, false).0, "Quit while recording?");
        assert_eq!(super::quit_text(false, true).0, "Quit while live?");
        assert!(super::quit_text(true, true).1.contains("broadcast ends"));
    }

    #[test]
    fn app_icon_loads() {
        let icon = super::app_icon();
        assert!(icon.width >= 256 && icon.width == icon.height, "{}x{}", icon.width, icon.height);
    }
}

/// "10 minutes", "1 minute", "90 seconds".
fn minutes(m: f32) -> String {
    if m < 1.0 {
        format!("{:.0} seconds", m * 60.0)
    } else if (m - 1.0).abs() < 0.01 {
        "minute".into()
    } else if m.fract().abs() < 0.01 {
        format!("{m:.0} minutes")
    } else {
        format!("{m:.1} minutes")
    }
}
