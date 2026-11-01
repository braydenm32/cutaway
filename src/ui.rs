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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod panels;
mod column;
mod guide;
mod people;
mod production;
mod starting;
mod undo;
mod takes;
mod uploads;
mod waiting;
/// The redesign's map of every route and where it lives (data only for now).
mod nav;
mod routes;
mod shell;
mod search;

pub mod theme {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(14, 16, 20);
    pub const PANEL: Color32 = Color32::from_rgb(22, 25, 31);
    pub const RAISED: Color32 = Color32::from_rgb(34, 38, 46);
    pub const LINE: Color32 = Color32::from_rgb(52, 57, 68);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 147, 160);
    /// On air: the program, the scene and sources on it, tally. Red means
    /// this and nothing else that sits beside it (REC is a dark pill with a
    /// red dot, LIVE has its own pink, buttons like TAKE and FTB aren't red).
    pub const ON_AIR: Color32 = Color32::from_rgb(232, 52, 64);
    pub const ON_AIR_DIM: Color32 = Color32::from_rgb(120, 30, 38);
    /// Errors, failures, clipping and things that can't be undone (the
    /// same red: never next to the on-air ones in a way that mixes them up).
    pub const ERROR: Color32 = ON_AIR;
    pub const ERROR_DIM: Color32 = ON_AIR_DIM;
    /// The recording light: the dot on the dark REC pill.
    pub const REC_DOT: Color32 = Color32::from_rgb(255, 64, 64);
    pub const REC_PILL: Color32 = Color32::from_rgb(40, 30, 34);
    pub const REC_EDGE: Color32 = Color32::from_rgb(120, 44, 50);
    /// Pressed-and-held across the REC pill (to stop): a see-through red.
    pub const REC_HOLD: Color32 = Color32::from_rgba_premultiplied(110, 26, 30, 110);
    /// TAKE: the main switching action (CUT is a plain button).
    pub const TAKE: Color32 = Color32::from_rgb(52, 112, 204);
    pub const TAKE_HOVER: Color32 = Color32::from_rgb(76, 136, 228);
    pub const NEUTRAL_HOVER: Color32 = Color32::from_rgb(60, 66, 80);
    /// An input that's off (not heard in the mixes).
    pub const OFF: Color32 = Color32::from_rgb(58, 62, 74);
    pub const AMBER: Color32 = Color32::from_rgb(245, 166, 35);
    pub const AMBER_DIM: Color32 = Color32::from_rgb(110, 72, 16);
    pub const GREEN: Color32 = Color32::from_rgb(58, 196, 112);
    pub const GREEN_DIM: Color32 = Color32::from_rgb(28, 96, 58);
    pub const ACCENT: Color32 = Color32::from_rgb(90, 160, 255);
    /// Text sizes: every label in Cutaway is one of these five.
    /// Hints, tags and second lines.
    pub const TEXT_SMALL: f32 = 11.5;
    /// Buttons, lists and most labels.
    pub const TEXT_BODY: f32 = 13.0;
    /// What's going on now: the REC and LIVE pills, ON/OFF, loudness,
    /// levels, the scene and source names on the monitors.
    pub const TEXT_STATE: f32 = 14.0;
    /// A window's or card's heading.
    pub const TEXT_HEADING: f32 = 16.0;
    /// Big: clocks, "The engine couldn't start", the clap.
    pub const TEXT_TITLE: f32 = 22.0;
    /// "On air" for streaming, so it reads differently from recording red.
    pub const LIVE: Color32 = Color32::from_rgb(214, 40, 120);
    /// GOING LIVE pulses between these; the GO LIVE button's edge.
    pub const LIVE_DIM: Color32 = Color32::from_rgb(60, 24, 44);
    pub const LIVE_GLOW: Color32 = Color32::from_rgb(150, 30, 90);
    pub const LIVE_EDGE: Color32 = Color32::from_rgb(110, 30, 70);
    /// Text about going live ("Connecting to YouTube").
    pub const LIVE_SOFT: Color32 = Color32::from_rgb(230, 120, 170);
    /// Live, but a destination isn't.
    pub const LIVE_TROUBLE: Color32 = Color32::from_rgb(170, 90, 20);
}
use theme::*;

/// The right-hand column's usual width.
const COLUMN_WIDTH: f32 = 400.0;
/// How long the REC button is held to stop the take.
const REC_HOLD_SECS: f32 = 1.0;
/// A scene button's size (at most, and with its picture).
const BANK_BUTTON: egui::Vec2 = vec2(132.0, 84.0);
/// Scene buttons with a picture get no narrower than this.
const BANK_THUMB_MIN: f32 = 100.0;
/// Up to this many scenes always fit in the bank; past it, it may scroll
/// sideways (with "+n ›") rather than shrink the buttons further.
const BANK_FIT: usize = 12;

/// How the scene bank lays its buttons out.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BankLayout {
    rows: usize,
    button: egui::Vec2,
    /// A live picture on each (else just the key and the name).
    thumbs: bool,
}

/// `n` scene buttons in `width` by `height`: one row with pictures while
/// they fit; a tall window's second row (`tall`); else two rows of
/// key-and-name buttons, so up to BANK_FIT scenes are all in view.
fn bank_layout(n: usize, width: f32, height: f32, gap: f32, gap_y: f32, tall: bool) -> BankLayout {
    let across = |per_row: usize| (width + gap) / per_row.max(1) as f32 - gap;
    let one = across(n).min(BANK_BUTTON.x);
    if one >= BANK_THUMB_MIN && n <= BANK_FIT {
        return BankLayout { rows: 1, button: vec2(one.floor(), BANK_BUTTON.y), thumbs: true };
    }
    let half = n.div_ceil(2);
    if tall {
        // (The bank has grown to fit two rows of them: see `fn ui`.)
        let w = if n <= BANK_FIT { across(half).clamp(BANK_THUMB_MIN, BANK_BUTTON.x) } else { BANK_BUTTON.x };
        return BankLayout { rows: 2, button: vec2(w.floor(), BANK_BUTTON.y), thumbs: true };
    }
    let h = ((height - gap_y) / 2.0).floor().clamp(28.0, 44.0);
    // (A long bank keeps its names readable, and scrolls.)
    let w = across(half).clamp(if n <= BANK_FIT { 64.0 } else { 120.0 }, 220.0);
    BankLayout { rows: 2, button: vec2(w.floor(), h), thumbs: false }
}

/// What a multiview tile's ⋯ (or right-click) menu picked.
enum TilePick {
    Framing(crate::config::FramingConfig),
    Player(PlayerCmd),
    /// Split into this many close-ups.
    Split(usize),
}

/// A multiview tile's ⋯ menu, the same as its right-click one: who an
/// auto-framed source follows and how tight, a media player's playlist, or
/// splitting a camera into close-ups.
fn tile_menu(ui: &mut egui::Ui, framing: Option<&crate::config::FramingConfig>, player: Option<&crate::engine::player::PlayerState>, split: bool) -> Option<TilePick> {
    let mut pick = None;
    if let Some(cfg) = framing {
        ui.label(RichText::new("Follow").strong());
        for v in crate::config::FramingFollow::ALL {
            if ui.selectable_label(cfg.follow == v, v.label()).clicked() {
                pick = Some(TilePick::Framing(crate::config::FramingConfig { follow: v, ..cfg.clone() }));
                ui.close();
            }
        }
        ui.separator();
        ui.label(RichText::new("Shot").strong());
        for v in crate::config::FramingShot::ALL {
            if ui.selectable_label(cfg.shot == v, v.label()).clicked() {
                pick = Some(TilePick::Framing(crate::config::FramingConfig { shot: v, ..cfg.clone() }));
                ui.close();
            }
        }
    }
    if let Some(st) = player {
        ui.label(RichText::new("Playlist").strong());
        for (k, item) in st.items.iter().enumerate() {
            if ui.selectable_label(k == st.index, item).clicked() {
                pick = Some(TilePick::Player(PlayerCmd::Select(k)));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Previous").clicked() {
            pick = Some(TilePick::Player(PlayerCmd::Previous));
            ui.close();
        }
    }
    if split {
        ui.label(RichText::new("Close-ups of the people in it").strong());
        for k in [2, 3] {
            if ui.button(format!("Split into {k} close-ups…")).clicked() {
                pick = Some(TilePick::Split(k));
                ui.close();
            }
        }
    }
    pick
}

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

const MIN_WINDOW: [f32; 2] = [1000.0, 640.0];

/// The first window's size on a screen of `screen` points: the usual size,
/// smaller to leave room for the menu bar, title bar and dock or taskbar.
fn first_window_size(inner: egui::Vec2, screen: egui::Vec2) -> egui::Vec2 {
    let room = egui::vec2(screen.x - 40.0, screen.y - 140.0);
    inner.min(room).max(egui::Vec2::from(MIN_WINDOW))
}

pub fn run(cfg: Config, save_path: Option<PathBuf>, self_test: bool, made_pin: bool, first_run: bool) -> anyhow::Result<()> {
    // The window's size and place, remembered per computer beside the
    // settings (eframe puts it back on a screen that's still there).
    let window_file = crate::config::default_config_path().parent().map(|d| d.join("Main window.ron"));
    let remembered = window_file.as_ref().is_some_and(|f| f.exists());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Cutaway").with_icon(app_icon()).with_inner_size([1480.0, 920.0]).with_min_inner_size(MIN_WINDOW),
        persist_window: true,
        persistence_path: window_file,
        ..Default::default()
    };
    eframe::run_native(
        "Cutaway",
        options,
        Box::new(move |cc| {
            // Uploads left unfinished last time are kept beside the settings.
            let queue = save_path.as_ref().and_then(|p| p.parent()).map(|d| d.join("uploads.json")).or_else(|| std::env::var_os("CUTAWAY_UI_TEST_UPLOADS").map(PathBuf::from));
            let mut app = App::new(cc, cfg, save_path);
            if let Some(q) = queue {
                app.uploads.load(q);
            }
            app.guide_open = first_run || std::env::var_os("CUTAWAY_UI_TEST_GUIDE").is_some();
            app.fit_window = !remembered;
            if made_pin {
                // Said after any update news, so neither hides the other.
                let pin = format!("The remote now needs a PIN: {} (Settings → Remote & MIDI)", app.cfg.remote.pin);
                // Up for half a minute rather than the usual few seconds:
                // phones and the Stream Deck stop working until it's typed in.
                app.toasts.push(Toast { text: pin, until: Some(Instant::now() + Duration::from_secs(30)), error: false });
            }
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
    started: Instant,
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
    /// Takes started this session, and whether one was recording last
    /// frame (the starting slate's "TAKE n" is the next take).
    takes_made: u32,
    was_recording: bool,
    scenes: Vec<Scene>,
    scenes_dirty: bool,
    cfg_dirty: bool,
    /// When the desk first changed something not yet written to disk.
    dirty_since: Option<Instant>,
    textures: HashMap<String, (u64, egui::TextureHandle)>,
    /// The right-hand column's edge is being dragged.
    column_dragged: bool,
    /// The right-hand column: Arrange panels, the audio strip's height.
    column: column::ColumnState,
    waker: Waker,
    edit_mode: bool,
    selected_layer: Option<usize>,
    /// A scene button dropped in a new place in the bank: (from, before).
    bank_move: Option<(usize, usize)>,
    /// A scene's new name being typed (scene, text): it's renamed when
    /// the box is left, so a half-typed name never clashes with another.
    scene_name_edit: Option<(String, String)>,
    /// Undo and redo for scene layouts and settings.
    history: undo::History,
    /// Changes from a page that saves as you go, left waiting for its box
    /// when you went elsewhere: applied at the start of the next frame.
    deferred_settings: Vec<SettingsAction>,
    /// Saving a change made as you go: no "Saved settings" message.
    quiet_save: bool,
    drag: Option<Drag>,
    settings: Option<SettingsUi>,
    confirm: Option<Confirm>,
    /// The REC button: when the hold to stop began, whether this press
    /// already stopped the take, and until when "Hold to stop" shows.
    rec_hold: Option<Instant>,
    rec_hold_spent: bool,
    rec_hint_until: Option<Instant>,
    /// The self-test holds the REC button (for a screenshot).
    test_hold: bool,
    /// The self-test opens this take's marker list.
    test_marker_list: bool,
    /// The self-test takes this source's signal away (by number).
    test_drop: Option<usize>,
    /// Closing was confirmed (or nothing was running): let the window go.
    quit_ok: bool,
    /// The first-show guide is showing.
    guide_open: bool,
    /// ⌘K search (ui/search.rs).
    search: search::Search,
    /// No remembered window yet: fit the first one to the screen.
    fit_window: bool,
    /// A recording was started (the guide's last step).
    guide_recorded: bool,
    /// The Settings page last open, so the window comes back to it (this run only).
    settings_page: Option<Section>,
    /// Sound was heard once while the guide was open (latched so the mic step doesn't flicker).
    guide_heard: bool,
    /// The pre-show check was run from the guide.
    guide_checked: bool,
    /// Edit layout was turned on from the guide (its scenes step).
    guide_scenes: bool,
    /// The rail, the top bar's tabs and the status light.
    shell: shell::Shell,
    peak_hold: HashMap<String, PeakHold>,
    /// Messages at the bottom, newest last. Errors stay until clicked;
    /// the rest go after a few seconds.
    toasts: Vec<Toast>,
    /// How many log lines had been written when the log was last looked at.
    log_seen: u64,
    log_warnings_only: bool,
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
    /// ⌂ Home held while the camera's on air: since when, and whether it
    /// went home already (so a long hold sends it once).
    ptz_home_hold: Option<(Instant, bool)>,
    /// A virtual camera's shot before Home, for Undo: (camera, shot, when).
    ptz_home_undo: Option<(usize, crate::engine::ptz::Shot, Instant)>,
    /// The last replay problem shown.
    replay_problem: Option<String>,
    /// The scene bank: scenes that don't fit in one row (last frame), how
    /// many rows it has, and a scroll asked for by its edge arrows.
    bank_overflows: bool,
    bank_rows: usize,
    bank_scroll_to: Option<f32>,
    /// The multiview tile picked with Preview off (it shows "Put on air").
    mv_selected: Option<usize>,
    /// Sources known to have no signal (None: not looked yet with this
    /// engine, so the ones missing from the start don't count as drops).
    dropped: Option<HashSet<String>>,
    /// Each source (by number): no signal now.
    offline_sources: Vec<bool>,
    /// The scheduled start this session already went live for.
    scheduled_done: Option<String>,
    /// Slow motion ticked in the Replay button's menu.
    replay_slow: bool,
    /// A phone camera's QR code on show: its link id.
    qr_for: Option<String>,
    /// A newer Cutaway, once GitHub has said so (More › App › Updates & about).
    update: Arc<std::sync::Mutex<Option<crate::net::update::Release>>>,
    /// The logo at the top of the rail; clicking it opens About.
    logo: egui::TextureHandle,
    /// When this (just updated) version counts as started properly.
    settle_at: Option<Instant>,
    /// Uploads to YouTube and Google Drive.
    uploads: uploads::Uploads,
    /// The "Take saved" card.
    takes: takes::Takes,
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
    /// The last recording failure and when, so controllers still hear of it
    /// after `poll_status` has taken it (cleared by the next take).
    rec_problem: Option<(String, Instant)>,
    precheck: Option<Precheck>,
    /// Title being edited live: (graphic, title, subtitle).
    title_edit: Option<(usize, String, String)>,
    /// A marker being named (Shift+M): its time (which identifies it), its
    /// own name until then, and what's typed.
    marker_name: Option<(crate::media::Ns, String, String)>,
    /// A marker's name being changed in the list under ◆ Marker ▾.
    marker_edit: Option<(crate::media::Ns, String)>,
    display_fullscreen: bool,
    /// Full screen on or off asked for from Record › Screens & webcam.
    display_toggle_full: bool,
    /// The write rate expected for the settings the take in progress started with.
    take_rate_estimate: Option<f64>,
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
            ptz_home_hold: None,
            ptz_home_undo: None,
            replay_problem: None,
            bank_overflows: false,
            bank_rows: 1,
            bank_scroll_to: None,
            mv_selected: None,
            dropped: None,
            offline_sources: Vec::new(),
            scheduled_done: None,
            replay_slow: false,
            qr_for: None,
            update: Arc::new(std::sync::Mutex::new(None)),
            logo: logo_texture(&cc.egui_ctx),
            settle_at: None,
            uploads: Default::default(),
            takes: Default::default(),
            chat: None,
            comment_on: None,
            comment_at: None,
            comment_cursor: 0,
            prompter: production::Prompter { wpm: cfg.prompter.words_per_minute, ..Default::default() },
            pad_held: None,
            talk_held: false,
            scenes: cfg.effective_scenes(),
            display_fullscreen: cfg.display.fullscreen,
            display_toggle_full: false,
            take_rate_estimate: None,
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
            takes_made: 0,
            was_recording: false,
            scenes_dirty: false,
            cfg_dirty: false,
            dirty_since: None,
            textures: HashMap::new(),
            column_dragged: false,
            column: Default::default(),
            waker,
            edit_mode: false,
            bank_move: None,
            selected_layer: None,
            scene_name_edit: None,
            history: undo::History::default(),
            deferred_settings: Vec::new(),
            quiet_save: false,
            drag: None,
            settings: None,
            confirm: None,
            rec_hold: None,
            rec_hold_spent: false,
            rec_hint_until: None,
            test_hold: false,
            test_drop: None,
            test_marker_list: false,
            quit_ok: false,
            guide_open: false,
            search: search::Search::default(),
            fit_window: false,
            guide_recorded: false,
            settings_page: None,
            guide_heard: false,
            guide_checked: false,
            guide_scenes: false,
            shell: shell::Shell::load(cc.storage),
            peak_hold: HashMap::new(),
            toasts: Vec::new(),
            log_seen: 0,
            log_warnings_only: false,
            requests_tx,
            requests_rx,
            remote_shared,
            remote: None,
            midi: None,
            osc: None,
            last_publish: Instant::now(),
            rec_problem: None,
            precheck: None,
            title_edit: None,
            marker_name: None,
            marker_edit: None,
            self_test: None,
        };
        app.vcam_wanted = app.cfg.display.virtual_camera;
        app.start_engine();
        app.apply_control_settings();
        if !app.cfg.held_secrets.is_empty() {
            app.toast(
                format!(
                    "{} saved key(s) or password(s) couldn't be read from the {}. Allow Cutaway to use them and restart, or type them in again in Settings.",
                    app.cfg.held_secrets.len(),
                    if cfg!(windows) { "Credential Manager" } else { "Keychain" }
                ),
                true,
            );
        }
        // How the last update went (and the old version let go of, once
        // this one has run a while).
        if let Ok(me) = crate::net::self_update::Install::here() {
            use crate::net::self_update::Outcome;
            match crate::net::self_update::after_start(&me, env!("CARGO_PKG_VERSION")) {
                Some(Outcome::Updated { from }) => {
                    let mut text = format!("Updated to {} (from {from})", env!("CARGO_PKG_VERSION"));
                    if crate::platform::vcam::needs_reinstall() {
                        text.push_str(". The virtual camera changed too: reinstall it in Record › Screens & webcam");
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
            app.go("more/app/about");
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
    /// Cutaway, its version and where to read more (More › App › Updates & about).
    fn about_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).max_height(64.0));
            ui.vertical(|ui| {
                ui.label(RichText::new("Cutaway").size(TEXT_TITLE).strong().color(Color32::WHITE));
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
                ui.label(RichText::new(&r.notes).size(TEXT_BODY).color(TEXT_DIM));
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
                    ui.label(RichText::new(format!("Get it from the download page: {why}.")).size(TEXT_SMALL).color(TEXT_DIM));
                }
            }
            Stage::Downloading { got, total } => {
                let k = total.filter(|t| *t > 0).map(|t| got as f32 / t as f32).unwrap_or(0.0);
                ui.add(egui::ProgressBar::new(k).desired_width(300.0).text(format!("Downloading… {}", crate::net::ndi_install::progress_text(got, total))));
                ui.horizontal(|ui| {
                    if ui.button("Stop").clicked() {
                        su::cancel();
                    }
                    ui.label(RichText::new("Carry on with the show meanwhile.").size(TEXT_SMALL).color(TEXT_DIM));
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
                    let b = if live { b.on_disabled_hover_text("Wait until recording, streaming, saving and uploads have finished (or choose When I quit)") } else { b.on_hover_text("Cutaway closes, updates and opens again") };
                    if b.clicked() {
                        *restart = true;
                    }
                    let on_quit = su::finish() == Finish::OnQuit;
                    if ui.add(egui::Button::selectable(on_quit, "When I quit")).on_hover_text("Update the next time Cutaway is closed").clicked() {
                        su::choose(if on_quit { Finish::Ask } else { Finish::OnQuit });
                    }
                });
                if su::finish() == Finish::OnQuit {
                    ui.label(RichText::new("It goes in when you close Cutaway.").size(TEXT_SMALL).color(TEXT_DIM));
                } else if live {
                    ui.label(RichText::new("Restarting waits until recording and streaming have stopped.").size(TEXT_SMALL).color(TEXT_DIM));
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
        ui.label(RichText::new("Your settings and shows stay as they are when you update.").size(TEXT_SMALL).color(TEXT_DIM));
    }

    /// Syrinscape, guests and chat to match the settings (each only
    /// restarts what changed).
    fn apply_services(&mut self) {
        self.syrinscape.set_token(&self.cfg.syrinscape.token);
        crate::engine::guests::hub().apply(&self.cfg, self.waker.clone());
        self.apply_chat();
    }

    /// Start or stop the remote server, MIDI and OSC (and the services
    /// above) to match the settings.
    fn apply_control_settings(&mut self) {
        self.apply_services();
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
        self.restart_midi();
        self.osc = None;
        if self.cfg.osc.enabled {
            match osc::Osc::start(&self.cfg.osc, self.requests_tx.clone(), self.waker.clone()) {
                Ok(o) => self.osc = Some(o),
                Err(e) => self.log(LogLevel::Warning, format!("OSC: {e:#}")),
            }
        }
    }

    /// MIDI as set (its mappings are read when it starts).
    fn restart_midi(&mut self) {
        self.midi = None;
        if self.cfg.midi.enabled && midi::available() {
            match midi::Midi::start(&self.cfg.midi, self.requests_tx.clone(), self.waker.clone()) {
                Ok(m) => self.midi = Some(m),
                Err(e) => self.log(LogLevel::Warning, format!("MIDI: {e:#}")),
            }
        }
    }

    /// Rename the scene whose new name is being typed in the edit bar,
    /// before its box was left (Done or Save pressed straight away).
    fn commit_scene_name(&mut self) {
        let Some((of, text)) = self.scene_name_edit.take() else { return };
        let text = text.trim().to_string();
        let Some(i) = self.scenes.iter().position(|s| s.name == of) else { return };
        if text.is_empty() || text == of {
            return;
        }
        if self.scenes.iter().any(|s| s.name != of && s.name.eq_ignore_ascii_case(&text)) {
            self.toast(format!("There's already a scene called {text}"), true);
            return;
        }
        self.remember(undo::Kind::Scenes, format!("Rename scene {of}"));
        let before = self.scene_names();
        self.scenes[i].name = text;
        self.scenes_dirty = true;
        // (The scene on air, in preview or last on air takes the name too.)
        self.scenes_moved(&before);
    }

    fn scene_names(&self) -> Vec<String> {
        self.scenes.iter().map(|s| s.name.clone()).collect()
    }

    /// The scene bank was reordered, or a scene renamed or deleted (it was
    /// `before`, by name): the scenes on air, in preview and last on air take
    /// their new names, and what the show keeps that picks a scene (macros,
    /// MIDI, keys, cues, the rundown…) goes on picking the same one.
    fn scenes_moved(&mut self, before: &[String]) {
        let after = self.scene_names();
        if before == after.as_slice() {
            return;
        }
        let renamed = control::scene_renames(before, &after);
        if let (Some(e), false) = (&self.engine, renamed.is_empty()) {
            e.switcher.rename_scenes(&|n| renamed.iter().find(|(a, _)| a == n).map(|(_, b)| b.clone()));
        }
        let midi = self.cfg.midi.clone();
        // (The bank's own cues too: it stands in for the saved scenes.)
        let saved = std::mem::replace(&mut self.cfg.scenes, std::mem::take(&mut self.scenes));
        let changed = control::follow_scenes(&mut self.cfg, before, &after);
        self.scenes = std::mem::replace(&mut self.cfg.scenes, saved);
        if changed {
            // (Written with the scenes, so the file never has one without the other.)
            self.scenes_dirty = true;
            if let Some(e) = &self.engine {
                let _ = e.update_config(&self.cfg);
            }
            if self.cfg.midi != midi {
                self.restart_midi();
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
                    crate::secrets::tidy(p);
                    if !self.quiet_save {
                        self.toast(format!("Saved settings to {}", p.display()), false)
                    }
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

    /// Changes made at the desk (levels, inputs on or off, the transition,
    /// layout) are written a few seconds after the last one, so a crash
    /// doesn't lose them. Scene edits wait until Edit layout is done.
    fn autosave(&mut self) {
        // A scene name typed in the edit bar when Edit layout was left (Done, E).
        if !self.edit_mode && self.scene_name_edit.is_some() {
            self.commit_scene_name();
        }
        let pending = self.cfg_dirty || (self.scenes_dirty && !self.edit_mode);
        if !pending {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        if since.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.dirty_since = None;
        if self.scenes_dirty && !self.edit_mode {
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
                    if !self.cfg.ui.profile.trim().is_empty() {
                        let _ = profiles::save(&crate::config::profiles_dir(p), &self.cfg.ui.profile, &self.cfg);
                    }
                }
                Err(e) => self.log(LogLevel::Warning, format!("Couldn't save settings: {e:#}")),
            }
        }
    }

    fn toast(&mut self, text: String, error: bool) {
        // One passing message at a time; errors pile up until they're read.
        self.toasts.retain(|t| t.error || t.until.is_none());
        self.toasts.retain(|t| t.text != text);
        if error {
            // Errors go in the log too, to look back at (and for problem reports).
            if let Some(e) = &self.engine {
                let mut st = e.status.lock_ok();
                if !st.log.iter().rev().take(5).any(|l| l.text == text) {
                    st.push(LogLevel::Error, text.clone());
                }
            }
            while self.toasts.iter().filter(|t| t.error).count() >= 4 {
                if let Some(i) = self.toasts.iter().position(|t| t.error) {
                    self.toasts.remove(i);
                }
            }
        }
        let until = (!error).then(|| Instant::now() + Duration::from_secs(6));
        self.toasts.push(Toast { text, until, error });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.until.is_none_or(|u| Instant::now() < u));
        if self.toasts.is_empty() {
            return;
        }
        let mut gone = None;
        egui::Area::new(egui::Id::unique("toasts")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -120.0)).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for (i, t) in self.toasts.iter().enumerate() {
                let r = egui::Frame::new()
                    .fill(if t.error { ERROR_DIM } else { RAISED })
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&t.text).color(Color32::WHITE));
                            if t.error {
                                ui.label(RichText::new("✕").color(Color32::WHITE));
                            }
                        });
                    })
                    .response
                    .interact(Sense::click());
                if t.error && r.on_hover_text("Click to close (it's kept in the Log)").clicked() {
                    gone = Some(i);
                }
            }
        });
        if let Some(i) = gone {
            self.toasts.remove(i);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
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
                // While recording or live, the same care as Settings and the
                // show list: no sources taken out or restarted, no other show.
                if self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) {
                    if s.cfg.ui.profile != self.cfg.ui.profile {
                        return Err("stop recording and streaming before switching shows".into());
                    }
                    if s.cfg.lineup_changed(&self.cfg) {
                        return Err("it changes the sources or inputs: stop recording and streaming first, or change them in Settings".into());
                    }
                }
                self.apply_settings_as(s.cfg.clone(), ctx, true)?;
                let now = self.live_settings();
                if let Some(win) = self.settings.as_mut() {
                    win.restored(&now);
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
        let before = std::mem::replace(&mut self.scenes, s.scenes.clone());
        // (Sources taken away since can't come back with a scene.)
        let n = self.cfg.sources.len();
        for sc in &mut self.scenes {
            sc.retain_sources(n);
        }
        self.scenes_dirty = true;
        // A move or rename undone or redone: what's on air, in preview and
        // last on air, and what picks a scene, follow.
        let before: Vec<String> = before.iter().map(|s| s.name.clone()).collect();
        self.scenes_moved(&before);
        if let Some(e) = &self.engine {
            if self.preview_mode() {
                // With Preview on, the program never changes but by Take:
                // the scene in preview shows the change.
                if let Some(sc) = e.switcher.preview().and_then(|p| self.scenes.iter().find(|x| x.name == p.name)).cloned() {
                    e.switcher.set_preview(Some(sc));
                }
            } else {
                let live = e.switcher.live().name;
                let on_air = self.scenes.iter().find(|x| x.name == live).or_else(|| self.scenes.iter().find(|x| x.name == s.live)).cloned();
                if let Some(sc) = on_air {
                    e.switcher.set_live(sc);
                }
            }
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
            if self.edit_mode {
                // Another scene to edit: nothing in it is picked yet.
                self.selected_layer = None;
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

    /// The scene on air before this one, back on air as if its button were
    /// pressed to take it (with the transition and its cues). A scene since
    /// changed in the bank goes back as it is now.
    fn back_to_last_scene(&mut self) -> Result<String, String> {
        let e = self.engine.as_ref().ok_or("the engine isn't running")?;
        let last = e.switcher.previous().ok_or("nothing else has been on air yet")?;
        let scene = self.scenes.iter().find(|s| s.name == last.name).cloned().unwrap_or(last);
        self.take_scene(&scene, false);
        Ok(scene.name)
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
            // Keys always ask (the REC button is held instead), so a stray
            // Cmd+R can't split the take.
            if self.confirm.is_none() {
                self.confirm = Some(Confirm::StopRecording);
            }
        } else if self.cfg.ui.check_before_start && self.precheck.is_none() {
            self.start_precheck(Some(Then::Record));
        } else {
            self.start_recording_now();
        }
    }

    /// Work that quitting would cut short, after recording and streaming
    /// have stopped: saving the last take, the podcast copy, uploads.
    fn busy(&self) -> Option<String> {
        let mut what = Vec::new();
        if let Some(e) = &self.engine {
            let st = e.status.lock_ok();
            if let RecState::Finalizing { done, .. } = &st.rec {
                what.push(format!("saving the last take ({:.0}%)", done * 100.0));
            }
            if let Some((step, _)) = &st.background {
                what.push(step.to_lowercase());
            }
        }
        let uploading = self.uploads.jobs.lock_ok().iter().filter(|j| matches!(j.state, uploads::JobState::Waiting | uploads::JobState::Uploading)).count();
        if uploading > 0 {
            what.push(format!("uploading {uploading} file{}", if uploading == 1 { "" } else { "s" }));
        }
        (!what.is_empty()).then(|| what.join(" and "))
    }

    fn start_recording_now(&mut self) {
        let Some(e) = &self.engine else { return };
        if let Err(err) = e.start_recording() {
            let msg = format!("Couldn't start recording: {err:#}");
            self.toast(msg, true);
        } else {
            self.guide_recorded = true;
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
            self.open_page(Section::Streaming, None);
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
                    // As from a scene button: only sources that exist.
                    let mut p = s.clone();
                    p.retain_sources(e.sources.len());
                    e.switcher.set_preview(Some(p));
                    (self.waker)();
                }
                Ok(format!("{} in preview", s.name))
            }
            Action::Back => {
                let name = self.back_to_last_scene()?;
                Ok(format!("Back to {name}"))
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
                if want && !now && self.cfg.ui.check_before_start {
                    // Controllers get the same pre-show check as the button.
                    if self.precheck.is_none() {
                        self.start_precheck(Some(Then::Record));
                    }
                    return Ok("Running the pre-show check on the desk".into());
                }
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
                if want && !now && self.cfg.ui.check_before_start && self.cfg.streaming.destinations.iter().any(|d| d.enabled) {
                    if self.precheck.is_none() {
                        self.start_precheck(Some(Then::Live));
                    }
                    return Ok("Running the pre-show check on the desk".into());
                }
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
        let saving = match &st.rec {
            RecState::Finalizing { step, done, .. } => Some(crate::control::state::SavingState { step: step.clone(), percent: (done * 100.0).round().clamp(0.0, 100.0) as u32 }),
            _ => None,
        };
        // The same LIVE time as the desk's: from the first destination really live.
        let live_seconds = st.stream.live_seconds();
        let mut problems = st.health.problems();
        // Still there until a new take starts (poll_status takes the flag itself).
        let rec_since = match &st.rec {
            RecState::Recording { since, .. } => Some(*since),
            _ => None,
        };
        let rec_problem = self.rec_problem.as_ref().filter(|(_, at)| rec_since.is_none_or(|s| s < *at));
        if st.rec_failed.is_some() || rec_problem.is_some() {
            problems.push(match rec_problem {
                Some((f, _)) => format!("recording problem: {f}"),
                None => "recording problem".into(),
            });
        }
        let disk = st.disk.as_ref().map(|d| d.describe()).unwrap_or_default();
        let disk_left = st.disk.as_ref().and_then(|d| d.seconds_left()).map(crate::engine::health::fmt_left).unwrap_or_default();
        let stream = show::StreamState::from_status(&st.stream, e.is_streaming());
        let markers = st.markers;
        drop(st);
        let tally = |i: usize| if live_sources.contains(&i) { "program" } else if preview_sources.contains(&i) { "preview" } else { "off" }.to_string();
        Some(ShowState {
            saving,
            version: env!("CARGO_PKG_VERSION").into(),
            show_name: self.cfg.ui.profile.clone(),
            stream,
            disk_left,
            precheck: self.precheck.as_ref().map(|pc| {
                let worst = pc.items.iter().map(|i| i.level).max().unwrap_or(Level::Ok);
                let word = |l: Level| match l {
                    Level::Ok => "ok",
                    Level::Warn => "warn",
                    Level::Fail => "fail",
                };
                show::PrecheckState {
                    then: match pc.then {
                        Some(Then::Record) => "record",
                        Some(Then::Live) => "live",
                        None => "",
                    }
                    .into(),
                    verdict: precheck_verdict(&pc.items, pc.waiting == 0).0,
                    level: word(worst).into(),
                    waiting: pc.waiting,
                    items: pc.items.iter().filter(|i| i.level != Level::Ok).map(|i| show::CheckLine { level: word(i.level).into(), what: i.what.clone(), detail: i.detail.clone() }).collect(),
                }
            }),
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
            sources: e.sources.iter().enumerate().map(|(i, s)| show::SourceState { name: s.name.clone(), tally: tally(i), offline: e.source_offline(i) }).collect(),
            graphics: e.graphics.list().into_iter().enumerate().map(|(i, (name, on, _))| show::GraphicState { name, on, names: self.cfg.graphics.get(i).is_some_and(|g| g.follow) }).collect(),
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
            comments: self.chat.as_ref().map(|c| {
                let messages = c.messages();
                show::CommentsState {
                    showing: self.comment_on.is_some(),
                    waiting: messages.iter().filter(|m| m.id > self.comment_cursor).count(),
                    recent: messages
                        .iter()
                        .rev()
                        .take(15)
                        .rev()
                        .map(|m| show::CommentInfo { id: m.id, author: m.author.clone(), text: m.text.clone(), platform: m.platform.name().into(), showing: self.comment_on == Some(m.id), new: m.id > self.comment_cursor })
                        .collect(),
                }
            }),
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
                    drift_text: show::RundownState::drift_words(st.drift),
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
        // (Settings lets them through after a click outside it, while the
        // pointer's away from it.)
        // (An area page's settings let them through, except while one of
        // its questions is up or it's waiting for a key.)
        if self.keymap.as_ref().is_none_or(|(c, _)| *c != self.cfg.keys) {
            self.keymap = Some((self.cfg.keys.clone(), Keymap::new(&self.cfg.keys)));
        }
        // ⌘K works even while typing; the search box keeps the rest.
        self.search_key(ctx);
        if ctx.egui_wants_keyboard_input() || self.search_open() || self.settings.as_ref().is_some_and(|s| s.busy()) || self.title_edit.is_some() || self.marker_name.is_some() || self.confirm.is_some() || self.precheck.is_some() || self.sync.is_some() {
            return;
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
                // ⌘1–⌘7 to the areas, Esc back to Show (unless given to something else).
                None if !repeat && self.shell_key(combo) => {}
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

    /// The key for a built-in shortcut ("Shift+M"), or `none` when it has none.
    fn key_label_or(&self, b: Builtin, none: &str) -> String {
        let label = match &self.keymap {
            Some((_, m)) => m.label_of(b),
            None => Keymap::new(&self.cfg.keys).label_of(b),
        };
        if label.is_empty() { none.to_string() } else { label }
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
            Builtin::Search => self.toggle_search(),
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
            // The time is taken now, so typing the name doesn't move it.
            Builtin::NamedMarker => match self.engine.as_ref().map(|e| e.add_marker_now(None)) {
                Some(Ok(m)) => self.marker_name = Some((m.at, m.name, String::new())),
                Some(Err(err)) => self.toast(format!("{err:#}"), true),
                None => {}
            },
            Builtin::Auto => {
                let on = !self.auto_on();
                self.set_auto(on);
            }
            Builtin::Back => {
                if let Err(e) = self.back_to_last_scene() {
                    self.toast(format!("Can't go back: {e}"), false);
                }
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

    /// Going live by itself at the scheduled start time (if Cutaway is open
    /// then; up to ten minutes late, never later, and only once).
    fn scheduled_go_live(&mut self, ctx: &egui::Context) {
        let s = &self.cfg.schedule;
        let Some(at) = s.start_time() else { return };
        let left = at - chrono::Local::now();
        // The countdown in the header ticks.
        if (s.go_live || !s.youtube_id.is_empty()) && left.num_seconds() > 0 && left.num_minutes() < 60 {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if !s.go_live || left.num_seconds() > 0 || self.scheduled_done.as_deref() == Some(s.start.as_str()) {
            return;
        }
        if left.num_minutes() < -10 {
            self.scheduled_done = Some(s.start.clone());
            return;
        }
        let Some(e) = &self.engine else {
            // Still starting up: try again in a moment (for up to 10 minutes).
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        };
        self.scheduled_done = Some(s.start.clone());
        if e.is_streaming() {
            return;
        }
        if !self.cfg.streaming.destinations.iter().any(|d| d.enabled) {
            self.toast("It's the scheduled start time, but there's nowhere to stream to: add one in Record › Streaming".into(), true);
            return;
        }
        self.toast("Going live: it's the scheduled start time".into(), false);
        self.start_stream_now();
    }

    /// The header line before a scheduled stream (within a day).
    fn scheduled_line(&self) -> Option<(String, Color32)> {
        let s = &self.cfg.schedule;
        if !s.go_live && s.youtube_id.is_empty() {
            return None;
        }
        let at = s.start_time()?;
        let left = at - chrono::Local::now();
        if left.num_seconds() <= 0 || left.num_hours() >= 24 {
            return None;
        }
        let name = if s.title.trim().is_empty() { "Scheduled stream".to_string() } else { s.title.trim().to_string() };
        let how = if s.go_live { "goes live by itself" } else { "go live yourself" };
        if left.num_minutes() < 60 {
            let secs = left.num_seconds();
            Some((format!("{name}: live in {}:{:02} ({how})", secs / 60, secs % 60), AMBER))
        } else {
            Some((format!("{name}: live at {} ({how})", at.format("%H:%M")), TEXT_DIM))
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
        if step == 3 && t > 7.0 && self.marker_name.is_none() {
            // CUTAWAY_UI_TEST_MARKERS=list opens the take's markers, =name the Shift+M box.
            match std::env::var("CUTAWAY_UI_TEST_MARKERS").as_deref() {
                Ok("list") => {
                    for name in ["Guest arrives", "Big laugh"] {
                        let _ = self.add_marker(Some(name.into()));
                    }
                    self.test_marker_list = true;
                    self.self_test = Some((start, 5));
                }
                Ok("name") => {
                    self.run_builtin(ctx, Builtin::NamedMarker, Key::M);
                    if let Some(m) = &mut self.marker_name {
                        m.2 = "Cold open ends".into();
                    }
                    self.self_test = Some((start, 5));
                }
                _ => {}
            }
            if self.self_test.is_some_and(|(_, s)| s == 5) {
                return;
            }
        }
        if step == 0 {
            // A window this size (CUTAWAY_UI_TEST_WINDOW=1500x1200).
            if let Some((w, h)) = std::env::var("CUTAWAY_UI_TEST_WINDOW").ok().and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)))) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
            }
        }
        if step == 0 && self.scenes.len() < 12 {
            // More scenes than fit, some with long names (CUTAWAY_UI_TEST_SCENES=16).
            if let Some(n) = std::env::var("CUTAWAY_UI_TEST_SCENES").ok().and_then(|v| v.parse::<usize>().ok()) {
                let base = self.scenes.clone();
                for k in 0..n.saturating_sub(self.scenes.len()) {
                    let mut sc = base[k % base.len()].clone();
                    sc.name = if k % 3 == 0 { format!("{} with the whole panel, take {}", sc.name, k + 2) } else { format!("{} {}", sc.name, k + 2) };
                    self.scenes.push(sc);
                }
            }
        }
        if step == 3 && self.test_drop.is_none() {
            // A source on air losing its signal (CUTAWAY_UI_TEST_DROP=2), and where to cut (…_DROP_TO=scene).
            if let Some(i) = std::env::var("CUTAWAY_UI_TEST_DROP").ok().and_then(|v| v.parse::<usize>().ok()).filter(|i| *i >= 1) {
                if let (Ok(to), Some(src)) = (std::env::var("CUTAWAY_UI_TEST_DROP_TO"), self.cfg.sources.get_mut(i - 1)) {
                    src.on_drop_scene = to;
                }
                self.test_drop = Some(i - 1);
            }
        }
        if step >= 3 && std::env::var_os("CUTAWAY_UI_TEST_HOLD").is_some() {
            // Held part way: the fill shows, but it never gets to stop the take.
            self.test_hold = true;
            if self.rec_hold.is_some_and(|h| h.elapsed() > Duration::from_millis(600)) {
                self.rec_hold = Some(Instant::now() - Duration::from_millis(600));
            }
        }
        let pip = self.scenes.iter().position(|s| s.name == "Picture in picture").unwrap_or(0);
        let next = match step {
            0 if t > 2.0 => {
                // A show chosen (takes are named after it).
                if let Ok(show) = std::env::var("CUTAWAY_UI_TEST_SHOW") {
                    self.cfg.ui.profile = show;
                    if let Some(e) = &self.engine {
                        let _ = e.update_config(&self.cfg);
                    }
                }
                self.press_scene(pip, false);
                if std::env::var_os("CUTAWAY_UI_TEST_PREVIEW").is_some() {
                    // Picture in picture on air, the third scene waiting in preview.
                    self.cfg.transition.preview_mode = true;
                    self.press_scene(pip, false);
                    self.take_preview(true);
                    self.press_scene(2, false);
                }
                // A multiview tile picked with Preview off (CUTAWAY_UI_TEST_PICK=4).
                if let Some(i) = std::env::var("CUTAWAY_UI_TEST_PICK").ok().and_then(|v| v.parse::<usize>().ok()) {
                    self.mv_selected = i.checked_sub(1);
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
                if let Ok(i) = std::env::var("CUTAWAY_UI_TEST_SPLIT_PAGE").map(|v| v.parse::<usize>().unwrap_or(0)) {
                    // SPLIT_PAGE=n: source n split in two on Sources › Close-ups & PTZ, waiting for Apply.
                    self.go("sources/closeups/split");
                    if let Some(win) = self.settings.as_mut() {
                        win.split_source(i, 2);
                    }
                }
                if std::env::var_os("CUTAWAY_UI_TEST_LIVE_EDIT").is_some() {
                    // LIVE_EDIT=1: Sources › Cameras with the first source mirrored, as a click there does (saved as you go).
                    self.go("sources/cameras/list");
                    if let Some(win) = self.settings.as_mut() {
                        win.edit_draft(|c| {
                            if let Some(s) = c.sources.first_mut() {
                                s.look.mirror = true;
                            }
                        });
                    }
                }
                if std::env::var_os("CUTAWAY_UI_TEST_SPLIT").is_some() {
                    // Split the first camera in two and apply, as the Split… button does.
                    let cfg = self.new_settings().with_split(0, 2).draft_config();
                    let _ = self.apply_settings(cfg, ctx);
                }
                if std::env::var_os("CUTAWAY_UI_TEST_EDIT").is_some() {
                    self.edit_mode = true;
                    self.selected_layer = Some(1);
                    // EDIT=reorder: picture in picture dragged to the front of the bank.
                    if std::env::var("CUTAWAY_UI_TEST_EDIT").is_ok_and(|v| v == "reorder") {
                        self.move_scene(pip, 0);
                    }
                    // EDIT=move: picture in picture's inset moved, not saved yet (in preview, with Preview on).
                    if std::env::var("CUTAWAY_UI_TEST_EDIT").is_ok_and(|v| v == "move") {
                        if self.preview_mode() {
                            self.press_scene(pip, false);
                        }
                        if let Some(mut sc) = self.edit_target().filter(|s| s.layers.len() > 1) {
                            (sc.layers[1].x, sc.layers[1].y) = (0.04, 0.06);
                            self.remember(undo::Kind::Scenes, "Move Cam 2");
                            self.apply_edit(sc);
                        }
                    }
                }
                if std::env::var_os("CUTAWAY_UI_TEST_CHECK").is_some() {
                    self.start_precheck(None);
                }
                // The status light's panel, on Now (HEALTH) or the Log (LOG).
                if std::env::var_os("CUTAWAY_UI_TEST_HEALTH").is_some() {
                    self.shell.open_status = Some(shell::StatusTab::Now);
                }
                if std::env::var_os("CUTAWAY_UI_TEST_LOG").is_some() {
                    self.shell.open_status = Some(shell::StatusTab::Log);
                }
                // An area (CUTAWAY_UI_TEST_AREA=record/streaming/where).
                if let Ok(place) = std::env::var("CUTAWAY_UI_TEST_AREA") {
                    match nav::AREAS.iter().position(|a| a.id == place) {
                        Some(i) => self.go_area(i),
                        None => self.go(&place),
                    }
                }
                if std::env::var_os("CUTAWAY_UI_TEST_COMMENTS").is_some() {
                    // A few viewer comments, as if from a live chat.
                    let chat = crate::net::chat::Chat::start(&Default::default(), self.waker.clone());
                    for (who, text) in [("maya_k", "Love the new set!"), ("streamfan22", "What mic are you using? It sounds great"), ("Sam O", "Hi from Lisbon 👋")] {
                        chat.inject(crate::net::chat::Platform::Twitch, who, text);
                    }
                    self.chat = Some(chat);
                }
                if std::env::var_os("CUTAWAY_UI_TEST_ARRANGE").is_some() {
                    self.column.arrange = true;
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
                if std::env::var_os("CUTAWAY_UI_TEST_NAMES").is_some() {
                    // A who's-on-air title, with no names set on the sources.
                    self.cfg.graphics.push(crate::config::Graphic::names("Names"));
                }
                if std::env::var_os("CUTAWAY_UI_TEST_REMOTE").is_some() {
                    // Remote control on with a PIN, for a screenshot of its page.
                    self.cfg.remote.enabled = true;
                    self.cfg.remote.pin = "4821".into();
                }
                let source = std::env::var("CUTAWAY_UI_TEST_SOURCE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut win = self.new_settings().open_at(section).select_source(source);
                // SCENE=n picks a scene in Settings → Scenes; SCENE_DELETE=1 asks to delete it.
                if let Some(n) = std::env::var("CUTAWAY_UI_TEST_SCENE").ok().and_then(|v| v.parse::<usize>().ok()) {
                    win = win.select_scene(n, std::env::var_os("CUTAWAY_UI_TEST_SCENE_DELETE").is_some());
                }
                self.settings = Some(win);
                // FIND=query: ⌘K search open with it typed (FINDGO: gone to its first result).
                if let Ok(q) = std::env::var("CUTAWAY_UI_TEST_FIND") {
                    self.sync_shell();
                    self.search_for(&q, std::env::var_os("CUTAWAY_UI_TEST_FINDGO").is_some());
                }
                4
            }
            // Stop, to show the "Take saved" card.
            3 if t > 7.0 && std::env::var_os("CUTAWAY_UI_TEST_STOP").is_some() => {
                // Two named markers, then stopped straight away (the key would ask first).
                for name in ["Guest arrives", "Big laugh"] {
                    let _ = self.add_marker(Some(name.into()));
                }
                if let Some(e) = &self.engine {
                    e.stop_recording();
                }
                5
            }
            // STOP=takes: then Record › Takes.
            5 if t > 11.0 && std::env::var("CUTAWAY_UI_TEST_STOP").is_ok_and(|v| v == "takes") => {
                self.go("record/takes/takes");
                6
            }
            // STOP=resolve: then its Open in Resolve.
            5 if t > 11.0 && std::env::var("CUTAWAY_UI_TEST_STOP").is_ok_and(|v| v == "resolve") => {
                // (Once the take is saved.)
                let folder = self.engine.as_ref().and_then(|e| e.status.lock_ok().last_take.as_ref().map(|r| r.folder.clone()));
                match folder {
                    Some(f) => {
                        self.take_action(takes::TakeAction::Resolve(f), ctx);
                        6
                    }
                    None => 5,
                }
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
        // Takes, counted as they start (from any button, key or remote).
        let recording = e.is_recording();
        if recording && !self.was_recording {
            self.takes_made += 1;
            // What these settings were expected to write, for the
            // Recording page's "writes about X/hour".
            self.take_rate_estimate = Some(crate::engine::health::estimate_for(&self.cfg));
        }
        self.was_recording = recording;
        let failed = e.status.lock_ok().rec_failed.take();
        if let Some(f) = failed {
            if e.is_recording() {
                e.stop_recording();
            }
            self.rec_problem = Some((f.to_string(), Instant::now()));
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
        self.watch_dropped_sources();
        let Some(e) = &self.engine else { return };
        // A sharper monitor picture while a second screen shows it.
        let want = if self.cfg.display.output != DisplayOutput::Off { e.info.canvas.0.min(1920) } else { self.cfg.program.preview_width };
        e.preview_width.store(want, std::sync::atomic::Ordering::Relaxed);
    }

    /// A source on air that loses its signal: one error message, and (if
    /// its settings say so) a cut to a scene that doesn't need it, once.
    fn watch_dropped_sources(&mut self) {
        let Some(e) = &self.engine else { return };
        let names: Vec<String> = e.sources.iter().map(|s| s.name.clone()).collect();
        // Offline as it's meant everywhere (numbers audit #29).
        self.offline_sources = (0..e.sources.len()).map(|i| e.source_offline(i)).collect();
        if let Some(o) = self.test_drop.and_then(|i| self.offline_sources.get_mut(i)) {
            *o = true;
        }
        let now: HashSet<String> = names.iter().zip(&self.offline_sources).filter(|(_, off)| **off).map(|(n, _)| n.clone()).collect();
        let Some(before) = self.dropped.replace(now.clone()) else { return };
        let live = e.switcher.live();
        let on_air = live.sources();
        let mut dropped = Vec::new();
        for (i, name) in names.iter().enumerate() {
            if now.contains(name) && !before.contains(name) && on_air.contains(&i) {
                self.toast(format!("{name} dropped while on air: no signal"), true);
                dropped.push(name.clone());
            }
        }
        if let Some(scene) = cut_after_drop(&dropped, &self.cfg.sources, &self.scenes, &self.offline_sources, &live.name) {
            self.take_scene(&scene, true);
            self.toast(format!("Cut to {}", scene.name), false);
        }
    }

    /// Each source a scene shows that has no signal now.
    fn scene_offline(&self, scene: &Scene) -> Vec<usize> {
        scene.sources().into_iter().filter(|i| self.offline_sources.get(*i).copied().unwrap_or(false)).collect()
    }

    /// The right-hand column's edge: a grip to show it can be dragged, and
    /// its width kept (with the show) once it's let go.
    fn column_edge(&mut self, ctx: &egui::Context, rect: Rect) {
        let edge = Rect::from_center_size(pos2(rect.left(), rect.center().y), vec2(6.0, 36.0));
        let near = ctx.input(|i| i.pointer.hover_pos().is_some_and(|p| (p.x - rect.left()).abs() < 6.0 && rect.y_range().contains(p.y)));
        let painter = ctx.layer_painter(egui::LayerId::background());
        for k in -1..=1 {
            painter.circle_filled(edge.center() + vec2(0.0, k as f32 * 7.0), 1.6, if near { ACCENT } else { LINE });
        }
        // Only a drag of the edge changes it (not the window getting smaller).
        if near && ctx.input(|i| i.pointer.primary_pressed()) {
            self.column_dragged = true;
        }
        if !self.column_dragged || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.column_dragged = false;
        let w = rect.width().round();
        if (w - self.cfg.ui.column_width).abs() >= 1.0 {
            self.cfg.ui.column_width = w;
            self.cfg_dirty = true;
        }
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

    /// The show (profile) picker, under the logo at the top of the rail
    /// (`width`: what it has).
    fn show_menu(&mut self, ui: &mut egui::Ui, width: f32) {
        let Some(path) = self.save_path.clone() else { return };
        let dir = crate::config::profiles_dir(&path);
        let current = self.cfg.ui.profile.clone();
        let name = if current.is_empty() { "No show".to_string() } else { current.clone() };
        // As much of the name as fits, then ▾; all of it on hover.
        let font = FontId::proportional(TEXT_SMALL);
        let fits = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x <= width - 8.0;
        let mut short = format!("{name} ▾");
        let mut keep = name.chars().count();
        while !fits(&short) && keep > 1 {
            keep -= 1;
            short = format!("{}… ▾", name.chars().take(keep).collect::<String>().trim_end());
        }
        let r = ui.menu_button(RichText::new(short).size(TEXT_SMALL).color(TEXT_DIM), |ui| {
            ui.label(RichText::new(format!("Show: {name}")).strong());
            ui.separator();
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
            if ui.button("Save as a show…").on_hover_text("More › App › Shows").clicked() {
                self.open_page(Section::General, None);
                ui.close();
            }
        });
        r.response.on_hover_text(format!("{name}: switch shows, or save this one as a show"));
    }

    /// The name this setup is saved under before switching shows, when it
    /// isn't a show yet: "My setup", or "My setup 2" and so on if taken.
    fn unnamed_setup_name(&self) -> Option<String> {
        if !self.cfg.ui.profile.trim().is_empty() {
            return None;
        }
        let dir = crate::config::profiles_dir(self.save_path.as_ref()?);
        let taken = profiles::list(&dir);
        (1..).map(|i| if i == 1 { "My setup".to_string() } else { format!("My setup {i}") }).find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
    }

    fn switch_show(&mut self, name: &str, ctx: &egui::Context) {
        let Some(path) = &self.save_path else { return };
        let dir = crate::config::profiles_dir(path);
        if self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) {
            self.toast("Stop recording and streaming before switching shows".into(), true);
            return;
        }
        // Keep the current show's latest changes first; a setup that isn't
        // a show yet is saved as one, so switching never loses it.
        let unnamed = self.unnamed_setup_name();
        if let Some(n) = &unnamed {
            self.cfg.ui.profile = n.clone();
        }
        if !self.cfg.ui.profile.trim().is_empty() {
            self.cfg.scenes = self.scenes.clone();
            if let Err(e) = profiles::save(&dir, &self.cfg.ui.profile, &self.cfg) {
                if unnamed.is_some() {
                    self.cfg.ui.profile.clear();
                    self.toast(format!("Couldn't save this setup before switching: {e:#}"), true);
                    return;
                }
            }
        }
        match profiles::load(&dir, name) {
            Ok(mut cfg) => {
                let before = self.snapshot(undo::Kind::Settings, format!("Switch to \"{name}\""));
                // Its own scenes (settings without any would keep this show's).
                if cfg.scenes.is_empty() {
                    cfg.scenes = cfg.effective_scenes();
                }
                match self.apply_settings_as(cfg, ctx, true) {
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

    /// The same report as Settings → Diagnostics, for when the engine won't start.
    fn save_problem_report(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Zip", &["zip"]).set_file_name(crate::report::file_name()).save_file() else { return };
        let facts = self.settings_context().engine_facts;
        let devices: Vec<String> = crate::devices::list(crate::devices::Kind::Video)
            .iter()
            .map(|d| format!("Video: {} {}", d.name, d.summary))
            .chain(crate::devices::list(crate::devices::Kind::Audio).iter().map(|d| format!("Audio: {} {}", d.name, d.summary)))
            .collect();
        let mut cfg = self.cfg.clone();
        cfg.scenes = self.scenes.clone();
        let report = crate::report::Report { cfg: &cfg, facts: &facts, devices: &devices, log: crate::logging::path() };
        match report.write(&path) {
            Ok(()) => {
                crate::logging::reveal_file(&path);
                self.toast(format!("Saved {}. Send it by email or attach it to your message.", path.display()), false)
            }
            Err(e) => self.toast(format!("Couldn't save the report: {e:#}"), true),
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
                ui.label(RichText::new("The engine couldn't start").size(TEXT_TITLE).color(Color32::WHITE));
                ui.add_space(8.0);
                ui.label(RichText::new(err).color(AMBER));
                ui.add_space(16.0);
                if ui.button("Open settings").clicked() {
                    self.open_settings();
                }
                if ui.button("Try again").clicked() {
                    self.start_engine();
                }
                if ui.button("Save a problem report…").on_hover_text("One .zip with the log, your settings and devices, to send when asking for help").clicked() {
                    self.save_problem_report();
                }
            });
            return;
        }
        let Some(engine) = &self.engine else { return };
        let (cw, ch) = engine.info.canvas;
        let canvas_aspect = cw as f32 / ch as f32;
        let n = engine.sources.len();
        if self.edit_mode {
            // With Preview on, Edit layout works on the scene in preview:
            // the one on air (as saved) goes there if nothing is.
            if self.preview_mode() && engine.switcher.preview().is_none() {
                let live = engine.switcher.live();
                let mut s = self.scenes.iter().find(|s| s.name == live.name).cloned().unwrap_or(live);
                s.retain_sources(n);
                engine.switcher.set_preview(Some(s));
            }
            self.edit_toolbar(ui);
            ui.add_space(6.0);
        }
        let avail = ui.available_rect_before_wrap();
        ui.allocate_rect(avail, Sense::hover());
        let preview_mode = self.preview_mode();
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
        if tall_aspect.is_some() && !(self.edit_mode && !preview_mode) {
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
            painter.text(rect.center(), Align2::CENTER_CENTER, "Click a scene to preview it", FontId::proportional(TEXT_STATE), TEXT_DIM);
        }
        painter.rect_stroke(rect, 4.0, Stroke::new(2.0, GREEN), StrokeKind::Outside);
        // (At the bottom while editing, out of the way of the frames' names.)
        let at = if self.edit_mode { pos2(rect.left() + 10.0, rect.bottom() - 32.0) } else { rect.min + vec2(10.0, 10.0) };
        let badge = Rect::from_min_size(at, vec2(86.0, 24.0));
        painter.rect_filled(badge, 3.0, GREEN_DIM);
        bold_text(&painter, badge.center(), Align2::CENTER_CENTER, "PREVIEW", TEXT_BODY, Color32::WHITE);
        if let Some(p) = pv {
            if self.edit_mode {
                self.edit_overlay(ui, rect, p);
            } else {
                label_chip(&painter, badge.right_center() + vec2(12.0, 0.0), &p.name);
            }
        }
        // Take / cut under the preview (over the frames being edited).
        let take = Rect::from_min_size(pos2(rect.right() - 170.0, rect.bottom() - 46.0), vec2(80.0, 36.0));
        let cut = Rect::from_min_size(pos2(rect.right() - 86.0, rect.bottom() - 46.0), vec2(76.0, 36.0));
        let rt = ui.interact(take, ui.scope_id().with("take_btn"), Sense::click());
        let rc = ui.interact(cut, ui.scope_id().with("cut_btn"), Sense::click());
        painter.rect_filled(take, 6.0, if rt.hovered() { TAKE_HOVER } else { TAKE });
        painter.text(take.center(), Align2::CENTER_CENTER, "TAKE", FontId::proportional(TEXT_STATE), Color32::WHITE);
        painter.rect_filled(cut, 6.0, if rc.hovered() { NEUTRAL_HOVER } else { RAISED });
        painter.text(cut.center(), Align2::CENTER_CENTER, "CUT", FontId::proportional(TEXT_STATE), Color32::WHITE);
        if rt.on_hover_text(format!("{} to program{}", self.cfg.transition.mode.label(), self.key_hint(Builtin::Take))).clicked() {
            self.take_preview(false);
        }
        if rc.on_hover_text(format!("Straight to program{}", self.key_hint(Builtin::CutPreview))).clicked() {
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
        painter.rect_stroke(rect, 4.0, Stroke::new(if recording { 2.0 } else { 1.0 }, if recording { ON_AIR } else { ON_AIR_DIM }), StrokeKind::Outside);
        let badge = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(26.0, 18.0));
        painter.rect_filled(badge, 3.0, Color32::from_black_alpha(170));
        painter.text(badge.center(), Align2::CENTER_CENTER, "9:16", FontId::proportional(TEXT_SMALL), Color32::WHITE);
        let r = ui.interact(rect, ui.scope_id().with("tall_monitor"), Sense::click());
        if r.on_hover_text("The vertical version (click to set its layouts and framing in Record › Vertical version)").clicked() {
            self.go("record/vertical/vertical");
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
        painter.rect_stroke(rect, 4.0, Stroke::new(if recording { 3.0 } else { 1.5 }, if recording { ON_AIR } else { ON_AIR_DIM }), StrokeKind::Outside);
        let live = engine.switcher.live();
        if self.edit_mode && !self.preview_mode() {
            return self.edit_overlay(ui, rect, live);
        }
        let badge = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(86.0, 24.0));
        painter.rect_filled(badge, 3.0, ON_AIR);
        bold_text(&painter, badge.center(), Align2::CENTER_CENTER, "PROGRAM", TEXT_BODY, Color32::WHITE);
        let name_chip = label_chip(&painter, badge.right_center() + vec2(12.0, 0.0), &live.name);
        // Status chips after the scene name (the top right is where logos usually sit).
        let mut x = name_chip.right() + 8.0;
        let auto = engine.auto_switch_enabled();
        let auto_st = engine.status.lock_ok().auto_switch.clone();
        let mut chips: Vec<(String, Color32)> = Vec::new();
        // A source on air with no signal, first.
        for i in self.scene_offline(&live) {
            if let Some(src) = engine.sources.get(i) {
                let how = if src.input.self_managed() { "no signal" } else { "no signal, reconnecting…" };
                chips.push((format!("⚠ {}: {how}", src.name), ERROR));
            }
        }
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
            let galley = painter.layout_no_wrap(text, FontId::proportional(TEXT_BODY), Color32::WHITE);
            let r = Rect::from_min_size(pos2(x, badge.center().y - 12.0), vec2(galley.size().x + 14.0, 24.0));
            painter.rect_filled(r, 3.0, color);
            painter.galley(r.min + vec2(7.0, 4.0), galley, Color32::WHITE);
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
                painter.text(tag.left_center(), Align2::LEFT_CENTER, "CC · stream only", FontId::proportional(TEXT_SMALL), Color32::from_white_alpha(170));
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
                self.apply_edit(scene.clone());
            }
        }
        if resp.double_clicked() {
            if let Some(i) = pointer.and_then(|p| hit_at(&scene.layers, p)) {
                let name = names.get(scene.layers[i].source).cloned().unwrap_or_default();
                self.remember(undo::Kind::Scenes, if scene.layers[i].fill { format!("Fit {name}") } else { format!("Fill with {name}") });
                scene.layers[i].fill = !scene.layers[i].fill;
                self.selected_layer = Some(i);
                self.apply_edit(scene.clone());
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
                self.apply_edit(scene.clone());
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
            let tag = painter.text(r.min + vec2(6.0, 5.0), Align2::LEFT_TOP, &name, FontId::proportional(TEXT_BODY), Color32::WHITE);
            painter.rect_filled(tag.expand(3.0), 2.0, Color32::from_black_alpha(150));
            painter.text(r.min + vec2(6.0, 5.0), Align2::LEFT_TOP, &name, FontId::proportional(TEXT_BODY), Color32::WHITE);
            let corner = Rect::from_min_max(r.right_bottom() - vec2(handle, handle), r.right_bottom());
            painter.rect_filled(corner, 2.0, color);
            if l.crops() {
                let text = match (l.fill, l.zoom > 1.0001) {
                    (true, true) => format!("FILL · {:.1}×", l.zoom),
                    (true, false) => "FILL".to_string(),
                    _ => format!("{:.1}×", l.zoom),
                };
                let at = pos2(r.right() - 6.0, r.top() + 5.0);
                let tag = painter.text(at, Align2::RIGHT_TOP, &text, FontId::proportional(TEXT_SMALL), Color32::WHITE);
                painter.rect_filled(tag.expand(3.0), 2.0, Color32::from_black_alpha(150));
                painter.text(at, Align2::RIGHT_TOP, &text, FontId::proportional(TEXT_SMALL), Color32::WHITE);
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

    /// The scene Edit layout works on: the one in preview with Preview on
    /// (the program stays as it is until Take), else the one on air.
    fn edit_target(&self) -> Option<Scene> {
        let e = self.engine.as_ref()?;
        if self.preview_mode() {
            e.switcher.preview()
        } else {
            Some(e.switcher.live())
        }
    }

    /// Show an edit where it's being made: in preview, or on air.
    fn show_edit(&self, scene: Scene) {
        if let Some(e) = &self.engine {
            if self.preview_mode() {
                e.switcher.set_preview(Some(scene));
            } else {
                e.switcher.set_live(scene);
            }
        }
    }

    fn apply_edit(&mut self, scene: Scene) {
        self.show_edit(scene.clone());
        if let Some(s) = self.scenes.iter_mut().find(|s| s.name == scene.name) {
            *s = scene;
            self.scenes_dirty = true;
        }
    }

    fn edit_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else { return };
        let Some(mut scene) = self.edit_target() else { return };
        let in_preview = self.preview_mode();
        let names: Vec<String> = engine.sources.iter().map(|s| s.name.clone()).collect();
        let inputs: Vec<String> = engine.audio.iter().map(|a| a.name.clone()).collect();
        let cue_suggestions = control::suggestions(&self.cfg);
        let mut changed = false;
        let mut new_scene: Option<Scene> = None;
        // ＋ New scene: blank or from a layout.
        let mut fresh: Option<Scene> = None;
        let mut delete = false;
        let (mut undo_clicked, mut redo_clicked) = (false, false);
        let old_name = scene.name.clone();
        let mut rename_clash = None;
        // The keys as they're set now (numbers audit #22).
        let (undo_key, redo_key) = (self.key_hint(Builtin::Undo), self.key_hint(Builtin::Redo));
        // A heading, what's being edited, and Undo / Save / Done.
        ui.horizontal(|ui| {
            ui.label(RichText::new("EDIT LAYOUT").color(AMBER).strong());
            let (what, tip) = if in_preview {
                (format!("{} in preview: the program stays as it is until you Take", scene.name), "With Preview on you edit the next shot. Take (or Cut) puts it on air.")
            } else {
                (format!("{} on air: changes show at once", scene.name), "Turn on Preview to edit a scene without it showing in the recording or stream until you Take it.")
            };
            ui.label(RichText::new(what).color(if in_preview { GREEN } else { Color32::WHITE }).size(TEXT_BODY)).on_hover_text(tip);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    self.edit_mode = false;
                }
                if ui.add_enabled(self.scenes_dirty || self.scene_name_edit.is_some(), egui::Button::new("Save scenes")).on_hover_text("Write the scenes to the settings file now (they're also saved a few seconds after Done)").clicked() {
                    self.commit_scene_name();
                    self.save_config();
                }
                if self.scenes_dirty {
                    let chip = egui::Button::new(RichText::new("Unsaved").size(TEXT_BODY).color(Color32::BLACK)).fill(AMBER).sense(Sense::hover());
                    ui.add(chip).on_hover_text("Changes to scenes not written to disk yet");
                }
                if ui.add_enabled(self.history.can_redo().is_some(), egui::Button::new("↷ Redo")).on_hover_text(match self.history.can_redo() {
                    Some(l) => format!("Redo: {l}{redo_key}"),
                    None => format!("Nothing to redo{redo_key}"),
                }).clicked() {
                    redo_clicked = true;
                }
                if ui.add_enabled(self.history.can_undo().is_some(), egui::Button::new("↶ Undo")).on_hover_text(match self.history.can_undo() {
                    Some(l) => format!("Undo: {l}{undo_key}"),
                    None => format!("Nothing to undo{undo_key}"),
                }).clicked() {
                    undo_clicked = true;
                }
            });
        });
        // The two groups side by side when they fit (as measured last time), else one under the other.
        let widths_id = egui::Id::unique("edit_group_widths");
        let widths: [f32; 2] = ui.ctx().data(|d| d.get_temp(widths_id)).unwrap_or([f32::INFINITY; 2]);
        let one_row = widths[0] + widths[1] + ui.spacing().item_spacing.x <= ui.available_width();
        let mut measured = [0.0; 2];
        ui.horizontal_wrapped(|ui| {
            // Frame: the selected frame and the picture in it.
            let sel = self.selected_layer.filter(|&i| i < scene.layers.len());
            measured[0] = tool_group(ui, "FRAME", |ui| {
                egui::ComboBox::from_id_salt("add_layer").selected_text("Add source").show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        // A source shows once per scene (the program has one
                        // picture of each).
                        let there = scene.layers.iter().any(|l| l.source == i);
                        if ui.add_enabled(!there, egui::Button::selectable(false, n)).on_disabled_hover_text("Already in this scene").clicked() {
                            scene.layers.push(Layer::at(i, 0.3, 0.3, 0.4, 0.4));
                            changed = true;
                        }
                    }
                });
                if ui.add_enabled(sel.is_some(), egui::Button::new("Remove")).on_disabled_hover_text("Click a frame first").clicked() {
                    scene.layers.remove(sel.unwrap());
                    self.selected_layer = None;
                    changed = true;
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("To front")).on_disabled_hover_text("Click a frame first").clicked() {
                    let l = scene.layers.remove(sel.unwrap());
                    scene.layers.push(l);
                    self.selected_layer = Some(scene.layers.len() - 1);
                    changed = true;
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("Full frame")).on_disabled_hover_text("Click a frame first").clicked() {
                    let l = &mut scene.layers[sel.unwrap()];
                    (l.x, l.y, l.w, l.h) = (0.0, 0.0, 1.0, 1.0);
                    changed = true;
                }
                // The picture in the selected frame: fitted or filling, and zoom
                // (always there, so the bar doesn't change size on a click).
                let mut none = Layer::at(0, 0.0, 0.0, 1.0, 1.0);
                ui.add_enabled_ui(sel.is_some(), |ui| {
                    let l = match sel {
                        Some(i) => &mut scene.layers[i],
                        None => &mut none,
                    };
                    if ui.selectable_label(!l.fill && sel.is_some(), "Fit").on_hover_text("Show the whole picture (bars where the frame is another shape)").clicked() {
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
                });
            });
            if !one_row {
                ui.end_row();
            }
            // Scene: its name, sound and cues, and the scene itself.
            measured[1] = tool_group(ui, "SCENE", |ui| {
                let mut name = match &self.scene_name_edit {
                    Some((of, text)) if *of == old_name => text.clone(),
                    _ => scene.name.clone(),
                };
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(130.0)).on_hover_text("The scene's name");
                if r.changed() {
                    self.scene_name_edit = Some((old_name.clone(), name.clone()));
                }
                if r.lost_focus() {
                    if let Some((_, text)) = self.scene_name_edit.take().filter(|(of, _)| *of == old_name) {
                        let text = text.trim();
                        if text.is_empty() || text == old_name {
                            // Nothing to do: the old name stays.
                        } else if self.scenes.iter().any(|s| s.name != old_name && s.name.eq_ignore_ascii_case(text)) {
                            rename_clash = Some(text.to_string());
                        } else {
                            scene.name = text.to_string();
                            changed = true;
                        }
                    }
                }
                // Scene audio: inputs this scene turns on or off.
                let n_audio = scene.audio_on.len() + scene.audio_off.len();
                ui.menu_button(if n_audio > 0 { format!("Audio ({n_audio})") } else { "Audio".into() }, |ui| {
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
                    ui.label(RichText::new("e.g. a break scene that mutes the mics and brings up the music. Changes land with the picture and fade with mixes.").size(TEXT_SMALL).color(TEXT_DIM));
                })
                .response
                .on_hover_text("Scene audio: inputs this scene turns on or off");
                // Scene cues: commands run when the scene is taken.
                let n_cues = scene.cues.len();
                ui.menu_button(if n_cues > 0 { format!("Cues ({n_cues})") } else { "Cues".into() }, |ui| {
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
                    ui.label(RichText::new("e.g. start a Syrinscape mood, play a pad, show a title. Pick from ＋ Add; ✕ takes one off. Several run in order.").size(TEXT_SMALL).color(TEXT_DIM));
                })
                .response
                .on_hover_text("Scene cues: commands run when this scene is taken");
                if let Some(s) = crate::settings::new_scene_menu(ui, &names, &self.scenes) {
                    fresh = Some(s);
                }
                if ui.button("Duplicate").on_hover_text("A copy of this scene, to change").clicked() {
                    let mut s = scene.clone();
                    s.name = unique_name(&self.scenes, &format!("{} copy", scene.name));
                    new_scene = Some(s);
                }
                if ui.add_enabled(self.scenes.len() > 1, egui::Button::new("Delete")).on_hover_text("Delete this scene (Undo brings it back)").on_disabled_hover_text("The last scene can't go").clicked() {
                    delete = true;
                }
            });
        });
        ui.ctx().data_mut(|d| d.insert_temp(widths_id, measured));
        ui.label(RichText::new(if cfg!(target_os = "macos") { "Drag frames · corner to resize · Shift keeps shape · scroll to zoom · double-click to fill · Option-drag moves the picture" } else { "Drag frames · corner to resize · Shift keeps shape · scroll to zoom · double-click to fill · Alt-drag moves the picture" }).color(TEXT_DIM).size(TEXT_SMALL));
        if let Some(n) = rename_clash {
            self.toast(format!("There's already a scene called {n}"), true);
        }
        if let Some(s) = fresh {
            // Added at the end, so no scene's number changes. With Preview on
            // it goes in preview to edit; never on air.
            self.remember(undo::Kind::Scenes, format!("New scene {}", s.name));
            self.scenes.push(s.clone());
            self.scenes_dirty = true;
            if in_preview {
                self.show_edit(s);
                self.selected_layer = None;
            } else {
                self.toast(format!("{} is at the end of the scenes: take it to edit it", s.name), false);
            }
        }
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
        let before = self.scene_names();
        if changed {
            self.show_edit(scene.clone());
            if let Some(s) = self.scenes.iter_mut().find(|s| s.name == old_name) {
                *s = scene;
            }
            self.scenes_dirty = true;
        }
        if let Some(s) = new_scene {
            self.scenes.push(s.clone());
            self.scenes_dirty = true;
            self.show_edit(s);
        }
        if delete && self.scenes.len() > 1 {
            self.scenes.retain(|s| s.name != old_name);
            self.scenes_dirty = true;
            if in_preview {
                // The next scene to edit goes in preview; the program carries on.
                if let (Some(e), Some(mut s)) = (&self.engine, self.scenes.first().cloned()) {
                    s.retain_sources(e.sources.len());
                    e.switcher.set_preview(Some(s));
                }
                self.selected_layer = None;
            } else if let Some(s) = self.scenes.first().cloned() {
                self.take_scene(&s, true);
            }
        }
        self.scenes_moved(&before);
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
        let mut picked: Option<(usize, TilePick)> = None;
        let mut menu_hit = false;
        let mut put_on_air: Option<usize> = None;
        let preview_mode = self.preview_mode();
        if self.mv_selected.is_some_and(|i| i >= n || preview_mode) {
            self.mv_selected = None;
        }
        // Sources that can be split into close-ups (not ones cut from another).
        let splittable: Vec<bool> = (0..n).map(|i| self.cfg.sources.get(i).is_some_and(|s| s.framed_from().is_none() && s.virtual_ptz_of().is_none())).collect();
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
            let selected = !preview_mode && self.mv_selected == Some(i);
            let bar = Rect::from_min_size(pic.left_bottom(), vec2(tile_w, label_h));
            painter.rect_filled(bar, 0.0, if on_air { ON_AIR } else if in_preview { GREEN_DIM } else { RAISED });
            // ⋯ at the right of the bar: what right-click offers, where it
            // can be seen (forensics multiview #2).
            let mut right = bar.right() - 2.0;
            let player_state = players[i].as_ref().map(|p| p.state.lock_ok().clone());
            let framing_cfg = framers[i].as_ref().map(|f| f.cfg.lock_ok().clone());
            let can_split = player_state.is_none() && framing_cfg.is_none() && splittable.get(i).copied().unwrap_or(false);
            let has_menu = player_state.is_some() || framing_cfg.is_some() || can_split;
            if has_menu {
                let br = Rect::from_min_size(pos2(right - 24.0, bar.top() + 1.0), vec2(24.0, label_h - 2.0));
                let dots = ui.interact(br, ui.scope_id().with(("tile_menu", i)), Sense::click());
                painter.rect_filled(br, 3.0, if dots.hovered() { LINE } else { Color32::TRANSPARENT });
                painter.text(br.center(), Align2::CENTER_CENTER, "⋯", FontId::proportional(TEXT_STATE), Color32::WHITE);
                if dots.clicked() {
                    menu_hit = true;
                }
                egui::Popup::menu(&dots).show(|ui| {
                    if let Some(p) = tile_menu(ui, framing_cfg.as_ref(), player_state.as_ref(), can_split) {
                        picked = Some((i, p));
                    }
                });
                dots.on_hover_text(if player_state.is_some() { "Playlist" } else if framing_cfg.is_some() { "Who to follow, and how tight" } else { "Split into close-ups" });
                right -= 26.0;
            }
            // Media player transport.
            if let Some(st) = &player_state {
                if st.duration > 0 {
                    let k = (st.position as f32 / st.duration as f32).clamp(0.0, 1.0);
                    let track = Rect::from_min_size(pic.left_bottom() - vec2(0.0, 4.0), vec2(tile_w, 4.0));
                    painter.rect_filled(track, 0.0, Color32::from_black_alpha(160));
                    painter.rect_filled(Rect::from_min_size(track.min, vec2(tile_w * k, 4.0)), 0.0, ACCENT);
                }
                for (glyph, cmd, tip) in [("⏭", PlayerCmd::Next, "Next"), (if st.playing { "⏸" } else { "▶" }, PlayerCmd::TogglePause, "Play / pause"), ("⟲", PlayerCmd::Restart, "From the start")] {
                    let br = Rect::from_min_size(pos2(right - 22.0, bar.top() + 1.0), vec2(22.0, label_h - 2.0));
                    let rr = ui.interact(br, ui.scope_id().with(("media", i, tip)), Sense::click());
                    painter.rect_filled(br, 3.0, if rr.hovered() { LINE } else { Color32::TRANSPARENT });
                    painter.text(br.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(TEXT_BODY), Color32::WHITE);
                    if rr.on_hover_text(tip).clicked() {
                        picked = Some((i, TilePick::Player(cmd)));
                    }
                    right -= 24.0;
                }
                if let Some(item) = st.items.get(st.index) {
                    painter.text(pic.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, item, FontId::proportional(TEXT_SMALL), Color32::from_white_alpha(200));
                }
            }
            // The source's name and who's in it: no number, so it can't be
            // taken for a scene key (forensics bank #2).
            let person = self.cfg.sources.get(i).map(|c| c.person.trim().to_string()).unwrap_or_default();
            let mut job = egui::text::LayoutJob::default();
            let body = FontId::proportional(TEXT_BODY);
            if person.is_empty() {
                job.append(name, 0.0, egui::TextFormat::simple(body, Color32::WHITE));
            } else if person.to_lowercase().starts_with(&name.to_lowercase()) {
                // "Maya" showing Maya Chen: the full name says both.
                job.append(&person, 0.0, egui::TextFormat::simple(body, Color32::WHITE));
            } else {
                job.append(name, 0.0, egui::TextFormat::simple(body.clone(), Color32::WHITE));
                job.append(&format!("  ·  {person}"), 0.0, egui::TextFormat::simple(body, if on_air { Color32::from_white_alpha(220) } else { TEXT_DIM }));
            }
            job.wrap = egui::text::TextWrapping::truncate_at_width((right - bar.left() - 12.0).max(10.0));
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(pos2(bar.left() + 8.0, bar.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
            // Auto-framing: who it's following.
            if let Some(f) = &framers[i] {
                let cfg = framing_cfg.clone().unwrap_or_default();
                let people = f.people.load(std::sync::atomic::Ordering::Relaxed);
                let text = if f.problem.lock_ok().is_some() {
                    "AUTO-FRAME OFF · whole picture".to_string()
                } else if people == 0 {
                    "AUTO · looking for people".to_string()
                } else {
                    format!("AUTO · {} · {}", cfg.shot.label(), cfg.follow.label())
                };
                let font = FontId::proportional(TEXT_SMALL);
                let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
                let chip = Rect::from_min_size(pic.left_top() + vec2(5.0, 5.0), galley.size() + vec2(10.0, 4.0));
                painter.rect_filled(chip, 3.0, Color32::from_black_alpha(150));
                painter.galley(chip.min + vec2(5.0, 2.0), galley, Color32::WHITE);
            }
            let stroke = if selected {
                Stroke::new(3.0, ACCENT)
            } else if on_air {
                Stroke::new(3.0, ON_AIR)
            } else if in_preview {
                Stroke::new(3.0, GREEN)
            } else if resp.hovered() {
                Stroke::new(1.5, ACCENT)
            } else {
                Stroke::new(1.0, LINE)
            };
            painter.rect_stroke(full, 3.0, stroke, StrokeKind::Outside);
            // Picked (Preview off): one button puts it on air with the transition.
            if selected {
                let size = vec2((tile_w - 16.0).min(120.0), 30.0);
                let br = Rect::from_min_size(pos2(pic.center().x - size.x / 2.0, pic.bottom() - size.y - 8.0), size);
                let rb = ui.interact(br, ui.scope_id().with(("put_on_air", i)), Sense::click());
                painter.rect_filled(br, 6.0, if rb.hovered() { TAKE_HOVER } else { TAKE });
                painter.text(br.center(), Align2::CENTER_CENTER, "Put on air", FontId::proportional(TEXT_STATE), Color32::WHITE);
                if rb.on_hover_text(format!("{} to {name} full screen", self.cfg.transition.mode.label())).clicked() {
                    put_on_air = Some(i);
                }
            }
            if resp.clicked() {
                clicked = Some((i, ui.input(|inp| inp.modifiers.shift)));
            }
            if has_menu {
                resp.context_menu(|ui| {
                    if let Some(p) = tile_menu(ui, framing_cfg.as_ref(), player_state.as_ref(), can_split) {
                        picked = Some((i, p));
                    }
                });
            }
            let base = if preview_mode {
                "Click: preview this source full screen. Shift+click: cut to it."
            } else {
                "Click: pick this source, then Put on air. Shift+click: cut to it now."
            };
            let tip = if player_state.is_some() {
                format!("{base}\n⋯ or right-click: playlist.")
            } else if framing_cfg.is_some() {
                format!("{base}\n⋯ or right-click: who to follow, and how tight.")
            } else if can_split {
                format!("{base}\n⋯ or right-click: split into close-ups.")
            } else {
                base.to_string()
            };
            resp.on_hover_text(tip);
        }
        if menu_hit || put_on_air.is_some() {
            clicked = None;
        }
        match picked {
            Some((i, TilePick::Framing(f))) => {
                if let Some(s) = self.cfg.sources.get_mut(i) {
                    s.framing = f;
                    self.cfg_dirty = true;
                }
                if let Some(e) = &self.engine {
                    let _ = e.update_config(&self.cfg);
                }
                clicked = None;
            }
            Some((i, TilePick::Player(cmd))) => {
                if let Some(p) = &players[i] {
                    p.send(cmd);
                }
                clicked = None;
            }
            Some((i, TilePick::Split(k))) => {
                // Sources › Close-ups & PTZ, with the close-ups made and waiting to be framed.
                self.go("sources/closeups/split");
                if let Some(win) = self.settings.as_mut() {
                    win.split_source(i, k);
                }
                clicked = None;
            }
            None => {}
        }
        if let Some(i) = put_on_air {
            self.mv_selected = None;
            self.show_source(i, false);
        }
        // With Preview off a click picks the tile (to look closer without
        // switching: forensics multiview #1); Shift+click still cuts.
        if let Some((i, cut)) = clicked {
            if cut || preview_mode {
                self.mv_selected = None;
                self.show_source(i, cut);
            } else {
                self.mv_selected = if self.mv_selected == Some(i) { None } else { Some(i) };
            }
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
        // Scenes that show a source with no signal: its names.
        let source_names: Vec<String> = self.engine.as_ref().map(|e| e.sources.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
        let no_signal: Vec<Vec<String>> = self.scenes.iter().map(|s| self.scene_offline(s).into_iter().filter_map(|i| source_names.get(i).cloned()).collect()).collect();
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("TRANSITION").color(TEXT_DIM).size(TEXT_SMALL));
                ui.horizontal(|ui| self.transition_row(ui));
                ui.horizontal(|ui| self.bank_toggles(ui));
            });
            ui.separator();
            // Every scene in view (forensics bank #1): one row of buttons
            // with pictures when they fit, else two rows of smaller ones
            // (name and key), or in a tall window two rows with pictures.
            // Only a long bank (more than BANK_FIT) scrolls, with "+n ›".
            let gap = ui.spacing().item_spacing.x;
            let gap_y = ui.spacing().item_spacing.y;
            let n = self.scenes.len();
            let width = ui.available_width();
            let layout = bank_layout(n, width, height, gap, gap_y, self.bank_rows > 1);
            self.bank_overflows = !bank_layout(n, width, height, gap, gap_y, false).thumbs;
            let rows = layout.rows;
            let per_row = n.div_ceil(rows).max(1);
            let mut area = egui::ScrollArea::horizontal().id_salt("bank");
            if let Some(x) = self.bank_scroll_to.take() {
                area = area.horizontal_scroll_offset(x);
            }
            let scenes = self.scenes.clone();
            let out = area.show(ui, |ui| {
                ui.vertical(|ui| {
                    for row in scenes.chunks(per_row).enumerate() {
                        ui.horizontal(|ui| {
                            for (j, s) in row.1.iter().enumerate() {
                                let i = row.0 * per_row + j;
                                let tags = (live_name.as_deref() == Some(s.name.as_str()), preview_name.as_deref() == Some(s.name.as_str()));
                                if let Some(p) = self.scene_button(ui, i, s, tags, no_signal.get(i), layout) {
                                    press = Some(p);
                                }
                            }
                        });
                    }
                });
            });
            // Scenes out of sight: a fade and an arrow at that edge (with how many), to scroll a page.
            let view = out.inner_rect;
            let (x, content) = (out.state.clamped_offset().x, out.content_size.x);
            let step = layout.button.x + gap;
            let hidden_left = ((x / step).floor().max(0.0) as usize * rows).min(n);
            let hidden_right = (((content - x - view.width()) / step).ceil().max(0.0) as usize * rows).min(n);
            let mut edge = |ui: &mut egui::Ui, left: bool, count: usize| {
                let w = 70.0;
                let r = if left { Rect::from_min_size(view.min, vec2(w, view.height())) } else { Rect::from_min_size(pos2(view.right() - w, view.top()), vec2(w, view.height())) };
                let resp = ui.interact(r, ui.scope_id().with(("bank_edge", left)), Sense::click());
                let p = ui.painter_at(r);
                let (solid, clear) = (PANEL, PANEL.gamma_multiply(0.0));
                let mut mesh = egui::Mesh::default();
                let (a, b) = if left { (solid, clear) } else { (clear, solid) };
                mesh.colored_vertex(r.left_top(), a);
                mesh.colored_vertex(r.right_top(), b);
                mesh.colored_vertex(r.right_bottom(), b);
                mesh.colored_vertex(r.left_bottom(), a);
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                p.add(mesh);
                let label = if left { format!("‹ {count}") } else { format!("+{count} ›") };
                let galley = p.layout_no_wrap(label, FontId::proportional(TEXT_STATE), Color32::WHITE);
                let size = galley.size() + vec2(14.0, 8.0);
                let chip = if left { Rect::from_min_size(r.left_center() + vec2(2.0, -size.y / 2.0), size) } else { Rect::from_min_size(r.right_center() - vec2(size.x + 2.0, size.y / 2.0), size) };
                p.rect_filled(chip, size.y / 2.0, if resp.hovered() { ACCENT } else { Color32::from_rgb(58, 64, 78) });
                p.galley(chip.min + vec2(7.0, 4.0), galley, Color32::WHITE);
                let more = if count == 1 { "1 more scene".to_string() } else { format!("{count} more scenes") };
                if resp.on_hover_text(more).clicked() {
                    self.bank_scroll_to = Some(if left { (x - view.width() + step).max(0.0) } else { x + view.width() - step });
                }
            };
            if x > 1.0 {
                edge(ui, true, hidden_left.max(1));
            }
            if x + view.width() < content - 1.0 {
                edge(ui, false, hidden_right.max(1));
            }
        });
        if let Some((i, cut)) = press {
            self.press_scene(i, cut);
        }
        if let Some((from, to)) = self.bank_move.take() {
            self.move_scene(from, to);
        }
    }

    /// The transitions: CUT and MIX with the length, the other four under
    /// ⋯ (the button reads WIPE ▾ and so on while one of them is picked),
    /// and Fade to black set apart at the end.
    fn transition_row(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 4.0;
        let mut mode = self.cfg.transition.mode;
        let mut open_switching = false;
        for m in [TransitionMode::Cut, TransitionMode::Mix] {
            let b = ui.add(egui::Button::selectable(mode == m, RichText::new(m.label().to_uppercase()).strong().size(TEXT_BODY)).min_size(vec2(46.0, 26.0)));
            let tip = if m == TransitionMode::Cut { format!("Scenes change at once{}", self.key_hint(Builtin::Transition)) } else { format!("Frames glide to their new places and cross-fade{}", self.key_hint(Builtin::Transition)) };
            if b.on_hover_text(tip).clicked() {
                mode = m;
            }
        }
        // The length does nothing for a cut, or for a stinger (its video sets it: numbers audit #14).
        let fixed = match mode {
            TransitionMode::Cut => Some("A cut is instant. The length is for Mix and the other transitions."),
            TransitionMode::Stinger => Some("A stinger lasts as long as its video (Settings → Switching)."),
            _ => None,
        };
        let mut ms = self.cfg.transition.duration_ms;
        let len = ui.add_enabled(fixed.is_none(), egui::DragValue::new(&mut ms).range(0..=3000).speed(10).suffix(" ms"));
        let len = if let Some(why) = fixed { len.on_disabled_hover_text(why) } else { len.on_hover_text("Transition length: drag, or click to type") };
        if len.changed() {
            self.cfg.transition.duration_ms = ms;
            self.cfg_dirty = true;
        }
        let other = !matches!(mode, TransitionMode::Cut | TransitionMode::Mix);
        let label = if other { format!("{} ▾", mode.label().to_uppercase()) } else { "⋯".to_string() };
        let more = ui.add(egui::Button::selectable(other, RichText::new(label).strong().size(TEXT_BODY)).min_size(vec2(30.0, 26.0)));
        egui::Popup::menu(&more).show(|ui| {
            ui.label(RichText::new("More transitions").color(TEXT_DIM).size(TEXT_SMALL));
            for (m, what) in [
                (TransitionMode::Wipe, "A soft edge sweeps across"),
                (TransitionMode::Push, "The new scene pushes the old one out"),
                (TransitionMode::Dip, "Through a colour (black unless set)"),
                (TransitionMode::Stinger, "A video plays over the change"),
            ] {
                if ui.selectable_label(mode == m, m.label()).on_hover_text(what).clicked() {
                    mode = m;
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Transition settings ›").clicked() {
                open_switching = true;
                ui.close();
            }
        });
        more.on_hover_text("More transitions: Wipe, Push, Dip, Stinger");
        if mode != self.cfg.transition.mode {
            self.cfg.transition.mode = mode;
            self.cfg_dirty = true;
        }
        // Fade to black: apart from the rest, drawn as the live control it is
        // (forensics bank #3); flashes while the program is black.
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        let black = self.engine.as_ref().is_some_and(|e| e.is_black());
        let lit = black && (ui.input(|i| i.time) * 2.0).fract() < 0.5;
        let ftb = egui::Button::new(RichText::new(if black { "Fade up" } else { "Fade to black" }).strong().size(TEXT_BODY).color(if lit { Color32::BLACK } else { Color32::WHITE }))
            .fill(if lit { AMBER } else if black { AMBER_DIM } else { RAISED })
            .stroke(Stroke::new(1.0, if black { AMBER } else { LINE }))
            .min_size(vec2(104.0, 26.0));
        if ui.add(ftb).on_hover_text(if black { "The program is black: click to fade back up" } else { "Fade the program, graphics and sound to black; click again to come back (the fade's length is in Settings → Switching)" }).clicked() {
            self.act(Action::Ftb(Toggle::Toggle));
        }
        if black {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if open_switching && self.settings.is_none() {
            self.settings = Some(self.new_settings().open_at(Section::Switching));
        }
    }

    /// Under the transitions: Preview, AUTO, CC, Edit layout and ⟲ Back.
    fn bank_toggles(&mut self, ui: &mut egui::Ui) {
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
            self.mv_selected = None;
        }
        // AUTO: greyed until it has rules (forensics bank #5); a click then
        // says why and links to them. The A key still turns it on and off.
        let auto = self.auto_on();
        let no_rules = self.cfg.auto_switch.rules.is_empty();
        let color = if auto { Color32::WHITE } else if no_rules { TEXT_DIM.gamma_multiply(0.55) } else { TEXT_DIM };
        let auto_btn = ui.add(egui::Button::selectable(auto, RichText::new("AUTO").strong().color(color)));
        if no_rules && !auto {
            let mut set_up = false;
            egui::Popup::menu(&auto_btn).show(|ui| {
                ui.set_max_width(260.0);
                ui.label("Auto switching cuts to whoever is talking. It needs rules first: which mic goes with which scene.");
                if ui.button("Set up rules ›").clicked() {
                    set_up = true;
                    ui.close();
                }
            });
            auto_btn.on_hover_text("Auto switching: no rules yet. Click to set them up.");
            if set_up && self.settings.is_none() {
                self.settings = Some(self.new_settings().open_at(Section::Switching));
            }
        } else if auto_btn.on_hover_text(format!("Switch automatically to whoever is talking{}. Rules are in Settings → Switching.", self.key_hint(Builtin::Auto))).clicked() {
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
        // Back to the scene on air before this one (was Backspace only).
        let last = self.engine.as_ref().and_then(|e| e.switcher.previous()).map(|s| s.name);
        let back = ui.add_enabled(last.is_some(), egui::Button::new("⟲ Back"));
        let back = match &last {
            Some(name) => back.on_hover_text(format!("Back to {name}, with the transition{}", self.key_hint(Builtin::Back))),
            None => back.on_disabled_hover_text("Back to the last scene, once there's been another on air"),
        };
        if back.clicked() {
            if let Err(e) = self.back_to_last_scene() {
                self.toast(format!("Can't go back: {e}"), false);
            }
        }
    }

    /// Move the scene at `from` to before the one at `to` (the end: the
    /// number of scenes). What picks scenes by number follows (see `scenes_moved`).
    fn move_scene(&mut self, from: usize, to: usize) {
        let Some(to) = moved_to(from, to, self.scenes.len()) else { return };
        let before = self.scene_names();
        self.remember(undo::Kind::Scenes, format!("Move scene {}", before[from]));
        let s = self.scenes.remove(from);
        self.scenes.insert(to, s);
        self.scenes_dirty = true;
        self.scenes_moved(&before);
    }

    /// One scene button: its key number (large), its name, an ON AIR or
    /// NEXT tag, ♪ for scene sound and ⚠ when a source in it has no signal;
    /// on big buttons also a live picture of the scene (its sources'
    /// multiview pictures where its frames are). Some((i, cut)) when pressed.
    fn scene_button(&mut self, ui: &mut egui::Ui, i: usize, s: &Scene, (is_live, is_preview): (bool, bool), missing: Option<&Vec<String>>, layout: BankLayout) -> Option<(usize, bool)> {
        let key = if i < 9 { format!("{}", i + 1) } else { String::new() };
        // In Edit layout the buttons can be dragged into another order (never
        // during the show otherwise, so a slip can't upset it).
        let (rect, resp) = ui.allocate_exact_size(layout.button, if self.edit_mode { Sense::click_and_drag() } else { Sense::click() });
        if self.edit_mode {
            resp.dnd_set_drag_payload(BankDrag(i));
            if resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(p) = ui.ctx().pointer_interact_pos() {
                    let layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("bank_drag")));
                    let galley = layer.layout_no_wrap(s.name.clone(), FontId::proportional(TEXT_BODY), Color32::WHITE);
                    let r = Rect::from_min_size(p + vec2(10.0, -30.0), galley.size() + vec2(16.0, 10.0));
                    layer.rect_filled(r, 5.0, ACCENT);
                    layer.galley(r.min + vec2(8.0, 5.0), galley, Color32::WHITE);
                }
            }
            let after = ui.ctx().pointer_interact_pos().is_some_and(|p| p.x > rect.center().x);
            if resp.dnd_hover_payload::<BankDrag>().is_some_and(|d| d.0 != i) {
                // Where it would go: a bar on this side of the button.
                let x = if after { rect.right() + 3.0 } else { rect.left() - 3.0 };
                ui.painter().line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(3.0, ACCENT));
            }
            if let Some(d) = resp.dnd_release_payload::<BankDrag>() {
                self.bank_move = Some((d.0, if after { i + 1 } else { i }));
            }
        }
        let painter = ui.painter_at(rect);
        let fill = if is_live {
            ON_AIR
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
        let key_color = if is_live { Color32::WHITE } else { Color32::from_white_alpha(170) };
        let key_w = if key.is_empty() { 0.0 } else { 22.0 };
        let tag = if is_live { Some(("ON AIR", Color32::WHITE, ON_AIR)) } else if is_preview { Some(("NEXT", GREEN, Color32::BLACK)) } else { None };
        let tag_chip = |painter: &egui::Painter, at: egui::Pos2, align: Align2| -> f32 {
            let Some((text, fill, color)) = tag else { return 0.0 };
            let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(TEXT_SMALL), color);
            let r = align.anchor_size(at, galley.size() + vec2(8.0, 2.0));
            painter.rect_filled(r, 3.0, fill);
            painter.galley(r.min + vec2(4.0, 1.0), galley, color);
            r.width()
        };
        let missing = missing.filter(|m| !m.is_empty());
        let warn_color = if is_live { Color32::WHITE } else { AMBER };
        let sound = !s.audio_on.is_empty() || !s.audio_off.is_empty();
        let name_left;
        let name_right;
        let name_y;
        if layout.thumbs {
            // Picture top left, key top right, the name along the bottom;
            // the tag and ♪ sit on the picture.
            let canvas = self.engine.as_ref().map(|e| e.info.canvas).unwrap_or((1920, 1080));
            let tw = (rect.width() - 16.0 - key_w - 4.0).clamp(40.0, 80.0);
            let th = (tw * canvas.1 as f32 / canvas.0.max(1) as f32).clamp(20.0, 44.0);
            let thumb = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(tw, th));
            self.scene_thumb(&painter, thumb, s, canvas);
            tag_chip(&painter, thumb.left_bottom() + vec2(2.0, -2.0), Align2::LEFT_BOTTOM);
            if sound {
                let r = Rect::from_min_size(thumb.right_bottom() - vec2(16.0, 16.0), vec2(16.0, 16.0));
                painter.rect_filled(r, 3.0, Color32::from_black_alpha(150));
                painter.text(r.center(), Align2::CENTER_CENTER, "♪", FontId::proportional(TEXT_SMALL), Color32::WHITE);
            }
            painter.text(rect.right_top() + vec2(-8.0, 4.0), Align2::RIGHT_TOP, &key, FontId::proportional(TEXT_TITLE), key_color);
            if missing.is_some() {
                painter.text(rect.right_top() + vec2(-9.0, 34.0), Align2::RIGHT_TOP, "⚠", FontId::proportional(TEXT_STATE), warn_color);
            }
            name_left = rect.left() + 9.0;
            name_right = rect.right() - 9.0;
            name_y = rect.bottom() - 9.0;
        } else {
            // One line: key, name, then ♪ ⚠ and the tag at the right.
            painter.text(rect.left_center() + vec2(9.0, 0.0), Align2::LEFT_CENTER, &key, FontId::proportional(TEXT_TITLE), key_color);
            let mut right = rect.right() - 6.0;
            name_left = rect.left() + 9.0 + if key_w > 0.0 { key_w + 4.0 } else { 0.0 };
            // The tag only where it leaves the name room (the fill says it too).
            let marks = if missing.is_some() { 18.0 } else { 0.0 } + if sound { 14.0 } else { 0.0 };
            if right - 56.0 - marks - name_left >= 64.0 {
                let w = tag_chip(&painter, pos2(right, rect.center().y), Align2::RIGHT_CENTER);
                if w > 0.0 {
                    right -= w + 4.0;
                }
            }
            if missing.is_some() {
                painter.text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, "⚠", FontId::proportional(TEXT_STATE), warn_color);
                right -= 18.0;
            }
            if sound {
                painter.text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, "♪", FontId::proportional(TEXT_BODY), Color32::from_white_alpha(180));
                right -= 14.0;
            }
            name_right = right - 2.0;
            name_y = f32::NAN;
        }
        // The name, cut short with "…" when it's long (the whole of it is in the hover text).
        let mut job = egui::text::LayoutJob::simple_singleline(s.name.clone(), FontId::proportional(TEXT_BODY), Color32::WHITE);
        job.wrap = egui::text::TextWrapping::truncate_at_width((name_right - name_left).max(10.0));
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let elided = galley.elided;
        let top = if name_y.is_nan() { rect.center().y - galley.size().y / 2.0 } else { name_y - galley.size().y };
        painter.galley(pos2(name_left, top), galley, Color32::WHITE);
        let press = resp.clicked().then(|| (i, ui.input(|inp| inp.modifiers.shift)));
        let tip = match (i < 9, self.cfg.transition.preview_mode) {
            (true, true) => format!("Press {} to preview (Shift+{} cuts to program)", i + 1, i + 1),
            (true, false) => format!("Press {} (Shift+{} to cut)", i + 1, i + 1),
            _ => "Click to take".into(),
        };
        let tip = if elided { format!("{}\n{tip}", s.name) } else { tip };
        let tip = if self.edit_mode { format!("{tip}\nDrag to change the order") } else { tip };
        let tip = match missing {
            Some(m) => format!("{tip}\n⚠ No signal from {}", m.join(", ")),
            None => tip,
        };
        resp.on_hover_text(tip);
        press
    }

    /// A small live picture of a scene: each frame's source as the
    /// multiview last showed it (no extra GPU work), placed, cropped and
    /// filled as on air; a grey box for a source with no picture yet.
    fn scene_thumb(&self, painter: &egui::Painter, thumb: Rect, scene: &Scene, canvas: (u32, u32)) {
        painter.rect_filled(thumb, 2.0, Color32::from_black_alpha(140));
        let p = painter.with_clip_rect(thumb.intersect(painter.clip_rect()));
        let k = thumb.width() / canvas.0.max(1) as f32;
        for l in &scene.layers {
            let Some((_, tex)) = self.textures.get(&format!("src{}", l.source)) else {
                let r = Rect::from_min_size(thumb.min + vec2(l.x * thumb.width(), l.y * thumb.height()), vec2(l.w * thumb.width(), l.h * thumb.height())).intersect(thumb);
                p.rect_filled(r, 1.0, Color32::from_white_alpha(70));
                p.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::from_white_alpha(160)), StrokeKind::Inside);
                continue;
            };
            let [w, h] = tex.size();
            let look = self.cfg.sources.get(l.source).map(|c| c.look.clone()).unwrap_or_default();
            let (uv, (x, y, fw, fh)) = thumb_view(look.crop_rect(), look.cropped_aspect(w as f32 / h.max(1) as f32), l, canvas);
            let r = Rect::from_min_size(thumb.min + vec2(x * k, y * k), vec2(fw * k, fh * k));
            p.image(tex.id(), r, uv, Color32::WHITE);
        }
    }

    // ---- right panel -------------------------------------------------------------

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(which) = self.confirm.clone() else { return };
        let (title, body, button) = match &which {
            Confirm::EndStream => ("End the live stream?".to_string(), "Viewers on every destination will see the broadcast end.".to_string(), "End stream"),
            Confirm::StopRecording => ("Stop recording?".to_string(), "The files for this take will be finished and saved.".to_string(), "Stop recording"),
            Confirm::SwitchShow(n) => {
                let keep = match self.unnamed_setup_name() {
                    Some(m) => format!("This setup isn't a show yet, so Cutaway first saves it as \"{m}\" (rename it in Settings → General)."),
                    None => "The current show keeps its changes.".to_string(),
                };
                (format!("Switch to \"{n}\"?"), format!("Sources, scenes, audio and everything else change to that show's setup. {keep}"), "Switch")
            }
            Confirm::Quit => {
                let (rec, live) = self.engine.as_ref().map_or((false, false), |e| (e.is_recording(), e.is_streaming()));
                quit_text(rec, live, self.busy())
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
                if ui.add(egui::Button::new(RichText::new(button).color(Color32::WHITE)).fill(ERROR)).clicked() {
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

    /// Only once there's somewhere to stream to (Record › Streaming sets
    /// that up; the column's ⋯ links there too).
    fn stream_panel(&mut self, ui: &mut egui::Ui) {
        if !self.panel_present("stream") {
            return;
        }
        ui.add_space(6.0);
        if !self.section_heading(ui, "stream", "STREAM", Section::Streaming) {
            return;
        }
        let Some(engine) = &self.engine else { return };
        let stream = engine.status.lock_ok().stream.clone();
        // A stream scheduled within the day (set in Record › Scheduled).
        if !engine.is_streaming() {
            if let Some((text, color)) = self.scheduled_line() {
                ui.add(egui::Label::new(RichText::new(text).size(TEXT_BODY).color(color)).wrap());
            }
        }
        let dot = |ui: &mut egui::Ui, color: Color32| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.5, color);
        };
        if stream.destinations.is_empty() {
            let dests: Vec<_> = self.cfg.streaming.destinations.iter().filter(|d| d.enabled).cloned().collect();
            if dests.is_empty() {
                ui.label(RichText::new("No destinations yet.").color(TEXT_DIM).size(TEXT_BODY));
            }
            for d in dests {
                ui.horizontal(|ui| {
                    let problem = d.problem();
                    dot(ui, if problem.is_some() { AMBER } else { LINE });
                    let r = ui.label(RichText::new(&d.name).size(TEXT_BODY));
                    ui.label(RichText::new(if problem.is_some() { "needs setup" } else { "ready" }).size(TEXT_BODY).color(TEXT_DIM));
                    if let Some(p) = problem {
                        r.on_hover_text(p);
                    }
                });
            }
            return;
        }
        if let Some(kbps) = stream.lowered_kbps {
            ui.label(RichText::new(format!("⚠ bitrate lowered to {:.1} Mbps while the connection catches up", kbps as f64 / 1000.0)).size(TEXT_BODY).color(AMBER))
                .on_hover_text(format!("Set: {:.1} Mbps. It comes back by itself once every destination keeps up.", self.cfg.streaming.video_bitrate_kbps as f64 / 1000.0));
        }
        for d in &stream.destinations {
            if d.state == DestState::Off {
                continue;
            }
            let color = match d.state {
                DestState::Live => GREEN,
                DestState::Connecting | DestState::Retrying { .. } => AMBER,
                DestState::Failed => ERROR,
                DestState::Off => LINE,
            };
            let resp = ui
                .horizontal(|ui| {
                    dot(ui, color);
                    ui.label(RichText::new(&d.name).size(TEXT_BODY).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if d.state == DestState::Live {
                            ui.label(RichText::new(format!("{:.1} Mbps", d.kbps / 1000.0)).size(TEXT_BODY).monospace());
                        } else {
                            let text = streamer::describe_dest(d);
                            ui.label(RichText::new(text).size(TEXT_BODY).color(color));
                        }
                    });
                })
                .response;
            if d.congested {
                ui.label(RichText::new("   ⚠ connection too slow — dropping frames").size(TEXT_BODY).color(AMBER));
            }
            if let Some(m) = &d.message {
                if d.state != DestState::Live {
                    ui.label(RichText::new(format!("   {m}")).size(TEXT_SMALL).color(TEXT_DIM));
                }
                resp.on_hover_text(format!("{} — {m}", d.service.label()));
            } else {
                resp.on_hover_text(d.service.label());
            }
        }
    }

    /// Always there (titles are why most people open Cutaway): with no
    /// graphics yet it offers to make a name title in one click.
    fn graphics_panel(&mut self, ui: &mut egui::Ui) {
        let list = self.engine.as_ref().map(|e| e.graphics.list()).unwrap_or_default();
        ui.add_space(4.0);
        if !self.section_heading(ui, "graphics", "GRAPHICS", Section::Graphics) {
            return;
        }
        if list.is_empty() {
            self.graphics_empty(ui);
            return;
        }
        let Some(engine) = &self.engine else { return };
        let mut toggle = None;
        let mut edit = None;
        let mut clock_menus = Vec::new();
        let mut timer = None;
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
                let text = RichText::new(shown).strong().size(TEXT_BODY).color(if *on { Color32::BLACK } else if over { ERROR } else { Color32::WHITE });
                let b = egui::Button::new(text).fill(if *on { AMBER } else { RAISED }).min_size(vec2(64.0, 28.0));
                let mut r = ui.add(b);
                let is_timer = self.cfg.graphics.get(i).is_some_and(|g| g.kind == crate::config::GraphicKind::Clock && g.clock.is_timer());
                let shown = on_air.iter().find(|o| o.name == *name).zip(since).map(|(o, (_, when))| {
                    let n = o.showings.len();
                    format!("\nOn screen {n} time{} ({}) {when}", if n == 1 { "" } else { "s" }, crate::engine::fmt_duration(Duration::from_secs_f64(o.seconds())))
                });
                r = r.on_hover_text(match problem {
                    Some(p) => format!("⚠ {p}"),
                    None if is_timer => "Show / hide. ▶ starts it and ⟲ puts it back to the start (right-click for more).".into(),
                    None if i < 9 => format!("Show / hide ({})", shortcuts::KeyCombo { alt: true, ..shortcuts::KeyCombo::plain(shortcuts::DIGITS[i]) }.label()),
                    None => "Show / hide".into(),
                } + &shown.unwrap_or_default());
                if r.clicked() {
                    toggle = Some(i);
                }
                if is_timer {
                    clock_menus.push((r.clone(), i));
                    // Start / pause and back to the start, on show (right-click still works).
                    let running = self.engine.as_ref().is_some_and(|e| e.graphics.timers.running(name));
                    let gap = ui.spacing().item_spacing.x;
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if ui.small_button(if running { "⏸" } else { "▶" }).on_hover_text(if running { "Pause the timer" } else { "Start the timer" }).clicked() {
                        timer = Some((i, if running { control::TimerCmd::Pause } else { control::TimerCmd::Start }));
                    }
                    if ui.small_button("⟲").on_hover_text("Back to the start (right-click the timer for more)").clicked() {
                        timer = Some((i, control::TimerCmd::Reset));
                    }
                    ui.spacing_mut().item_spacing.x = gap;
                }
                let is_title = self.cfg.graphics.get(i).is_some_and(|g| matches!(g.kind, crate::config::GraphicKind::Title | crate::config::GraphicKind::Ticker) && !g.follow);
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
                let text = RichText::new(format!("⟳ {}", r.name)).strong().size(TEXT_BODY).color(if r.running { Color32::BLACK } else { Color32::WHITE });
                let b = egui::Button::new(text).fill(if r.running { AMBER } else { RAISED }).min_size(vec2(64.0, 28.0));
                let state = match (&r.showing, r.next_in) {
                    _ if !r.running => format!("Stopped. {}", match cfg.start {
                        crate::config::RotationStart::Manual => "Click to start it.".to_string(),
                        s => format!("Starts by itself {}, or click to start it now.", s.label().replace("I'm", "you're")),
                    }),
                    (Some(g), _) => format!("{g} is on now."),
                    (None, Some(d)) => format!("Next one in {}.", crate::engine::timers::fmt_span(d, true)),
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
        if let Some((i, c)) = timer {
            self.act(Action::Timer(control::Target::Index(i), c));
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

    /// Shift+M: the marker's already in, at the moment the key was pressed;
    /// this names it. Cancel keeps the name it has ("Marker 3").
    fn marker_name_dialog(&mut self, ctx: &egui::Context) {
        let Some((at, own, mut text)) = self.marker_name.clone() else { return };
        let mut done = false;
        let mut apply = false;
        let r = egui::Modal::new(egui::Id::unique("marker_name")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Name this marker");
            ui.label(RichText::new(format!("{own} at {}: the time was kept when you pressed the key.", fmt_clock(Duration::from_nanos(at.max(0) as u64)))).color(TEXT_DIM));
            ui.add_space(4.0);
            let field = ui.add(egui::TextEdit::singleline(&mut text).hint_text(own.as_str()).desired_width(340.0));
            if ui.memory(|m| m.focused().is_none()) {
                field.request_focus();
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if ui.add(egui::Button::new(RichText::new("OK").color(Color32::WHITE)).fill(ACCENT)).clicked() || enter {
                    apply = true;
                    done = true;
                }
                if ui.button("Cancel").on_hover_text(format!("Keep it as \"{own}\"")).clicked() {
                    done = true;
                }
            });
        });
        if apply && !text.trim().is_empty() {
            if let Some(e) = &self.engine {
                match e.rename_marker(at, &text) {
                    Ok(()) => self.toast(format!("Marker \"{}\" added", text.trim()), false),
                    Err(err) => self.toast(format!("Couldn't name the marker: {err:#}"), true),
                }
            }
        }
        self.marker_name = if done || r.should_close() { None } else { Some((at, own, text)) };
    }

    /// The list under ◆ Marker ▾: this take's markers, to rename or take out
    /// until it stops.
    fn marker_list(&mut self, ui: &mut egui::Ui, markers: &[crate::engine::recorder::Marker]) {
        ui.set_min_width(380.0);
        if markers.is_empty() {
            ui.label(RichText::new("No markers in this take yet.").color(TEXT_DIM));
            return;
        }
        ui.label(RichText::new("Names can be changed until the take stops.").size(TEXT_SMALL).color(TEXT_DIM));
        let mut rename: Option<(crate::media::Ns, String)> = None;
        let mut delete: Option<crate::media::Ns> = None;
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("marker_list").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                for m in markers {
                    ui.label(RichText::new(fmt_clock(Duration::from_nanos(m.at.max(0) as u64))).monospace());
                    if m.clip {
                        ui.label(format!("✂ {}", m.name)).on_hover_text("Left by a saved clip (in the take's Clips folder): on the editing timelines, not a chapter");
                    } else {
                        let mut text = match &self.marker_edit {
                            Some((at, t)) if *at == m.at => t.clone(),
                            _ => m.name.clone(),
                        };
                        let r = ui.add_sized([180.0, 20.0], egui::TextEdit::singleline(&mut text));
                        if r.changed() {
                            self.marker_edit = Some((m.at, text.clone()));
                        }
                        if r.lost_focus() {
                            if self.marker_edit.as_ref().is_some_and(|(at, _)| *at == m.at) {
                                if !text.trim().is_empty() && text.trim() != m.name {
                                    rename = Some((m.at, text.clone()));
                                }
                                self.marker_edit = None;
                            }
                        }
                    }
                    ui.label(RichText::new(&m.scene).color(TEXT_DIM)).on_hover_text("On air at the time");
                    if !m.clip && ui.small_button("✕").on_hover_text("Take this marker out").clicked() {
                        delete = Some(m.at);
                    }
                    ui.end_row();
                }
            });
        });
        let Some(e) = &self.engine else { return };
        let result = match (rename, delete) {
            (Some((at, name)), _) => e.rename_marker(at, &name),
            (None, Some(at)) => e.delete_marker(at),
            _ => Ok(()),
        };
        if let Err(err) = result {
            self.toast(format!("{err:#}"), true);
        }
    }

    /// The audio strip under the panels: each input's meter with its fader
    /// beside it, its level, ON and tags, then the two mixes. Returns the
    /// meters' height (so the column knows what the rest takes).
    fn audio_panel(&mut self, ui: &mut egui::Ui) -> Option<f32> {
        self.audio_heading(ui);
        let engine = self.engine.as_ref()?;
        let (meters, offline, loud) = {
            let st = engine.status.lock_ok();
            (st.meters.clone(), st.offline.clone(), st.loudness.clone())
        };
        let strips: Vec<(String, String, bool, f32)> = engine.audio.iter().map(|a| (a.name.clone(), a.description(), a.shared.is_on(), a.shared.reduction_db())).collect();
        let automix: Vec<Option<f32>> = engine.audio.iter().map(|a| a.shared.automix_db()).collect();
        let monitoring = !self.cfg.mix.monitor_device.is_empty();
        let ducking = engine.mix.ducking_db();
        let recording = engine.is_recording();
        let live = engine.is_streaming();
        let take_since = match &engine.status.lock_ok().rec {
            RecState::Recording { since, .. } => Some(since.elapsed()),
            _ => None,
        };
        if strips.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("No microphones or other inputs yet.").color(TEXT_DIM));
            if ui.add(egui::Button::new(RichText::new("Add one in Sound › Inputs ›").size(TEXT_BODY).color(ACCENT)).frame(false)).clicked() {
                self.open_page(Section::Audio, None);
            }
            return None;
        }
        let mut gain_changes = Vec::new();
        let mut on_changes = Vec::new();
        let mut solo: Option<String> = None;
        ui.add_space(2.0);
        // The meters (and the faders beside them) take the height the column
        // has left under the names, levels, ON buttons, tags and the
        // headphone line, which always fit.
        let rest = self.column.audio_rest.unwrap_or(130.0);
        let meter_h = (ui.available_height() - rest).clamp(column::METER_MIN, 560.0);
        egui::ScrollArea::horizontal().id_salt("strips").auto_shrink([true, true]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (i, (name, desc, on, reduction)) in strips.iter().enumerate() {
                    let key = format!("a{i}");
                    ui.vertical(|ui| {
                        ui.set_width(68.0);
                        ui.add(egui::Label::new(RichText::new(name).strong().size(TEXT_BODY)).truncate()).on_hover_text(desc);
                        let m = meters.get(&key).cloned().unwrap_or_default();
                        let hold = self.peak_hold_for(&key, &m.peak_db);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            draw_meter(ui, vec2(32.0, meter_h), &m.peak_db, &m.rms_db, &hold, offline.contains(name), !*on);
                            // Compressor gain reduction.
                            if let Some(ac) = self.cfg.audio.get(i) {
                                if ac.processing.enabled && ac.processing.compressor {
                                    draw_reduction(ui, vec2(5.0, meter_h), *reduction);
                                }
                            }
                            // The fader beside it, as tall as the meter.
                            if let Some(ac) = self.cfg.audio.get_mut(i) {
                                ui.spacing_mut().slider_width = meter_h;
                                let mut g = ac.gain_db as f32;
                                if ui.add(egui::Slider::new(&mut g, -60.0..=12.0).vertical().show_value(false)).on_hover_text(format!("Level {g:+.1} dB (double-click resets)")).double_clicked() {
                                    g = 0.0;
                                }
                                if (g as f64 - ac.gain_db).abs() > 1e-3 {
                                    ac.gain_db = g as f64;
                                    gain_changes.push((i, ac.gain_db));
                                }
                            }
                        });
                        if let Some(ac) = self.cfg.audio.get_mut(i) {
                            ui.label(RichText::new(format!("{:+.1} dB", ac.gain_db)).size(TEXT_BODY).color(TEXT_DIM));
                            let btn = egui::Button::new(RichText::new(if *on { "ON" } else { "OFF" }).strong().size(TEXT_BODY).color(if *on { Color32::BLACK } else { Color32::WHITE }))
                                .fill(if *on { GREEN } else { OFF })
                                .min_size(vec2(56.0, 24.0));
                            if ui.add(btn).on_hover_text(if *on { "Heard in the mixes: click to turn it off" } else { "Off: not heard in the mixes. Click to turn it on" }).clicked() {
                                on_changes.push((i, !*on));
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let soloed = monitoring && self.cfg.mix.monitor_source.eq_ignore_ascii_case(name);
                                let h = ui.add(egui::Button::selectable(soloed, RichText::new("🎧").size(TEXT_SMALL)).min_size(vec2(24.0, 18.0)));
                                if h.on_hover_text(if monitoring { "Solo in the headphones" } else { "Choose a headphone output in Settings → Mixing & headphones" }).clicked() && monitoring {
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
                                    ui.label(RichText::new(tags.trim()).size(TEXT_BODY).color(ACCENT)).on_hover_text("FX: voice processing on · R: recording only · S: stream only · D: ducks under voices · A: in the automatic mic mix");
                                }
                                if let Some(db) = automix.get(i).copied().flatten().filter(|d| *d < -0.5 && self.cfg.mix.automix && *on) {
                                    ui.label(RichText::new(format!("↓{:.0}", -db)).size(TEXT_BODY).color(ACCENT)).on_hover_text("The automatic mic mixer has this mic down right now (dB), because someone else is talking");
                                }
                                if ac.duck && ducking < -0.5 {
                                    ui.label(RichText::new(format!("↓{:.0}", -ducking)).size(TEXT_BODY).color(AMBER)).on_hover_text("Turned down under the voices right now (dB)");
                                }
                            });
                        }
                    });
                }
                // A line between the inputs and the mixes, as tall as the meters
                // (a plain separator would take the whole column's height).
                let (line, _) = ui.allocate_exact_size(vec2(9.0, meter_h + 20.0), Sense::hover());
                ui.painter().vline(line.center().x, line.y_range(), Stroke::new(1.0, LINE));
                for (key, label_text, l) in [("mix", "REC MIX", &loud.recording), ("stream", "STREAM", &loud.stream)] {
                    ui.vertical(|ui| {
                        ui.set_width(70.0);
                        ui.label(RichText::new(label_text).strong().size(TEXT_BODY));
                        let m = meters.get(key).cloned().unwrap_or_default();
                        let hold = self.peak_hold_for(key, &m.peak_db);
                        draw_meter(ui, vec2(36.0, meter_h), &m.peak_db, &m.rms_db, &hold, false, false);
                        let fmt = |v: f32| if v < -99.0 { "—".to_string() } else { format!("{v:.0}") };
                        ui.label(RichText::new(format!("{} LUFS", fmt(l.short_term))).monospace().size(TEXT_BODY))
                            .on_hover_text(format!("Short-term loudness (3 s). Momentary (0.4 s): {} LUFS", fmt(l.momentary)));
                        // Integrated loudness counts from the start of each take (or
                        // going live): shown only while there's one to count.
                        let counting = if key == "mix" { recording } else { live };
                        let since = if key == "mix" {
                            take_since.map(|d| format!("this take, since REC started {} ago", crate::engine::timers::fmt_span(d, false))).unwrap_or_else(|| "this take".into())
                        } else {
                            "since you went live".into()
                        };
                        let integrated = if counting { fmt(l.integrated) } else { "—".into() };
                        ui.label(RichText::new(format!("I {integrated}")).monospace().size(TEXT_BODY).color(TEXT_DIM)).on_hover_text(if counting {
                            format!("Integrated loudness of {since}")
                        } else if key == "mix" {
                            "Integrated loudness: starts counting when you record".to_string()
                        } else {
                            "Integrated loudness: starts counting when you go live".to_string()
                        });
                        if l.limiting > 0.5 {
                            ui.label(RichText::new(format!("lim −{:.0}", l.limiting)).size(TEXT_BODY).color(AMBER));
                        }
                        if key == "stream" {
                            if let Some(g) = loud.stream_gain_db {
                                ui.label(RichText::new(format!("lev {g:+.1}")).size(TEXT_BODY).color(ACCENT)).on_hover_text("What stream levelling is adding");
                            }
                        }
                    });
                }
            });
        });
        if monitoring {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🎧").size(TEXT_BODY));
                let src = if self.cfg.mix.monitor_source == "stream" {
                    "stream mix".to_string()
                } else if self.cfg.mix.monitor_source == "program" || self.cfg.mix.monitor_source.is_empty() {
                    "recording mix".to_string()
                } else {
                    format!("{} solo", self.cfg.mix.monitor_source)
                };
                ui.label(RichText::new(src).size(TEXT_SMALL).color(TEXT_DIM));
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
        Some(meter_h)
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
        let (log, logged): (Vec<_>, u64) = {
            let st = engine.status.lock_ok();
            (st.log.iter().cloned().collect(), st.logged)
        };
        self.log_seen = logged;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.log_warnings_only, "Warnings only");
            if ui.small_button("Copy").on_hover_text("Copy what's shown, to paste into a message").clicked() {
                let text: Vec<String> = log.iter().filter(|l| !self.log_warnings_only || l.level != LogLevel::Info).map(|l| format!("{}  {}", l.time, l.text)).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
        });
        egui::ScrollArea::vertical().id_salt("status_log").stick_to_bottom(true).max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
            for l in log.iter().filter(|l| !self.log_warnings_only || l.level != LogLevel::Info) {
                let color = match l.level {
                    LogLevel::Info => TEXT_DIM,
                    LogLevel::Warning => AMBER,
                    LogLevel::Error => ERROR,
                };
                ui.label(RichText::new(format!("{}  {}", l.time, l.text)).color(color).monospace().size(TEXT_SMALL));
            }
        });
    }

    // ---- health ----------------------------------------------------------------------

    /// Everything at a glance, under the status light's Now: drives,
    /// sources, the files being recorded and the sound.
    fn health_body(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else {
            ui.label(RichText::new("The engine isn't running.").color(TEXT_DIM));
            return;
        };
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
        let ok = |ui: &mut egui::Ui, good: bool, text: String| {
            ui.label(RichText::new(text).color(if good { Color32::WHITE } else { AMBER }));
        };
        ui.label(RichText::new("DRIVES").color(TEXT_DIM).size(TEXT_SMALL));
        match &disk {
            // The same words and limit as the top bar (numbers audit #15, #16).
            Some(d) => ok(ui, !d.low(), format!("Recording: {} (of {}, {})", d.describe(), crate::util::human_size(d.total), if d.measured { "rate measured" } else { "rate estimated" })),
            None => ok(ui, false, "Recording drive: unknown".into()),
        }
        if let Some(b) = &backup {
            ok(ui, b.free > crate::engine::health::MIN_START_BYTES, format!("Backup: {}", b.describe()));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("SOURCES").color(TEXT_DIM).size(TEXT_SMALL));
        egui::Grid::new("h_src").num_columns(4).striped(true).show(ui, |ui| {
            for (i, s) in h.sources.iter().enumerate() {
                ui.label(&s.name);
                // Nothing to measure from yet just after a start: no false 0 fps (numbers audit #3).
                let text = match (s.alive, s.measured) {
                    (false, _) => "no signal".to_string(),
                    (true, false) => "measuring…".to_string(),
                    (true, true) => format!("{:.1} fps", s.fps),
                };
                ok(ui, s.alive && !s.slow(), text);
                ui.label(RichText::new(format!("set to {}", s.expected_fps)).color(TEXT_DIM));
                ui.label(RichText::new(extras.get(i).cloned().unwrap_or_default()).color(TEXT_DIM));
                ui.end_row();
            }
        });
        if !h.files.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("FILES BEING RECORDED").color(TEXT_DIM).size(TEXT_SMALL));
            egui::Grid::new("h_files").num_columns(5).striped(true).show(ui, |ui| {
                for t in ["File", "Frames", "Repeated", "Dropped", "Waiting"] {
                    ui.label(RichText::new(t).color(TEXT_DIM).size(TEXT_SMALL));
                }
                ui.end_row();
                for f in &h.files {
                    ui.label(&f.label);
                    let r = ui.label(RichText::new(if f.failed { "failed".into() } else { f.encoded.to_string() }).color(if f.failed { AMBER } else { Color32::WHITE }));
                    if f.resized > 0 {
                        r.on_hover_text(format!("{} frames were rescaled because the source changed size", f.resized));
                    }
                    // Totals for the take; amber only for trouble in the last minute (numbers audit #17).
                    let recent = |ui: &mut egui::Ui, bad: bool, total: u64, recent: u64| {
                        ui.label(RichText::new(total.to_string()).color(if bad { AMBER } else { Color32::WHITE })).on_hover_text(format!("{recent} in the last minute"));
                    };
                    recent(ui, f.repeating(), f.repeated, f.recent_repeated);
                    recent(ui, f.dropping(), f.dropped, f.recent_dropped);
                    ok(ui, f.backlog <= 30, f.backlog.to_string());
                    ui.end_row();
                }
            });
            ui.label(RichText::new(format!("Repeated: the source skipped a frame (filled in). Dropped: extra frames from a fast source. Amber when there were {} or more in the last minute. Waiting: frames queued for the encoder (should stay small).", crate::engine::health::RECENT_FRAMES_LIMIT)).size(TEXT_SMALL).color(TEXT_DIM));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("AUDIO").color(TEXT_DIM).size(TEXT_SMALL));
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
        self.precheck = Some(Precheck { items, rx, waiting, then, started: Instant::now() });
    }

    fn precheck_dialog(&mut self, ctx: &egui::Context) {
        let Some(pc) = self.precheck.as_mut() else { return };
        if let Some(rx) = &pc.rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => {
                        pc.items.push(item);
                        pc.waiting = pc.waiting.saturating_sub(1);
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    // The checks are over, whether or not each one answered.
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        pc.waiting = 0;
                        break;
                    }
                }
            }
            if pc.waiting > 0 && pc.started.elapsed() > Duration::from_secs(20) {
                // Don't wait for ever on a server that never answers.
                pc.items.push(CheckItem { level: Level::Warn, what: "Streaming".into(), detail: format!("{} destination(s) didn't answer in time", pc.waiting) });
                pc.waiting = 0;
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
            let (verdict, vcolor) = precheck_verdict(&items, done);
            ui.label(RichText::new(verdict).strong().color(vcolor));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("checks").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                    for it in &items {
                        let (sym, color) = match it.level {
                            Level::Ok => ("✔", GREEN),
                            Level::Warn => ("⚠", AMBER),
                            Level::Fail => ("✖", ERROR),
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
                    if ui.add_enabled(done, egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(if worst == Level::Fail { AMBER_DIM } else { ACCENT })).clicked() {
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
                    ui.label(RichText::new(if p < 0.12 { "Get ready…" } else { "Clap now!" }).size(TEXT_TITLE).strong().color(if p < 0.12 { TEXT_DIM } else { AMBER }));
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
                            ui.label(RichText::new(t).color(TEXT_DIM).size(TEXT_SMALL));
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
                    ui.label(RichText::new("The latest camera or input sets the time; everything else is held back to match it. Sources and inputs restart for a moment when delays change.").size(TEXT_SMALL).color(TEXT_DIM));
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
            self.display_toggle_full = false;
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
        let mut toggle_full = std::mem::take(&mut self.display_toggle_full);
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
                        [(halves[0], preview, preview_name.clone().unwrap_or_else(|| "PREVIEW".into()), GREEN), (halves[1], program, live_name.clone(), ON_AIR)].into_iter().enumerate()
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
                        painter.text(rr.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, FontId::proportional(TEXT_HEADING), Color32::WHITE);
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
                        let color = if *on_air { ON_AIR } else if *pv { GREEN } else { Color32::from_gray(70) };
                        painter.rect_stroke(rr, 0.0, Stroke::new(if *on_air || *pv { 3.0 } else { 1.0 }, color), StrokeKind::Outside);
                        painter.text(rr.left_bottom() + vec2(6.0, -6.0), Align2::LEFT_BOTTOM, format!("{} {}", i + 1, name), FontId::proportional(TEXT_STATE), Color32::WHITE);
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

    /// The settings as they are now, scenes included (they can have
    /// changes not saved yet).
    fn live_settings(&self) -> Config {
        let mut cfg = self.cfg.clone();
        cfg.scenes = self.scenes.clone();
        cfg
    }

    /// A settings window on the settings as they are now.
    fn new_settings(&self) -> SettingsUi {
        let win = SettingsUi::new(self.live_settings());
        match self.settings_page {
            Some(section) => win.open_at(section),
            None => win,
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
        let disk_rate = self.engine.as_ref().and_then(|e| e.status.lock_ok().disk.clone()).filter(|d| d.measured).zip(self.take_rate_estimate).map(|(d, e)| (d.rate, e));
        SettingsContext {
            recording,
            streaming,
            disk_rate,
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
            undo: self.history.can_undo().map(String::from),
            redo: self.history.can_redo().map(String::from),
            pictures: Vec::new(),
            virtual_ptz: Vec::new(),
        }
    }

    /// What a Settings page shows from the running show, the sources'
    /// pictures (as the multiview shows them) included.
    fn settings_info(&mut self, ctx: &egui::Context) -> SettingsContext {
        let mut info = self.settings_context();
        if let Some(e) = &self.engine {
            let slots: Vec<(usize, u64, Arc<FrameSlot>)> = e.sources.iter().enumerate().map(|(i, s)| (i, s.uid, s.preview.clone())).collect();
            info.virtual_ptz = e.sources.iter().enumerate().filter_map(|(i, s)| Some((s.uid, e.ptz(i)?.virt.clone()?))).collect();
            for (i, uid, slot) in slots {
                let size = slot.latest().map(|(_, f)| [f.width, f.height]);
                if let (Some(tex), Some(size)) = (self.texture(ctx, &format!("src{i}"), &slot), size) {
                    info.pictures.push(crate::settings::Picture { uid, tex, size });
                }
            }
        }
        info
    }

    /// Do what a Settings page asked for.
    fn settings_action(&mut self, action: SettingsAction, ctx: &egui::Context) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => {
                // (A page that saves as you go: what's still in a box is kept.)
                if let Some(a) = self.settings.as_mut().and_then(|w| w.take_live()) {
                    self.settings_action(a, ctx);
                }
                self.settings = None;
            }
            SettingsAction::Live { cfg, base, label } => {
                let before = self.snapshot(undo::Kind::Settings, label);
                // Only what was changed on the page: the desk's changes stay.
                let merged = self.live_settings().with_edits(&base, &cfg);
                self.quiet_save = true;
                let result = self.apply_settings(merged, ctx);
                self.quiet_save = false;
                if result.is_ok() && before.settings_differ(&self.cfg) {
                    self.history.record(before);
                }
                let now = self.live_settings();
                match self.settings.as_mut() {
                    Some(win) => win.applied_live(result, &cfg, &now),
                    None => {
                        if let Err(e) = result {
                            self.toast(format!("Couldn't save a change: {e}"), true);
                        }
                    }
                }
            }
            SettingsAction::Undo => self.undo(ctx),
            SettingsAction::Redo => self.redo(ctx),
            SettingsAction::Go(place) => self.go(place),
            SettingsAction::Run(a) => {
                if let Err(e) = self.run_action(a) {
                    self.toast(e, true);
                }
            }
            SettingsAction::VirtualCamera(on) => self.set_virtual_camera(on),
            SettingsAction::MeasureSync => {
                self.settings = None;
                self.sync = Some(SyncWizard::default());
            }
            SettingsAction::Apply { cfg, base, close } => {
                let before = self.snapshot(undo::Kind::Settings, "Settings change");
                // Only what was changed in Settings: a fader moved or a
                // transition picked on the desk meanwhile stays as it is.
                let cfg = self.live_settings().with_edits(&base, &cfg);
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
    }

    /// Apply new settings: live where possible, otherwise rebuild the engine.
    fn apply_settings(&mut self, mut new: Config, ctx: &egui::Context) -> Result<(), String> {
        // Panels folded or moved while Settings was open stay that way.
        new.ui.panel_order = self.cfg.ui.panel_order.clone();
        new.ui.folded = self.cfg.ui.folded.clone();
        self.apply_settings_as(new, ctx, false)
    }

    /// `own_scenes`: the scenes in `new` already go with its sources (a
    /// show's saved settings, an undo), rather than with the ones running
    /// now (an edit in Settings), so they're not moved with the sources.
    fn apply_settings_as(&mut self, mut new: Config, ctx: &egui::Context, own_scenes: bool) -> Result<(), String> {
        new.validate().map_err(|e| e.to_string())?;
        if new.remote.ensure_pin() {
            self.toast(format!("The remote's PIN is {}", new.remote.pin), false);
        }
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
        if old_uids != new_uids && !own_scenes {
            let new_of = |old: usize| old_uids.get(old).and_then(|u| new_uids.iter().position(|n| n == u));
            for sc in &mut new.scenes {
                sc.remap_sources(&new_of);
            }
        }
        // Close-ups just cut from a camera also get a scene with them side by side.
        if !own_scenes {
            let both = crate::scene::split_scenes(&new.sources, &|i| !old_uids.contains(&new.sources[i].uid), &new.effective_scenes());
            if !both.is_empty() {
                new.scenes = new.effective_scenes();
                new.scenes.extend(both);
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
        let old_names = self.scene_names();
        self.cfg = new;
        self.scenes = if self.cfg.scenes.is_empty() { self.cfg.effective_scenes() } else { self.cfg.scenes.clone() };
        // Scenes renamed in Settings → Scenes: the one on air keeps its picture under its new name.
        let renamed = control::scene_renames(&old_names, &self.scene_names());
        if let (Some(e), false) = (&self.engine, renamed.is_empty()) {
            e.switcher.rename_scenes(&|n| renamed.iter().find(|(a, _)| a == n).map(|(_, b)| b.clone()));
        }
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
        } else {
            self.apply_services();
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

impl App {
    /// The first time, make the window fit the screen it opened on.
    fn fit_first_window(&mut self, ctx: &egui::Context) {
        if !self.fit_window {
            return;
        }
        let (screen, inner) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().inner_rect));
        let (Some(screen), Some(inner)) = (screen, inner) else { return };
        self.fit_window = false;
        if screen.x <= 0.0 || screen.y <= 0.0 {
            return;
        }
        let size = first_window_size(inner.size(), screen);
        if size != inner.size() {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(((screen.x - size.x) / 2.0).max(0.0), 30.0)));
        }
    }
}

impl App {
    /// The windows, dialogs and messages over everything (whichever area is open).
    fn over_the_window(&mut self, ctx: &egui::Context) {
        self.display_window(ctx);
        self.guide_window(ctx);
        self.search_window(ctx);
        self.confirm_dialog(ctx);
        self.title_dialog(ctx);
        self.marker_name_dialog(ctx);
        self.precheck_dialog(ctx);
        self.sync_dialog(ctx);
        self.rescue_card(ctx);
        self.toasts(ctx);
        // Warnings the engine wants seen (disk filling up, backup drive full).
        let alerts = self.engine.as_ref().map(|e| std::mem::take(&mut e.status.lock_ok().alerts)).unwrap_or_default();
        for a in alerts {
            self.toast(a, true);
        }
    }
}

impl eframe::App for App {
    /// Only the window's size and place are remembered (eframe's
    /// persistence), not egui's own state.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// Runs before every frame, and also while the window is minimised or
    /// hidden (eframe then skips drawing but still calls this when a repaint
    /// is asked for). So commands from the phone, Stream Deck, OSC and MIDI
    /// still run, a failed recording is still stopped, and the state they
    /// read keeps moving when nobody is looking at the desk.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_requests();
        self.poll_status();
        self.publish_state();
        if self.remote.is_some() || self.midi.is_some() || self.osc.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Each area's last tab, beside the window's size (this computer, not the show).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.shell.save(storage);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Closing the window (or Cmd+Q / Alt+F4) mid-show asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_ok && (self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) || self.busy().is_some()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Quit);
        }
        self.fit_first_window(&ctx);
        self.tick_starting(&ctx);
        self.autosave();
        // Controllers' commands, the engine's status and the published state
        // are handled in `logic`, which runs just before this.
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
        self.run_self_test(&ctx);
        self.scheduled_go_live(&ctx);
        let busy = self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming());
        if !busy && crate::platform::permissions_changed() {
            self.start_engine();
        }

        // What a page that saves as you go left waiting when you went elsewhere.
        for a in std::mem::take(&mut self.deferred_settings) {
            self.settings_action(a, &ctx);
        }
        // The rail of areas down the left, the top bar with the area's tabs
        // and the status cluster, then Show's desk or the area's page.
        self.sync_shell();
        let rail = Self::rail_width(&ctx);
        egui::Panel::left("rail").exact_size(rail).resizable(false).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(0, 6))).show(ui, |ui| self.rail(ui));
        egui::Panel::top("header").exact_size(shell::TOP_BAR).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6))).show(ui, |ui| self.top_bar(ui));
        if !self.on_show() {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(20, 12))).show(ui, |ui| self.area_page(ui));
            self.over_the_window(&ctx);
            return;
        }
        // A second row of scenes when they don't fit in one and the window is tall.
        self.bank_rows = if self.bank_overflows && ctx.content_rect().height() >= 1000.0 { 2 } else { 1 };
        let bank_height = 104.0 + (self.bank_rows - 1) as f32 * (BANK_BUTTON.y + 6.0);
        egui::Panel::bottom("bank").exact_size(bank_height).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6))).show(ui, |ui| self.scene_bank(ui));
        // The column on the right: drag its edge to make it wider (kept with the show).
        let widest = (ui.available_width() * 0.6).max(COLUMN_WIDTH);
        // Its usual width leaves the program room on a small screen.
        let usual = COLUMN_WIDTH.min((ui.available_width() * 0.38).max(300.0));
        let width = if self.cfg.ui.column_width > 0.0 { self.cfg.ui.column_width } else { usual };
        let column = egui::Panel::right(("column", self.column.resets))
            .resizable(true)
            .default_size(width.clamp(280.0, widest))
            .size_range(280.0..=widest)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| self.right_column(ui));
        self.column_edge(&ctx, column.response.rect);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(14))).show(ui, |ui| self.monitors(ui));

        self.over_the_window(&ctx);
    }

    fn on_exit(&mut self) {
        // A change still in a box on a page that saves as you go is kept.
        if let Some(SettingsAction::Live { cfg, base, .. }) = self.settings.as_mut().and_then(|w| w.take_live()) {
            self.cfg = self.cfg.with_edits(&base, &cfg);
            self.cfg_dirty = true;
        }
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

struct Toast {
    text: String,
    /// When it goes; None stays until clicked.
    until: Option<Instant>,
    error: bool,
}

/// What the quit confirmation says, for what's running.
fn quit_text(recording: bool, live: bool, busy: Option<String>) -> (String, String, &'static str) {
    let (title, body) = match (recording, live, busy) {
        (true, true, _) => ("Quit while recording and live?".into(), "Cutaway will finish and save this take, and the broadcast ends on every destination.".into()),
        (false, true, _) => ("Quit while live?".into(), "The broadcast ends on every destination.".into()),
        (true, false, _) => ("Quit while recording?".into(), "Cutaway will finish and save this take.".into()),
        (false, false, Some(b)) => (
            "Quit before Cutaway has finished?".to_string(),
            format!("Cutaway is still {b}. Saving finishes before Cutaway closes; uploads stop part way and carry on when you next open Cutaway, but the podcast copy has to be made again."),
        ),
        (false, false, None) => ("Quit?".into(), String::new()),
    };
    (title, body, "Quit")
}

// ---- drawing helpers -------------------------------------------------------------

/// Text drawn a second time half a pixel over, for a bold look (the
/// interface font has no bold of its own).
fn bold_text(painter: &egui::Painter, at: egui::Pos2, align: Align2, text: &str, size: f32, color: Color32) {
    painter.text(at, align, text, FontId::proportional(size), color);
    painter.text(at + vec2(0.6, 0.0), align, text, FontId::proportional(size), color);
}

fn label_chip(painter: &egui::Painter, left_center: egui::Pos2, text: &str) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(TEXT_STATE), Color32::WHITE);
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
        let zones = [(-60.0, -18.0, GREEN), (-18.0, -6.0, AMBER), (-6.0, 0.0, ERROR)];
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
                let color = if h > -1.0 { ERROR } else { Color32::from_white_alpha(120) };
                painter.line_segment([pos2(x, y), pos2(x + bar_w, y)], Stroke::new(1.0, color));
            }
        }
    }
    for db in [-6.0, -18.0, -40.0] {
        let y = to_y(db);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.left() + 3.0, y)], Stroke::new(1.0, LINE));
    }
    if offline {
        painter.text(rect.center(), Align2::CENTER_CENTER, "OFF", FontId::proportional(TEXT_BODY), AMBER);
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

/// A scene button being dragged in the bank (its place).
struct BankDrag(usize);

/// Where a scene dragged from `from` and dropped before place `to` (0..=n)
/// ends up in a list of `n`: None if it doesn't move.
fn moved_to(from: usize, to: usize, n: usize) -> Option<usize> {
    if from >= n {
        return None;
    }
    let to = if to > from { to - 1 } else { to }.min(n - 1);
    (to != from).then_some(to)
}

/// A labelled group of tools on the edit bar ("FRAME", "SCENE").
/// Wraps inside when the window is too narrow for it; its width.
fn tool_group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) -> f32 {
    let r = egui::Frame::new().stroke(Stroke::new(1.0, Color32::from_rgb(52, 58, 70))).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.set_max_width(ui.available_width() - 18.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(title).color(TEXT_DIM).size(TEXT_SMALL).strong());
            add(ui);
        });
    });
    r.response.rect.width()
}

fn unique_name(scenes: &[Scene], base: &str) -> String {
    if !scenes.iter().any(|s| s.name == base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !scenes.iter().any(|s| &s.name == n)).unwrap()
}

/// Where a layer's picture goes on a scene thumbnail (x, y, w, h in canvas
/// pixels) and the part of the source texture shown there: the frame's
/// placement (fit, fill, zoom) of the picture after the source's own crop.
fn thumb_view(crop: [f32; 4], cropped_aspect: f32, layer: &Layer, canvas: (u32, u32)) -> (Rect, (f32, f32, f32, f32)) {
    let (rect, part) = crate::scene::place(layer, canvas, cropped_aspect);
    let u = |f: f32| crop[0] + f * (crop[2] - crop[0]);
    let v = |f: f32| crop[1] + f * (crop[3] - crop[1]);
    (Rect::from_min_max(pos2(u(part[0]), v(part[1])), pos2(u(part[2]), v(part[3]))), rect)
}

/// Where to cut when sources on air have just dropped: the first one's
/// chosen scene, unless it's on already or needs a source with no signal too.
fn cut_after_drop(dropped: &[String], sources: &[crate::config::SourceConfig], scenes: &[Scene], offline: &[bool], live: &str) -> Option<Scene> {
    let target = dropped.iter().filter_map(|n| sources.iter().find(|s| &s.name == n)).map(|s| s.on_drop_scene.as_str()).find(|t| !t.is_empty())?;
    let scene = scenes.iter().find(|s| s.name == target)?;
    let blind = scene.sources().iter().any(|i| offline.get(*i).copied().unwrap_or(false));
    (scene.name != live && !blind).then(|| scene.clone())
}

/// The time of day for the header: "15:05", or "3:05 PM".
fn clock_text(t: chrono::NaiveTime, twelve: bool) -> String {
    if twelve { t.format("%-I:%M %p").to_string() } else { t.format("%H:%M").to_string() }
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
    /// Every scene in view on a laptop screen (forensics bank #1): the
    /// widths are what's left beside the transitions at 1280 and 1024.
    #[test]
    fn every_scene_fits_the_bank() {
        use super::{bank_layout, BANK_FIT};
        let (gap, gap_y, h) = (8.0, 4.0, 92.0);
        for (n, width) in [(7, 900.0), (9, 900.0), (7, 640.0), (9, 640.0), (BANK_FIT, 640.0)] {
            let l = bank_layout(n, width, h, gap, gap_y, false);
            let per_row = n.div_ceil(l.rows);
            assert!(per_row as f32 * (l.button.x + gap) - gap <= width + 0.5, "{n} scenes in {width}: {l:?}");
            assert!(l.rows as f32 * (l.button.y + gap_y) - gap_y <= h + 0.5, "{n} scenes in {width}: {l:?}");
        }
        // Pictures while there's room; a long bank scrolls rather than shrink to nothing.
        assert!(bank_layout(7, 900.0, h, gap, gap_y, false).thumbs);
        assert!(!bank_layout(7, 640.0, h, gap, gap_y, false).thumbs);
        assert!(bank_layout(20, 640.0, h, gap, gap_y, false).button.x >= 120.0);
    }

    #[test]
    fn quit_says_what_stops() {
        assert_eq!(super::quit_text(true, false, None).0, "Quit while recording?");
        assert_eq!(super::quit_text(false, true, None).0, "Quit while live?");
        assert!(super::quit_text(true, true, None).1.contains("broadcast ends"));
        let busy = super::quit_text(false, false, Some("uploading 2 files".into()));
        assert_eq!(busy.0, "Quit before Cutaway has finished?");
        assert!(busy.1.contains("still uploading 2 files"));
    }

    #[test]
    fn first_window_fits_the_screen() {
        use eframe::egui::vec2;
        // A 13" MacBook Air: 1470 × 956 points.
        let s = super::first_window_size(vec2(1480.0, 920.0), vec2(1470.0, 956.0));
        assert!(s.x <= 1430.0 && s.y <= 816.0 && s.x >= 1000.0 && s.y >= 640.0, "{s:?}");
        // A big screen: the usual size.
        assert_eq!(super::first_window_size(vec2(1480.0, 920.0), vec2(2560.0, 1440.0)), vec2(1480.0, 920.0));
        // Never below the smallest.
        assert_eq!(super::first_window_size(vec2(1480.0, 920.0), vec2(800.0, 600.0)), vec2(1000.0, 640.0));
    }

    #[test]
    fn precheck_verdict_reads_worst_level() {
        use crate::engine::precheck::{CheckItem, Level};
        let it = |level| CheckItem { level, what: "x".into(), detail: String::new() };
        assert_eq!(super::precheck_verdict(&[it(Level::Ok)], true).0, "Ready to record");
        assert_eq!(super::precheck_verdict(&[it(Level::Ok), it(Level::Warn)], true).0, "1 warning: you can still start");
        assert_eq!(super::precheck_verdict(&[it(Level::Warn), it(Level::Warn), it(Level::Ok)], true).0, "2 warnings: you can still start");
        assert_eq!(super::precheck_verdict(&[it(Level::Warn), it(Level::Fail)], true).0, "Fix before going live");
    }

    #[test]
    fn dropped_scenes_land_where_the_bar_was() {
        use super::moved_to;
        // Four scenes: the first dropped before the third ends up second.
        assert_eq!(moved_to(0, 2, 4), Some(1));
        assert_eq!(moved_to(0, 4, 4), Some(3), "after the last");
        assert_eq!(moved_to(3, 0, 4), Some(0));
        assert_eq!(moved_to(1, 1, 4), None, "before itself");
        assert_eq!(moved_to(1, 2, 4), None, "after itself");
        assert_eq!(moved_to(5, 0, 4), None);
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

/// The one-line verdict under the pre-show check's heading, from its worst result so far.
fn precheck_verdict(items: &[CheckItem], done: bool) -> (String, Color32) {
    let worst = items.iter().map(|i| i.level).max().unwrap_or(Level::Ok);
    let warnings = items.iter().filter(|i| i.level == Level::Warn).count();
    match worst {
        Level::Fail => ("Fix before going live".into(), ERROR),
        Level::Warn => (format!("{warnings} {}: you can still start", if warnings == 1 { "warning" } else { "warnings" }), AMBER),
        Level::Ok if done => ("Ready to record".into(), GREEN),
        Level::Ok => ("Checking…".into(), TEXT_DIM),
    }
}

#[cfg(test)]
mod desk_tests {
    use super::clock_text;
    use chrono::NaiveTime;

    #[test]
    fn cuts_away_from_a_dropped_source_once_and_safely() {
        use super::{cut_after_drop, Scene};
        use crate::config::SourceConfig;
        let names: Vec<String> = ["Cam 1", "Cam 2", "Cam 3"].iter().map(|s| s.to_string()).collect();
        let scenes: Vec<Scene> = crate::scene::presets(&names);
        let wide = scenes[0].name.clone();
        let mut sources: Vec<SourceConfig> = names.iter().map(|n| SourceConfig { name: n.clone(), ..Default::default() }).collect();
        let none = [false, true, false];
        // Not set: stays.
        assert!(cut_after_drop(&["Cam 2".into()], &sources, &scenes, &none, &scenes[1].name).is_none());
        sources[1].on_drop_scene = wide.clone();
        assert_eq!(cut_after_drop(&["Cam 2".into()], &sources, &scenes, &none, &scenes[1].name).map(|s| s.name), Some(wide.clone()));
        // Already on: nothing to do.
        assert!(cut_after_drop(&["Cam 2".into()], &sources, &scenes, &none, &wide).is_none());
        // The chosen scene has no signal either.
        assert!(cut_after_drop(&["Cam 2".into()], &sources, &scenes, &[true, true, false], &scenes[1].name).is_none());
        // A scene that's gone.
        sources[1].on_drop_scene = "Nowhere".into();
        assert!(cut_after_drop(&["Cam 2".into()], &sources, &scenes, &none, &scenes[1].name).is_none());
    }

    #[test]
    fn scene_thumbnails_crop_and_fill_as_on_air() {
        use super::{thumb_view, Layer};
        // Full frame, no crop: the whole texture over the whole canvas.
        let full = Layer::at(0, 0.0, 0.0, 1.0, 1.0);
        let (uv, r) = thumb_view([0.0, 0.0, 1.0, 1.0], 16.0 / 9.0, &full, (1920, 1080));
        assert_eq!((uv.min.x, uv.min.y, uv.max.x, uv.max.y), (0.0, 0.0, 1.0, 1.0));
        assert_eq!(r, (0.0, 0.0, 1920.0, 1080.0));
        // Cropped to its left half (taller than square), filling a square frame: all of that half's width, its middle rows.
        let mut sq = Layer::at(0, 0.0, 0.0, 0.5, 1.0);
        sq.w = 1080.0 / 1920.0;
        sq.fill = true;
        let (uv, r) = thumb_view([0.0, 0.0, 0.5, 1.0], 8.0 / 9.0, &sq, (1920, 1080));
        assert!((r.2 - 1080.0).abs() < 0.5 && (r.3 - 1080.0).abs() < 0.5);
        assert_eq!((uv.min.x, uv.max.x), (0.0, 0.5));
        assert!((uv.min.y - 1.0 / 18.0).abs() < 1e-4 && (uv.max.y - 17.0 / 18.0).abs() < 1e-4, "{uv:?}");
    }

    #[test]
    fn header_clock_in_12_and_24_hours() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(clock_text(t(15, 5), false), "15:05");
        assert_eq!(clock_text(t(15, 5), true), "3:05 PM");
        assert_eq!(clock_text(t(0, 30), true), "12:30 AM");
        assert_eq!(clock_text(t(9, 0), false), "09:00");
    }
}
