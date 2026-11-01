//! The window's frame in the calm redesign: the rail of areas down the
//! left, the top bar with the open area's tabs, and the status cluster at
//! the top right that never moves (one status light, the time left on the
//! drive, the clock, REC and LIVE).
//!
//! Everything about where things are is read from nav.rs. Show is the live
//! desk; the other areas open the Settings pages that hold their settings,
//! drawn inside the window (the Settings window's own sidebar and title
//! bar aren't used), until each area gets pages of its own.

use super::nav::{self, Icon, AREAS};
use super::theme::*;
use super::{fmt_clock, App, REC_HOLD_SECS};
use crate::control::shortcuts::{Builtin, KeyCombo, DIGITS};
use crate::control::{self, Action};
use crate::engine::streamer::DestState;
use crate::engine::{LogLevel, RecState};
use crate::settings::{Section, SettingsAction};
use crate::util::LockExt;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The rail's width with labels under the icons, and icons only (below
/// RAIL_LABELS_FROM wide).
pub(super) const RAIL_WIDE: f32 = 72.0;
pub(super) const RAIL_NARROW: f32 = 52.0;
const RAIL_LABELS_FROM: f32 = 1200.0;
/// The top bar's height.
pub(super) const TOP_BAR: f32 = 54.0;

/// Which part of the status light's panel shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StatusTab {
    Now,
    Log,
}

/// Where the rail and tabs are.
pub(super) struct Shell {
    /// The area open (in nav::AREAS). Cutaway always opens on Show.
    area: usize,
    /// Each area's tab and section last open, by id (remembered on this
    /// computer, not with the show).
    last: HashMap<String, (String, String)>,
    /// The last area open other than Show (⌘, goes back to it).
    last_page_area: Option<usize>,
    /// The Settings page the open area page last showed, to notice when
    /// something else (a link, ⌘K search, a problem) goes to another.
    shown_section: Option<Section>,
    /// The status light's panel: open it on this tab next frame.
    pub open_status: Option<StatusTab>,
    pub status_tab: StatusTab,
    /// When Record › Takes last looked for takes.
    takes_looked: Option<Instant>,
    /// What's picked on an area's page, by name ("sources" → the source),
    /// so coming back lands on it (remembered on this computer).
    pub picked: HashMap<String, String>,
}

/// What's remembered between runs (in "Main window.ron", beside the
/// window's size): each area's last tab and section.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Saved {
    tabs: Vec<(String, String, String)>,
    #[serde(default)]
    picked: Vec<(String, String)>,
}

const SAVE_KEY: &str = "nav";

impl Shell {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Shell {
        let saved: Saved = storage.and_then(|s| eframe::get_value(s, SAVE_KEY)).unwrap_or_default();
        let last = saved.tabs.into_iter().filter(|(a, t, s)| nav::place_exists(&format!("{a}/{t}/{s}"))).map(|(a, t, s)| (a, (t, s))).collect();
        let picked = saved.picked.into_iter().collect();
        Shell { area: 0, last, last_page_area: None, shown_section: None, open_status: None, status_tab: StatusTab::Now, takes_looked: None, picked }
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        let mut tabs: Vec<(String, String, String)> = self.last.iter().map(|(a, (t, s))| (a.clone(), t.clone(), s.clone())).collect();
        tabs.sort();
        let mut picked: Vec<(String, String)> = self.picked.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        picked.sort();
        eframe::set_value(storage, SAVE_KEY, &Saved { tabs, picked });
    }

    pub fn on_show(&self) -> bool {
        self.area == 0
    }

    /// The open area, tab and section (indexes into nav::AREAS).
    fn place(&self) -> (usize, usize, usize) {
        place_in(self.area, &self.last)
    }
}

/// An area's remembered tab and section, or its first.
fn place_in(area: usize, last: &HashMap<String, (String, String)>) -> (usize, usize, usize) {
    let a = &AREAS[area];
    let (t, s) = last
        .get(a.id)
        .and_then(|(t, s)| {
            let ti = a.tabs.iter().position(|x| x.id == t)?;
            let si = a.tabs[ti].sections.iter().position(|x| x.id == s).unwrap_or(0);
            Some((ti, si))
        })
        .unwrap_or((0, 0));
    (area, t, s)
}

/// What an area page shows for one of its sections.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Target {
    /// A Settings page, with one of its folded parts opened.
    Page(Section, Option<&'static str>),
    /// Record › Takes: the takes so far.
    Takes,
    /// Record › Recording › Before you start: the pre-show check.
    Check,
    /// More › App › Updates & about.
    About,
    /// More › App › Help: the first-show guide and the keys.
    Help,
    /// Sources › People & names: who's in each picture (drawn by the app,
    /// people.rs).
    People,
}

/// The section at "area/tab/section", and what it shows.
pub(super) fn target(area: &str, tab: &str, sect: &str) -> Option<Target> {
    use Section as P;
    let custom = match (area, tab, sect) {
        ("record", "takes", _) => Some(Target::Takes),
        ("record", "recording", "check") => Some(Target::Check),
        ("more", "app", "about") => Some(Target::About),
        ("more", "app", "help") => Some(Target::Help),
        ("sources", "people", _) => Some(Target::People),
        _ => None,
    };
    if custom.is_some() {
        return custom;
    }
    // The part of the old page each section is (its folded part's id).
    let part = match (area, tab, sect) {
        ("scenes", "auto", _) => Some("auto"),
        ("scenes", "stinger", _) => Some("stinger"),
        ("sound", "phones", _) => Some("mix_phones"),
        ("graphics", "turns", _) => Some("rotations"),
        ("record", "recording", "quality") => Some("rec_quality"),
        ("record", "recording", "editing") => Some("rec_editing"),
        ("record", "recording", "podcast") => Some("rec_podcast"),
        ("record", "screens", _) => Some("screen2"),
        ("record", "scheduled", _) => Some("scheduled"),
        ("more", "control", "remote") => Some("remote"),
        ("more", "control", "streamdeck") => Some("streamdeck"),
        ("more", "control", "oscmidi") => Some("osc"),
        ("more", "tools", "replay") => Some("replay"),
        _ => None,
    };
    let s = nav::area(area)?.tabs.iter().find(|t| t.id == tab)?.sections.iter().find(|s| s.id == sect)?;
    let page: P = *s.help.first()?;
    Some(Target::Page(page, part))
}

/// Where each old Settings page lives now ("area/tab/section"): where a
/// link, ⌘K search or a problem that names the page goes.
pub(super) fn home_of(page: Section) -> &'static str {
    use Section as P;
    match page {
        P::General => "more/app/general",
        P::Sources => "sources/cameras/list",
        P::Audio => "sound/inputs/inputs",
        P::Mixing => "sound/mixer/mix",
        P::Pads => "sound/pads/pads",
        P::Syrinscape => "sound/syrinscape/syrinscape",
        P::Program => "scenes/program/picture",
        P::Switching => "scenes/switching/transition",
        P::Scenes => "scenes/scenes/list",
        P::Graphics => "graphics/titles/list",
        P::Rundown => "more/tools/rundown",
        P::Macros => "more/tools/macros",
        P::Prompter => "more/tools/prompter",
        P::Comments => "more/tools/comments",
        P::Guests => "sources/guests/guests",
        P::Captions => "graphics/captions/captions",
        P::Recording => "record/recording/quality",
        P::Streaming => "record/streaming/where",
        P::Vertical => "record/vertical/vertical",
        P::Upload => "record/upload/upload",
        P::Outputs => "record/screens/screens",
        P::Control => "more/control/remote",
        P::Shortcuts => "more/control/shortcuts",
        P::Diagnostics => "more/app/diagnostics",
    }
}

/// "area/tab/section" as indexes into nav::AREAS.
pub(super) fn indexes(path: &str) -> Option<(usize, usize, usize)> {
    let mut p = path.split('/');
    let (a, t, s) = (p.next()?, p.next()?, p.next()?);
    let ai = AREAS.iter().position(|x| x.id == a)?;
    let ti = AREAS[ai].tabs.iter().position(|x| x.id == t)?;
    let si = AREAS[ai].tabs[ti].sections.iter().position(|x| x.id == s)?;
    Some((ai, ti, si))
}

pub(super) fn target_at(a: usize, t: usize, s: usize) -> Option<Target> {
    let (area, tab) = (&AREAS[a], &AREAS[a].tabs[t]);
    target(area.id, tab.id, tab.sections[s].id)
}

/// The status light's words: "All good", or how many things to look at.
pub(super) fn light_text(to_check: usize) -> String {
    match to_check {
        0 => "All good".into(),
        n => format!("{n} to check"),
    }
}

/// ⌘1…⌘7 ("Ctrl+1" on Windows): the key for an area.
fn area_key(n: u8) -> Option<KeyCombo> {
    let key = *DIGITS.get((n as usize).checked_sub(1)?)?;
    Some(KeyCombo { command: true, ..KeyCombo::plain(key) })
}

impl App {
    pub(super) fn on_show(&self) -> bool {
        self.shell.on_show()
    }

    // ---- going places ---------------------------------------------------------------

    /// Open a Settings page where it lives now (an area page), keeping any
    /// changes not applied yet; `part`: one of its folded parts, opened.
    pub(super) fn open_page(&mut self, page: Section, part: Option<&str>) {
        self.flush_live();
        match self.settings.as_mut() {
            Some(win) => win.go_to(page),
            None => self.settings = Some(self.new_settings().open_at(page)),
        }
        if let Some(p) = part {
            if let Some(win) = &self.settings {
                win.unfold(p);
            }
        }
        // The section that's this part of the page, if one is; else the page's home.
        let at = part.and_then(|p| sections().find(|&(a, t, s)| matches!(target_at(a, t, s), Some(Target::Page(pg, Some(q))) if pg == page && q == p)));
        if let Some((a, t, s)) = at.or_else(|| indexes(home_of(page))) {
            self.set_place(a, t, s);
        }
        self.shell.shown_section = Some(page);
    }

    /// Go to "area/tab/section".
    pub(super) fn go(&mut self, path: &str) {
        if let Some((a, t, s)) = indexes(path) {
            self.go_to(a, t, s);
        }
    }

    fn set_place(&mut self, a: usize, t: usize, s: usize) {
        let area = &AREAS[a];
        self.shell.area = a;
        if a != 0 {
            self.shell.last_page_area = Some(a);
            self.shell.last.insert(area.id.to_string(), (area.tabs[t].id.to_string(), area.tabs[t].sections[s].id.to_string()));
        }
    }

    /// Go to an area's tab and section (Show: its Switch or Edit layout tab).
    pub(super) fn go_to(&mut self, a: usize, t: usize, s: usize) {
        if a == 0 {
            if self.leave_page() {
                self.edit_mode = AREAS[0].tabs[t].id == "edit";
            }
            return;
        }
        self.flush_live();
        self.set_place(a, t, s);
        match target_at(a, t, s) {
            Some(Target::Page(page, part)) => {
                match self.settings.as_mut() {
                    Some(win) => win.go_to(page),
                    None => self.settings = Some(self.new_settings().open_at(page)),
                }
                if let (Some(p), Some(win)) = (part, &self.settings) {
                    win.unfold(p);
                }
                self.shell.shown_section = Some(page);
            }
            Some(Target::Takes) => self.shell.takes_looked = None,
            _ => {}
        }
    }

    /// Open an area where it was left.
    pub(super) fn go_area(&mut self, a: usize) {
        if a >= AREAS.len() {
            return;
        }
        if a == 0 {
            self.leave_page();
            return;
        }
        let (_, t, s) = place_in(a, &self.shell.last);
        self.go_to(a, t, s);
    }

    /// Back to Show. With Settings changes not applied yet, asks first
    /// (on their page); true once on Show.
    pub(super) fn leave_page(&mut self) -> bool {
        if self.shell.area == 0 {
            return true;
        }
        self.flush_live();
        if let Some(win) = self.settings.as_mut() {
            if !win.leave() {
                // The question shows on the page with the changes.
                let page = win.section();
                if let Some((a, t, s)) = indexes(home_of(page)) {
                    if !matches!(target_at(self.shell.area, self.place().1, self.place().2), Some(Target::Page(p, _)) if p == page) {
                        self.set_place(a, t, s);
                    }
                }
                return false;
            }
        }
        self.settings = None;
        self.shell.shown_section = None;
        self.shell.area = 0;
        true
    }

    fn place(&self) -> (usize, usize, usize) {
        self.shell.place()
    }

    /// Going somewhere else: what a page that saves as you go still has
    /// waiting (a box being typed in) is applied, at the start of the next
    /// frame (settings/live.rs).
    pub(super) fn flush_live(&mut self) {
        if let Some(a) = self.settings.as_mut().and_then(|w| w.take_live()) {
            self.deferred_settings.push(a);
        }
    }

    /// ⌘, : the area last open other than Show (Sources the first time).
    pub(super) fn open_settings(&mut self) {
        let a = self.shell.last_page_area.unwrap_or(1);
        self.go_area(a);
    }

    /// Each frame, before drawing: a page opened from elsewhere (a "Set
    /// up" link, the guide, ⌘K search, a problem's link) takes the
    /// rail and tabs to where that page lives now.
    pub(super) fn sync_shell(&mut self) {
        let now = self.settings.as_ref().map(|w| w.section());
        if now == self.shell.shown_section {
            return;
        }
        match now {
            Some(page) => {
                let here = self.place();
                let showing = self.shell.area != 0 && matches!(target_at(here.0, here.1, here.2), Some(Target::Page(p, _)) if p == page);
                if !showing {
                    if let Some((a, t, s)) = indexes(home_of(page)) {
                        self.set_place(a, t, s);
                    }
                }
                self.shell.shown_section = Some(page);
            }
            None => self.shell.shown_section = None,
        }
        // A page with no Settings left open (closed by OK, Cancel or
        // Discard): back to Show.
        if self.settings.is_none() && self.shell.area != 0 && matches!(self.page_target(), Some(Target::Page(..))) {
            self.shell.area = 0;
        }
    }

    fn page_target(&self) -> Option<Target> {
        let (a, t, s) = self.place();
        target_at(a, t, s)
    }

    /// ⌘1–⌘7 (when not given to anything else) and Esc (not while typing:
    /// that's handled in handle_keys' early return). `menu_open`: a menu is
    /// up, which Esc closes by itself.
    pub(super) fn shell_key(&mut self, combo: KeyCombo, menu_open: bool) -> bool {
        // Esc in Edit layout: done, back to Switch.
        if combo == KeyCombo::plain(egui::Key::Escape) && self.on_show() && self.edit_mode {
            if !menu_open {
                self.edit_mode = false;
            }
            return true;
        }
        if combo.command && !combo.shift && !combo.alt {
            if let Some(i) = DIGITS.iter().position(|k| *k == combo.key) {
                if let Some(a) = AREAS.iter().position(|a| a.key == Some(i as u8 + 1)) {
                    self.go_area(a);
                    return true;
                }
            }
        }
        // Esc on a page that isn't a Settings page (those ask about
        // changes themselves): back to Show.
        if combo == KeyCombo::plain(egui::Key::Escape) && !self.on_show() && !matches!(self.page_target(), Some(Target::Page(..))) {
            self.leave_page();
            return true;
        }
        false
    }

    // ---- the rail -------------------------------------------------------------------

    pub(super) fn rail_width(ctx: &egui::Context) -> f32 {
        if ctx.content_rect().width() >= RAIL_LABELS_FROM { RAIL_WIDE } else { RAIL_NARROW }
    }

    pub(super) fn rail(&mut self, ui: &mut egui::Ui) {
        let wide = ui.available_width() >= RAIL_WIDE - 1.0;
        let w = ui.available_width();
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            // The logo: About and updates.
            let logo = egui::Button::image(egui::Image::new(&self.logo).max_height(26.0)).frame(false);
            if ui.add(logo).on_hover_text("About Cutaway (More › App › Updates & about)").clicked() {
                self.go("more/app/about");
            }
            self.show_menu(ui, w - 6.0);
            ui.add_space(6.0);
        });
        let (here, _, _) = self.place();
        // The areas with changes not applied yet.
        let dirty: Vec<usize> = match &self.settings {
            Some(w) if w.dirty() => Section::ALL.into_iter().filter(|p| w.section_dirty(*p)).filter_map(|p| indexes(home_of(p))).map(|(a, _, _)| a).collect(),
            _ => Vec::new(),
        };
        let update = self.update.lock_ok().as_ref().is_some_and(|r| r.version != self.cfg.ui.skip_version);
        let bottom = ui.max_rect().bottom();
        let item_h = if wide { 54.0 } else { 44.0 };
        let mut go = None;
        for (i, area) in AREAS.iter().enumerate() {
            // More sits at the bottom.
            let top = if area.id == "more" { bottom - item_h - 8.0 } else { ui.cursor().top() };
            let rect = Rect::from_min_size(pos2(ui.max_rect().left(), top), vec2(w, item_h));
            let r = ui.interact(rect, ui.make_persistent_id(("rail", area.id)), Sense::click());
            if area.id != "more" {
                ui.advance_cursor_after_rect(rect);
            }
            let on = here == i;
            let p = ui.painter();
            if on {
                p.rect_filled(rect.shrink2(vec2(6.0, 2.0)), 8.0, RAISED);
                p.rect_filled(Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.0, rect.height() - 20.0)), 1.5, ACCENT);
            } else if r.hovered() {
                p.rect_filled(rect.shrink2(vec2(6.0, 2.0)), 8.0, NEUTRAL_HOVER.gamma_multiply(0.5));
            }
            let color = if on { Color32::WHITE } else { TEXT_DIM };
            let icon_c = if wide { rect.center_top() + vec2(0.0, 18.0) } else { rect.center() };
            draw_icon(p, area.icon, icon_c, color);
            if wide {
                p.text(rect.center_bottom() - vec2(0.0, 8.0), Align2::CENTER_BOTTOM, area.label, FontId::proportional(TEXT_SMALL), color);
            }
            // A dot: changes not applied yet in this area, or a new version (More).
            let mark = dirty.contains(&i) || (area.id == "more" && update);
            if mark {
                p.circle_filled(icon_c + vec2(12.0, -10.0), 4.0, if area.id == "more" && update { ACCENT } else { AMBER });
            }
            let key = area.key.and_then(area_key).map(|k| format!(" ({})", k.label())).unwrap_or_default();
            let tip = match (area.id, mark) {
                ("more", true) if update => format!("{}{key}\nA new version of Cutaway is out: More › App › Updates & about", area.label),
                (_, true) => format!("{}{key}\nChanges here aren't applied yet", area.label),
                _ => format!("{}{key}", area.label),
            };
            if r.on_hover_text(tip).clicked() {
                go = Some(i);
            }
        }
        if let Some(i) = go {
            if i == 0 && self.on_show() && self.edit_mode {
                // Show again from Edit layout: back to switching.
                self.edit_mode = false;
            }
            self.go_area(i);
        }
    }

    // ---- the top bar ----------------------------------------------------------------

    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.status_cluster(ui, width);
                // ⌘K search, between the tabs and the status cluster.
                self.search_button(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| self.tab_row(ui));
            });
        });
        // The clock turns over on the minute.
        let secs_left = 60 - chrono::Timelike::second(&chrono::Local::now()) as u64;
        ui.ctx().request_repaint_after(Duration::from_secs(secs_left.clamp(1, 60)));
    }

    /// The open area's tabs, as many as fit; the rest (and the rarely used
    /// ones) under More ▾. Then a quiet line about what's under way.
    fn tab_row(&mut self, ui: &mut egui::Ui) {
        let (a, t, _) = self.place();
        let area = &AREAS[a];
        // Show's tabs follow Edit layout (E turns it on and off too).
        let active = if a == 0 { if self.edit_mode { 1 } else { 0 } } else { t };
        let font = FontId::proportional(TEXT_STATE);
        let pad = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
        let width_of = |ui: &egui::Ui, text: &str| ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x + pad;
        let more_w = width_of(ui, "More ▾") + 12.0;
        let mut room = ui.available_width() - more_w;
        let mut shown = Vec::new();
        let mut under_more = Vec::new();
        for (i, tab) in area.tabs.iter().enumerate() {
            let w = width_of(ui, tab.label);
            if !tab.more && w <= room && under_more.is_empty() {
                room -= w;
                shown.push(i);
            } else {
                under_more.push(i);
            }
        }
        let mut go = None;
        for &i in &shown {
            let b = egui::Button::selectable(i == active, RichText::new(area.tabs[i].label).size(TEXT_STATE)).min_size(vec2(0.0, 30.0));
            let mut r = ui.add(b);
            if a == 0 && area.tabs[i].id == "edit" {
                let what = if self.preview_mode() { "Move and resize the frames of the scene in preview" } else { "Move and resize the frames of the scene on air" };
                r = r.on_hover_text(format!("{what}{}. Esc or Done comes back to Switch", self.key_hint(Builtin::Edit)));
            }
            if r.clicked() {
                go = Some(i);
            }
        }
        if !under_more.is_empty() {
            let current = under_more.contains(&active).then(|| area.tabs[active].label);
            let text = match current {
                Some(l) => format!("{l} ▾"),
                None => "More ▾".to_string(),
            };
            let text = RichText::new(text).size(TEXT_STATE).color(if current.is_some() { Color32::WHITE } else { TEXT_DIM });
            ui.menu_button(text, |ui| {
                for &i in &under_more {
                    if ui.selectable_label(i == active, area.tabs[i].label).clicked() {
                        go = Some(i);
                        ui.close();
                    }
                }
            });
        }
        if let Some(i) = go {
            if a == 0 {
                self.edit_mode = area.tabs[i].id == "edit";
            } else {
                let s = if i == t { self.place().2 } else { 0 };
                self.go_to(a, i, s);
            }
        }
        // What's under way, in what's left (never over the status cluster).
        if let Some((text, color)) = self.note() {
            if ui.available_width() > 90.0 {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(&text).color(color).size(TEXT_BODY)).truncate()).on_hover_text(text);
            }
        }
    }

    /// A line about something under way: saving, a podcast being made,
    /// connecting, a destination down, a scheduled stream coming up.
    fn note(&self) -> Option<(String, Color32)> {
        let e = self.engine.as_ref()?;
        let (rec, stream, background) = {
            let st = e.status.lock_ok();
            (st.rec.clone(), st.stream.clone(), st.background.clone())
        };
        if let RecState::Finalizing { step, .. } = &rec {
            return Some((step.clone(), ACCENT));
        }
        if let Some((what, f)) = &background {
            return Some((format!("{what}… {:.0}%", f * 100.0), ACCENT));
        }
        if e.is_streaming() && !stream.stopping {
            let live = stream.destinations.iter().filter(|d| d.state == DestState::Live).count();
            let total = stream.destinations.iter().filter(|d| d.state != DestState::Off).count();
            let connecting: Vec<&str> = stream.destinations.iter().filter(|d| matches!(d.state, DestState::Connecting | DestState::Retrying { .. })).map(|d| d.name.as_str()).collect();
            if live == 0 && !connecting.is_empty() {
                return Some((format!("Connecting to {}", connecting.join(", ")), LIVE_SOFT));
            }
            if live < total {
                return Some((format!("{live} of {total} destinations live — see the Stream panel"), AMBER));
            }
            return None;
        }
        self.scheduled_line()
    }

    // ---- the status cluster ---------------------------------------------------------

    /// Right to left: LIVE, ✂ Clip, ◆ Marker, REC, the clock, the time left,
    /// the status light, and "Typing" when a box has the keyboard. Drawn
    /// first, so the tabs only get what it leaves: it never moves or
    /// overlaps anything.
    fn status_cluster(&mut self, ui: &mut egui::Ui, bar_width: f32) {
        let roomy = bar_width >= 1100.0;
        let card = self.engine.as_ref().map(|e| {
            let st = e.status.lock_ok();
            (matches!(st.rec, RecState::Recording { .. }), st.last_take.clone())
        });
        if let Some((recording, last)) = &card {
            self.tick_take_card(*recording, last.as_ref());
        }
        let Some(engine) = &self.engine else {
            self.status_light(ui);
            return;
        };
        let (rec, stream, markers, disk, backup) = {
            let st = engine.status.lock_ok();
            (st.rec.clone(), st.stream.clone(), st.markers, st.disk.clone(), st.backup_disk.clone())
        };
        let streaming = engine.is_streaming();
        let take_markers = engine.take_markers();

        // ---- LIVE (once there's somewhere to stream to)
        self.live_pill(ui, &stream, streaming, roomy);

        // ---- ✂ and ◆ while recording, then REC
        let mut marker_clicked = false;
        let mut clip_clicked = false;
        let mut named = false;
        if matches!(rec, RecState::Recording { .. }) {
            let clip = if roomy { "✂ Clip" } else { "✂" };
            if ui
                .add(egui::Button::new(clip).min_size(vec2(0.0, 30.0)))
                .on_hover_text(format!("Save the last {} seconds as a clip in the take's Clips folder{}", self.cfg.output.clip_seconds, self.key_hint(Builtin::Clip)))
                .clicked()
            {
                clip_clicked = true;
            }
            let gap = ui.spacing().item_spacing.x;
            ui.spacing_mut().item_spacing.x = 2.0;
            let list = ui.add(egui::Button::new("▾").min_size(vec2(22.0, 30.0))).on_hover_text("This take's markers: name one, rename or take them out");
            let mtext = match (roomy, markers) {
                (true, 0) => "◆ Marker".to_string(),
                (true, n) => format!("◆ Marker ({n})"),
                (false, 0) => "◆".to_string(),
                (false, n) => format!("◆ {n}"),
            };
            let named_key = self.key_label_or(Builtin::NamedMarker, "");
            if ui
                .add(egui::Button::new(mtext).min_size(vec2(0.0, 30.0)))
                .on_hover_text(format!(
                    "Mark this moment{}: becomes a chapter and a timeline marker.{}",
                    self.key_hint(Builtin::Marker),
                    if named_key.is_empty() { " ▾ adds one and asks for its name".to_string() } else { format!(" {named_key} (or ▾) adds one and asks for its name") }
                ))
                .clicked()
            {
                marker_clicked = true;
            }
            ui.spacing_mut().item_spacing.x = gap;
            if std::mem::take(&mut self.test_marker_list) {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&list));
            }
            egui::Popup::from_toggle_button_response(&list).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                if ui.button(format!("◆ Named marker…{}", self.key_hint(Builtin::NamedMarker))).on_hover_text("Add a marker now, then type its name").clicked() {
                    named = true;
                    ui.close();
                }
                ui.separator();
                self.marker_list(ui, &take_markers);
            });
        }
        let rec_rect = self.rec_pill(ui, &rec, roomy);
        if self.on_show() {
            if let Some((_, last)) = &card {
                self.take_card(ui.ctx(), rec_rect, last.as_ref());
            }
        }

        // ---- the clock
        ui.add_space(4.0);
        let clock = super::clock_text(chrono::Local::now().naive_local().time(), self.cfg.ui.clock_12h);
        let b = egui::Button::new(RichText::new(clock).size(TEXT_STATE).color(TEXT_DIM)).frame(false);
        let tip = if self.cfg.ui.clock_12h { "The time here. Click for 24-hour time (clock graphics and the second screen follow)" } else { "The time here. Click for 12-hour time (clock graphics and the second screen follow)" };
        if ui.add(b).on_hover_text(tip).clicked() {
            self.set_clock_12h(!self.cfg.ui.clock_12h);
        }

        // ---- the time left on the drive
        if let Some(d) = &disk {
            let (first, second) = match d.left_text() {
                Some(left) => (left, Some(d.free_text())),
                None => (d.free_text(), None),
            };
            let color = if d.low() { AMBER } else { Color32::WHITE };
            let mut tip = format!(
                "Recording to {}\n{} of {} ({})",
                d.path.display(),
                d.describe(),
                crate::util::human_size(d.total),
                if d.measured { "time left from this take's real write rate" } else { "time left estimated from your recording settings" }
            );
            if let Some(b) = &backup {
                tip.push_str(&format!("\nBackup drive: {}", b.describe()));
            }
            // Two lines, measured first so it gets its whole width (never cut short).
            let font = |size| FontId::proportional(size);
            let g1 = ui.painter().layout_no_wrap(first.clone(), font(TEXT_BODY), color);
            let g2 = second.as_ref().map(|t| ui.painter().layout_no_wrap(t.clone(), font(TEXT_SMALL), TEXT_DIM));
            let w = g1.size().x.max(g2.as_ref().map(|g| g.size().x).unwrap_or(0.0));
            let (rect, r) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
            match g2 {
                Some(g2) => {
                    let top = rect.center().y - (g1.size().y + g2.size().y) / 2.0;
                    ui.painter().galley(pos2(rect.right() - g1.size().x, top), g1.clone(), color);
                    ui.painter().galley(pos2(rect.right() - g2.size().x, top + g1.size().y), g2, TEXT_DIM);
                }
                None => {
                    ui.painter().galley(pos2(rect.right() - g1.size().x, rect.center().y - g1.size().y / 2.0), g1, color);
                }
            }
            r.on_hover_text(tip);
        }

        // ---- the light, and "Typing"
        self.status_light(ui);
        if ui.ctx().egui_wants_keyboard_input() {
            ui.label(RichText::new("⌨ Typing").color(AMBER).size(TEXT_BODY))
                .on_hover_text("A box has the keyboard, so number keys, Space and the rest type into it. Press Esc or click elsewhere to use your show keys again.");
        }

        if marker_clicked {
            match self.add_marker(None) {
                Ok(n) => self.toast(format!("{n} added"), false),
                Err(e) => self.toast(e, true),
            }
        }
        if named {
            self.named_marker();
        }
        if clip_clicked {
            match self.run_action(Action::Clip(None)) {
                Ok(m) => self.toast(m, false),
                Err(e) => self.toast(e, true),
            }
        }
        if let Some(p) = self.engine.as_ref().and_then(|e| e.replay.problem()) {
            if self.replay_problem.as_deref() != Some(p.as_str()) {
                self.replay_problem = Some(p.clone());
                self.toast(p, true);
            }
        }
    }

    /// A marker now, and a box to type its name (as Shift+M).
    pub(super) fn named_marker(&mut self) {
        match self.engine.as_ref().map(|e| e.add_marker_now(None)) {
            Some(Ok(m)) => self.marker_name = Some((m.at, m.name, String::new())),
            Some(Err(err)) => self.toast(format!("{err:#}"), true),
            None => {}
        }
    }

    /// REC: "● Record" until a take starts; then the time, with "hold to
    /// stop" written under it all the time (a click never stops a take);
    /// "Saving 40%" while it finishes. Returns where it is (for the take card).
    fn rec_pill(&mut self, ui: &mut egui::Ui, rec: &RecState, roomy: bool) -> Rect {
        let w = if roomy { 150.0 } else { 122.0 };
        let (rect, r) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
        let p = ui.painter().clone();
        let radius = 20.0;
        match rec {
            RecState::Recording { since, folder } => {
                let t = since.elapsed();
                p.rect_filled(rect, radius, REC_PILL);
                p.rect_stroke(rect, radius, Stroke::new(1.5, REC_EDGE), StrokeKind::Inside);
                let pulse = 0.55 + 0.45 * ((t.as_secs_f32() * std::f32::consts::PI).cos() * 0.5 + 0.5);
                p.circle_filled(rect.left_center() + vec2(18.0, 0.0), 6.0, REC_DOT.gamma_multiply(pulse));
                let down = r.is_pointer_button_down_on() || self.test_hold;
                match (down, self.rec_hold) {
                    (true, _) if self.rec_hold_spent => {}
                    (true, None) => self.rec_hold = Some(Instant::now()),
                    (true, Some(since)) => {
                        let f = (since.elapsed().as_secs_f32() / REC_HOLD_SECS).min(1.0);
                        // A red fill grows across the pill while it's held (see-through, so the text reads).
                        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * f, rect.height()));
                        p.with_clip_rect(fill).rect_filled(rect, radius, REC_HOLD);
                        if f >= 1.0 {
                            self.rec_hold = None;
                            self.rec_hold_spent = true;
                            if let Some(e) = &self.engine {
                                e.stop_recording();
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                    (false, held) => {
                        if held.is_some() {
                            // Let go too soon: nothing happens, but the hint lights up.
                            self.rec_hint_until = Some(Instant::now() + Duration::from_millis(1500));
                        }
                        self.rec_hold = None;
                        self.rec_hold_spent = false;
                    }
                }
                let text_x = rect.left() + 32.0;
                super::bold_text(&p, pos2(text_x, rect.center().y - 7.0), Align2::LEFT_CENTER, &format!("REC {}", fmt_clock(t)), TEXT_STATE, Color32::WHITE);
                let hint_lit = self.rec_hold.is_some() || self.rec_hint_until.is_some_and(|u| Instant::now() < u);
                p.text(pos2(text_x, rect.center().y + 9.0), Align2::LEFT_CENTER, "hold to stop", FontId::proportional(TEXT_SMALL), if hint_lit { Color32::WHITE } else { TEXT_DIM });
                ui.ctx().request_repaint_after(Duration::from_millis(50));
                r.on_hover_text(format!("Hold for a second to stop recording (the key{} asks first)\nRecording to {}", self.key_hint(Builtin::Record), folder.display()));
            }
            RecState::Finalizing { folder, step, done } => {
                p.rect_filled(rect, radius, RAISED);
                // A bar along the bottom fills as it goes.
                let bar = Rect::from_min_size(rect.left_bottom() - vec2(-14.0, 5.0), vec2(rect.width() - 28.0, 3.0));
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * done.clamp(0.02, 1.0), bar.height())), 1.5, ACCENT);
                p.text(rect.center() - vec2(0.0, 2.0), Align2::CENTER_CENTER, format!("Saving {:.0}%", done * 100.0), FontId::proportional(TEXT_STATE), Color32::WHITE);
                r.on_hover_text(format!("{step}\nFinishing the take in {}", folder.display()));
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            RecState::Idle => {
                self.rec_hold_spent = false;
                let fill = if r.hovered() { NEUTRAL_HOVER } else { RAISED };
                p.rect_filled(rect, radius, fill);
                p.rect_stroke(rect, radius, Stroke::new(1.5, REC_EDGE), StrokeKind::Inside);
                p.circle_filled(rect.left_center() + vec2(18.0, 0.0), 6.0, REC_DOT);
                super::bold_text(&p, pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, "Record", TEXT_STATE, Color32::WHITE);
                if r.on_hover_text(format!("Record every source, the program and the audio{}", self.key_hint(Builtin::Record))).clicked() {
                    self.toggle_record();
                }
            }
        }
        rect
    }

    /// LIVE: "◉ Go live" once there's somewhere to stream to; "Going live…"
    /// until a destination is really on air; then the time on air.
    fn live_pill(&mut self, ui: &mut egui::Ui, stream: &crate::engine::streamer::StreamStatus, streaming: bool, roomy: bool) {
        let w = if roomy { 150.0 } else { 122.0 };
        let pill = |text: RichText, fill: Color32| egui::Button::new(text).fill(fill).min_size(vec2(w, 40.0)).corner_radius(20.0);
        if stream.stopping {
            ui.add_enabled(false, pill(RichText::new("Ending…").size(TEXT_STATE).color(Color32::WHITE), RAISED));
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else if streaming {
            let live = stream.destinations.iter().filter(|d| d.state == DestState::Live).count();
            let total = stream.destinations.iter().filter(|d| d.state != DestState::Off).count();
            match stream.live_clock_from() {
                // Nothing's on air yet: say so, not "LIVE".
                None => {
                    let time = ui.input(|i| i.time) as f32;
                    let pulse = ((time * 3.0).sin() + 1.0) * 0.5;
                    let fill = LIVE_DIM.lerp_to_gamma(LIVE_GLOW, pulse);
                    let text = format!("◉ Going live{}", super::waiting::ellipsis(time));
                    if ui.add(pill(RichText::new(text).size(TEXT_STATE).strong().color(Color32::WHITE), fill)).on_hover_text("Connecting. Click to stop.").clicked() {
                        self.toggle_stream();
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                }
                Some(on_air) => {
                    let fill = if live == total && total > 0 { LIVE } else { LIVE_TROUBLE };
                    let text = format!("◉ LIVE {}", fmt_clock(on_air.elapsed()));
                    if ui.add(pill(RichText::new(text).size(TEXT_STATE).strong().color(Color32::WHITE), fill)).on_hover_text(format!("{live} of {total} destinations live. Click to end the stream.")).clicked() {
                        self.toggle_stream();
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
            }
        } else if self.cfg.streaming.destinations.iter().any(|d| d.enabled) {
            let btn = pill(RichText::new("◉ Go live").size(TEXT_STATE).strong().color(LIVE), RAISED).stroke(Stroke::new(1.5, LIVE_EDGE));
            let names: Vec<String> = self.cfg.streaming.destinations.iter().filter(|d| d.enabled).map(|d| d.name.clone()).collect();
            if ui.add(btn).on_hover_text(format!("Stream to {}", names.join(", "))).clicked() {
                self.toggle_stream();
            }
        }
        // (Nowhere to stream to: nothing here. Record › Streaming sets it up.)
    }

    // ---- the status light -----------------------------------------------------------

    /// What the light counts: problems now, and warnings in the log not
    /// looked at yet.
    fn to_check(&self) -> (Vec<String>, usize) {
        let mut problems = Vec::new();
        if let Some(err) = &self.engine_error {
            problems.push(format!("The engine stopped: {err}"));
        }
        let Some(e) = &self.engine else { return (problems, 0) };
        let st = e.status.lock_ok();
        problems.extend(st.health.problems());
        if let Some(r) = &st.rec_failed {
            problems.push(format!("Recording: {r}"));
        }
        if let Some(d) = st.disk.as_ref().filter(|d| d.low()) {
            problems.push(format!("Recording drive: {}", d.describe()));
        }
        if let Some(b) = st.backup_disk.as_ref().filter(|b| b.free < crate::engine::health::MIN_START_BYTES) {
            problems.push(format!("Backup drive: {}", b.describe()));
        }
        let warnings = st.log.iter().filter(|l| l.level != LogLevel::Info && l.seq > self.log_seen).count();
        (problems, warnings)
    }

    /// One light for Health and the Log: "All good" or "N to check". Click
    /// for what's going on now, the log, and the pre-show check.
    fn status_light(&mut self, ui: &mut egui::Ui) {
        let (problems, warnings) = self.to_check();
        let n = problems.len() + warnings;
        let color = if n == 0 { GREEN } else { AMBER };
        let text = light_text(n);
        let galley_w = ui.painter().layout_no_wrap(text.clone(), FontId::proportional(TEXT_BODY), Color32::WHITE).size().x;
        let (rect, r) = ui.allocate_exact_size(vec2(galley_w + 34.0, 30.0), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
        let p = ui.painter();
        if r.hovered() || open {
            p.rect_filled(rect, 15.0, RAISED);
        }
        p.circle_filled(rect.left_center() + vec2(14.0, 0.0), 5.0, color);
        p.text(rect.left_center() + vec2(26.0, 0.0), Align2::LEFT_CENTER, &text, FontId::proportional(TEXT_BODY), if n == 0 { TEXT_DIM } else { AMBER });
        let tip = match (problems.len(), warnings) {
            (0, 0) => "Sources, drives, files and sound are fine. Click for details, the log and the pre-show check".to_string(),
            (p, 0) => format!("{p} problem{} now. Click to see", if p == 1 { "" } else { "s" }),
            (0, w) => format!("{w} new warning{} in the log. Click to see", if w == 1 { "" } else { "s" }),
            (p, w) => format!("{p} problem{} now and {w} new warning{} in the log. Click to see", if p == 1 { "" } else { "s" }, if w == 1 { "" } else { "s" }),
        };
        let r = r.on_hover_text(tip);
        if r.clicked() && !open {
            // Open where there's something to see.
            self.shell.status_tab = if problems.is_empty() && warnings > 0 { StatusTab::Log } else { StatusTab::Now };
        }
        if let Some(tab) = self.shell.open_status.take() {
            self.shell.status_tab = tab;
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&r));
        }
        let mut check = false;
        egui::Popup::from_toggle_button_response(&r)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(500.0)
            .show(|ui| {
                ui.set_width(500.0);
                ui.horizontal(|ui| {
                    let now = if problems.is_empty() { "Now".to_string() } else { format!("Now ({})", problems.len()) };
                    if ui.add(egui::Button::selectable(self.shell.status_tab == StatusTab::Now, now)).clicked() {
                        self.shell.status_tab = StatusTab::Now;
                    }
                    let log = if warnings == 0 { "Log".to_string() } else { format!("Log ({warnings})") };
                    if ui.add(egui::Button::selectable(self.shell.status_tab == StatusTab::Log, log)).clicked() {
                        self.shell.status_tab = StatusTab::Log;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Run pre-show check").on_hover_text("Sources, sound, disk, encoder and stream servers").clicked() {
                            check = true;
                            ui.close();
                        }
                    });
                });
                ui.separator();
                match self.shell.status_tab {
                    StatusTab::Now => {
                        for p in &problems {
                            ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
                        }
                        if !problems.is_empty() {
                            ui.add_space(4.0);
                            ui.separator();
                        }
                        egui::ScrollArea::vertical().id_salt("status_now").max_height(360.0).show(ui, |ui| self.health_body(ui));
                    }
                    StatusTab::Log => self.log_panel(ui),
                }
                ui.separator();
                self.status_tools(ui);
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            });
        if check {
            self.start_precheck(None);
        }
    }

    /// The tools that used to sit in the header: the second screen, the
    /// webcam, replay, the clock's 12/24 hours and the guide.
    fn status_tools(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let screen_on = self.cfg.display.output != crate::config::DisplayOutput::Off;
            if ui.add(egui::Button::selectable(screen_on, "Second screen")).on_hover_text("Program, multiview or prompter on another screen (Record › Screens & webcam). Esc closes it.").clicked() {
                self.cfg.display.output = if screen_on { crate::config::DisplayOutput::Off } else { crate::config::DisplayOutput::Program };
                self.cfg_dirty = true;
            }
            if crate::platform::vcam::available() {
                let on = self.engine.as_ref().is_some_and(|e| e.virtual_camera_on());
                if ui.add(egui::Button::selectable(on, "Webcam")).on_hover_text("Send the program to the \"Cutaway\" camera for Zoom, Teams, Meet…").clicked() {
                    if !on && !crate::platform::vcam::installed() {
                        self.open_page(Section::Outputs, Some("vcam"));
                        ui.close();
                    } else {
                        self.set_virtual_camera(!on);
                    }
                }
            }
            let mut twelve = self.cfg.ui.clock_12h;
            if ui.checkbox(&mut twelve, "12-hour clock").changed() {
                self.set_clock_12h(twelve);
            }
            if ui.button("First-show guide").clicked() {
                self.guide_open = true;
                ui.close();
            }
        });
        self.replay_row(ui);
    }

    /// Instant replay (while recording): play the last seconds again, on
    /// air, slowed down or from one camera.
    fn replay_row(&mut self, ui: &mut egui::Ui) {
        let recording = self.engine.as_ref().is_some_and(|e| e.is_recording());
        let replaying = self.engine.as_ref().is_some_and(|e| e.replay.playing());
        if !recording && !replaying {
            return;
        }
        let angles: Vec<String> = self.engine.as_ref().map(|e| e.sources.iter().filter(|s| s.record).map(|s| s.name.clone()).collect()).unwrap_or_default();
        let mut play: Option<Option<control::ReplayAngle>> = None;
        let mut stop = false;
        ui.horizontal_wrapped(|ui| {
            if replaying {
                if ui.add(egui::Button::new(RichText::new("⟲ Stop the replay").color(Color32::WHITE)).fill(ON_AIR_DIM)).clicked() {
                    stop = true;
                }
                return;
            }
            if ui.button("⟲ Replay").on_hover_text(format!("Play the last {:.0} seconds again, on air", self.cfg.replay.seconds)).clicked() {
                play = Some(None);
            }
            ui.checkbox(&mut self.replay_slow, "Slow");
            ui.label(RichText::new("from").color(TEXT_DIM));
            if ui.small_button("Program").clicked() {
                play = Some(Some(control::ReplayAngle::Program));
            }
            for a in &angles {
                if ui.small_button(a).clicked() {
                    play = Some(Some(control::ReplayAngle::Source(control::Target::Name(a.clone()))));
                }
            }
            if angles.len() >= 2 && ui.small_button(format!("Every angle ({})", angles.len().min(4))).clicked() {
                play = Some(Some(control::ReplayAngle::Quad));
            }
        });
        let cmd = if stop {
            Some(control::ReplayCmd::Stop)
        } else {
            play.map(|angle| control::ReplayCmd::Play { seconds: None, slow: self.replay_slow, angle })
        };
        if let Some(cmd) = cmd {
            if let Err(e) = self.run_action(Action::Replay(cmd)) {
                self.toast(e, true);
            }
        }
    }

    // ---- area pages -----------------------------------------------------------------

    /// The open area's page (any area but Show): its section's Settings page,
    /// or the takes, the pre-show check, About or Help.
    pub(super) fn area_page(&mut self, ui: &mut egui::Ui) {
        let (a, t, s) = self.place();
        let tab = &AREAS[a].tabs[t];
        let here = target_at(a, t, s);
        // The tab's sections, when they show different things.
        let targets: Vec<Option<Target>> = (0..tab.sections.len()).map(|i| target_at(a, t, i)).collect();
        let mut distinct = targets.clone();
        distinct.dedup();
        if distinct.len() > 1 {
            let mut go = None;
            ui.horizontal(|ui| {
                for (i, sect) in tab.sections.iter().enumerate() {
                    if ui.add(egui::Button::selectable(i == s, RichText::new(sect.label).size(TEXT_BODY)).min_size(vec2(0.0, 26.0))).clicked() && i != s {
                        go = Some(i);
                    }
                }
            });
            if let Some(i) = go {
                self.go_to(a, t, i);
                return;
            }
            ui.separator();
        }
        match here {
            Some(Target::Page(page, _)) => {
                if self.settings.is_none() {
                    self.settings = Some(self.new_settings().open_at(page));
                    self.shell.shown_section = Some(page);
                }
                if AREAS[a].tabs[t].sections[s].id == "general" || (page == Section::Graphics && AREAS[a].tabs[t].id == "timers") {
                    self.clock_choice(ui);
                }
                if AREAS[a].tabs[t].id == "screens" {
                    self.second_screen_now(ui);
                }
                if page == Section::Sources {
                    let view = match AREAS[a].tabs[t].id {
                        "closeups" => crate::settings::SourcesView::CloseUps,
                        "network" => crate::settings::SourcesView::Network,
                        "media" => crate::settings::SourcesView::Media,
                        _ => crate::settings::SourcesView::Cameras,
                    };
                    let remembered = self.shell.picked.get("sources").cloned();
                    if let Some(win) = self.settings.as_mut() {
                        win.set_sources_view(view);
                        if let Some(name) = remembered {
                            win.restore_source(&name);
                        }
                    }
                }
                if page == Section::Graphics {
                    let view = match AREAS[a].tabs[t].id {
                        "timers" => crate::settings::GraphicsView::Timers,
                        "turns" => crate::settings::GraphicsView::Turns,
                        "web" => crate::settings::GraphicsView::Web,
                        _ => crate::settings::GraphicsView::Titles,
                    };
                    let remembered = self.shell.picked.get("graphics").cloned();
                    if let Some(win) = self.settings.as_mut() {
                        win.set_graphics_view(view);
                        if let Some(name) = remembered {
                            win.restore_graphic(&name);
                        }
                    }
                }
                if page == Section::Mixing {
                    let view = if AREAS[a].tabs[t].id == "phones" { crate::settings::MixView::Phones } else { crate::settings::MixView::Mixer };
                    if let Some(win) = self.settings.as_mut() {
                        win.set_mix_view(view);
                    }
                }
                if page == Section::Switching {
                    use crate::settings::SwitchView;
                    let view = match (AREAS[a].id, AREAS[a].tabs[t].id) {
                        ("scenes", "auto") => SwitchView::Auto,
                        ("scenes", "stinger") => SwitchView::Stinger,
                        ("more", "tools") => SwitchView::Replay,
                        _ => SwitchView::Transition,
                    };
                    if let Some(win) = self.settings.as_mut() {
                        win.set_switch_view(view);
                    }
                }
                if page == Section::Audio {
                    let remembered = self.shell.picked.get("inputs").cloned();
                    if let (Some(win), Some(name)) = (self.settings.as_mut(), remembered) {
                        win.restore_input(&name);
                    }
                }
                self.settings_page(ui);
            }
            Some(Target::People) => self.people_page(ui),
            Some(Target::Takes) => self.takes_page(ui),
            Some(Target::Check) => self.check_page(ui),
            Some(Target::About) => self.about_page(ui),
            Some(Target::Help) => self.help_page(ui),
            None => {}
        }
    }

    /// The show's one clock format, 12 or 24 hours (as clicking the clock
    /// in the top bar does).
    fn clock_choice(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Times of day").color(TEXT_DIM));
            for (twelve, label) in [(false, "24-hour"), (true, "12-hour")] {
                if ui.add(egui::Button::selectable(self.cfg.ui.clock_12h == twelve, label)).clicked() && self.cfg.ui.clock_12h != twelve {
                    self.remember(super::undo::Kind::Settings, "Clock format");
                    self.set_clock_12h(twelve);
                }
            }
            ui.label(RichText::new("the top bar, clock graphics, the second screen and scheduled times; clicking the clock switches it too").size(TEXT_SMALL).color(TEXT_DIM));
        });
        ui.separator();
    }

    /// The show's clock format, everywhere at once (numbers audit #8):
    /// the top bar, the clock graphics on air, the second screen.
    pub(super) fn set_clock_12h(&mut self, twelve: bool) {
        self.cfg.ui.clock_12h = twelve;
        self.cfg.sync_clock_format();
        if let Some(e) = &self.engine {
            e.graphics.update(&self.cfg.graphics);
        }
        self.cfg_dirty = true;
    }

    /// The second screen as it is now, with what its window's own corner
    /// buttons do (open, full screen, close), above its settings.
    fn second_screen_now(&mut self, ui: &mut egui::Ui) {
        use crate::config::DisplayOutput;
        let shown = match self.cfg.display.output {
            DisplayOutput::Off => None,
            DisplayOutput::Program => Some("the program"),
            DisplayOutput::Multiview => Some("the multiview"),
            DisplayOutput::Prompter => Some("the teleprompter"),
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.5, if shown.is_some() { GREEN } else { LINE });
            match shown {
                Some(what) => {
                    let how = if self.display_fullscreen { "full screen" } else { "in a window" };
                    ui.label(RichText::new(format!("Second screen: showing {what}, {how}")).size(TEXT_BODY));
                    let full = if self.display_fullscreen { "Leave full screen" } else { "Full screen" };
                    if ui.button(full).on_hover_text("F, or double-click the picture, does this on the screen itself").clicked() {
                        self.display_toggle_full = true;
                    }
                    if ui.button("Close").on_hover_text("Esc on the screen itself (twice from full screen)").clicked() {
                        self.cfg.display.output = DisplayOutput::Off;
                        self.cfg_dirty = true;
                    }
                }
                None => {
                    ui.label(RichText::new("Second screen: off").size(TEXT_BODY).color(TEXT_DIM));
                    if ui.button("Show the program there").on_hover_text("Or pick what it shows below").clicked() {
                        self.cfg.display.output = DisplayOutput::Program;
                        self.cfg_dirty = true;
                    }
                }
            }
        });
        ui.separator();
    }

    /// The Settings page in the area page, and what it asked for.
    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let info = self.settings_info(ui.ctx());
        let now = self.settings.as_ref().filter(|w| crate::settings::is_live(w.section())).map(|_| self.live_settings());
        let Some(win) = self.settings.as_mut() else { return };
        // A page that saves as you go shows the settings in effect, with
        // what's being edited on top.
        if let Some(now) = &now {
            win.rebase(now);
        }
        let action = win.show_page(ui, &info);
        self.settings_page = Some(win.section());
        if win.section() == Section::Sources {
            if let Some(name) = win.source_name() {
                self.shell.picked.insert("sources".into(), name);
            }
        }
        if win.section() == Section::Graphics {
            if let Some(name) = win.graphic_name() {
                self.shell.picked.insert("graphics".into(), name);
            }
        }
        if win.section() == Section::Audio {
            if let Some(name) = win.input_name() {
                self.shell.picked.insert("inputs".into(), name);
            }
        }
        let closing = matches!(action, SettingsAction::Close | SettingsAction::Apply { close: true, .. });
        self.settings_action(action, ui.ctx());
        if closing && self.settings.is_none() {
            // OK, Cancel or Discard: back to Show.
            self.shell.area = 0;
            self.shell.shown_section = None;
        }
        // Live levels on the pages that show them.
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }

    /// Record › Takes: the takes so far, newest first.
    fn takes_page(&mut self, ui: &mut egui::Ui) {
        // Looked for again every few seconds, so a take just saved shows.
        if self.shell.takes_looked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.look_for_takes_again();
            self.shell.takes_looked = Some(Instant::now());
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(900.0);
            self.takes_list(ui);
        });
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    /// Record › Recording › Before you start.
    fn check_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("Before you start").size(TEXT_HEADING).color(Color32::WHITE));
        ui.label(RichText::new("The pre-show check looks at every source, the sound, the drive, the encoder and the stream servers, and says what to fix.").color(TEXT_DIM));
        ui.add_space(6.0);
        if ui.button("Run pre-show check").clicked() {
            self.start_precheck(None);
        }
        ui.add_space(4.0);
        ui.label(RichText::new("It's also one click away under the status light at the top right. To run it by itself before every Record and Go live, turn on \"Check before recording or going live\" in More › App › General.").size(TEXT_SMALL).color(TEXT_DIM));
    }

    /// More › App › Updates & about.
    fn about_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        self.about_body(ui);
        ui.add_space(12.0);
        ui.separator();
        let update = self.update.lock_ok().clone();
        match update {
            Some(r) => {
                let live = self.engine.as_ref().is_some_and(|e| e.is_recording() || e.is_streaming()) || self.busy().is_some();
                let (mut skip, mut restart) = (false, false);
                ui.set_max_width(560.0);
                self.update_window(ui, &r, live, &mut skip, &mut restart);
                if skip {
                    self.cfg.ui.skip_version = r.version.clone();
                    self.cfg_dirty = true;
                }
                if r.version == self.cfg.ui.skip_version {
                    ui.label(RichText::new("You chose to skip this version.").size(TEXT_SMALL).color(TEXT_DIM));
                }
                if restart {
                    crate::net::self_update::choose(crate::net::self_update::Finish::Restart);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            None => {
                ui.label(RichText::new("You have the newest version Cutaway knows about.").color(TEXT_DIM));
            }
        }
    }

    /// More › App › Help: the first-show guide, and every key as it's set now.
    fn help_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Open the first-show guide").clicked() {
                self.guide_open = true;
            }
            ui.label(RichText::new("A few steps from your cameras to a first recording.").color(TEXT_DIM));
        });
        ui.add_space(10.0);
        ui.label(RichText::new("KEYS").size(TEXT_SMALL).color(TEXT_DIM));
        let map = match &self.keymap {
            Some((_, m)) => m.clone(),
            None => crate::control::shortcuts::Keymap::new(&self.cfg.keys),
        };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("help_keys").num_columns(2).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, key: String, what: &str| {
                    ui.label(RichText::new(if key.is_empty() { "—".into() } else { key }).monospace().color(Color32::WHITE));
                    ui.label(what);
                    ui.end_row();
                };
                for b in Builtin::ALL {
                    row(ui, map.label_of(b), b.describe());
                }
                let shift = |k| KeyCombo { shift: true, ..KeyCombo::plain(k) }.label();
                let alt = |k| KeyCombo { alt: true, ..KeyCombo::plain(k) }.label();
                row(ui, format!("{}–{}", KeyCombo::plain(DIGITS[0]).label(), KeyCombo::plain(DIGITS[8]).label()), "Take scene 1–9 (or source 1–9, as set in Scenes › Switching)");
                row(ui, format!("{}–{}", shift(DIGITS[0]), shift(DIGITS[8])), "Cut straight to it, even with Preview on");
                row(ui, format!("{}–{}", alt(DIGITS[0]), alt(DIGITS[8])), "Show or hide graphic 1–9");
                row(ui, "F1–F12".into(), "Play sound pad 1–12 (held pads play while the key is down)");
                if let (Some(first), Some(last)) = (area_key(1), area_key(7)) {
                    row(ui, format!("{}–{}", first.label(), last.label()), "Show, Sources, Scenes, Sound, Graphics, Record, More");
                }
                row(ui, "Esc".into(), "Back to Show (when you're not typing)");
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Change these, or add your own, in More › Control › Shortcuts.").size(TEXT_SMALL).color(TEXT_DIM));
        });
    }
}

/// Every section, as indexes into nav::AREAS.
fn sections() -> impl Iterator<Item = (usize, usize, usize)> {
    AREAS.iter().enumerate().flat_map(|(a, area)| area.tabs.iter().enumerate().flat_map(move |(t, tab)| (0..tab.sections.len()).map(move |s| (a, t, s))))
}

/// An area's icon on the rail, drawn (not a font glyph, so it looks the
/// same on every computer).
fn draw_icon(p: &egui::Painter, icon: Icon, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    match icon {
        Icon::Monitor => {
            let r = Rect::from_center_size(c - vec2(0.0, 2.0), vec2(20.0, 13.0));
            p.rect_stroke(r, 2.0, s, StrokeKind::Middle);
            p.line_segment([pos2(c.x - 5.0, r.bottom() + 4.0), pos2(c.x + 5.0, r.bottom() + 4.0)], s);
        }
        Icon::Camera => {
            let r = Rect::from_center_size(c - vec2(3.0, 0.0), vec2(14.0, 11.0));
            p.rect_stroke(r, 2.0, s, StrokeKind::Middle);
            p.add(egui::Shape::convex_polygon(vec![pos2(r.right() + 1.0, c.y), pos2(r.right() + 7.0, c.y - 5.0), pos2(r.right() + 7.0, c.y + 5.0)], Color32::TRANSPARENT, s));
        }
        Icon::Layers => {
            for d in [-4.0, 0.0, 4.0] {
                let pts = vec![pos2(c.x - 9.0, c.y + d), pos2(c.x, c.y + d - 4.0), pos2(c.x + 9.0, c.y + d), pos2(c.x, c.y + d + 4.0)];
                p.add(egui::Shape::closed_line(pts, s));
            }
        }
        Icon::Wave => {
            for (i, h) in [4.0, 10.0, 16.0, 8.0, 12.0, 5.0].iter().enumerate() {
                let x = c.x - 9.0 + i as f32 * 3.6;
                p.line_segment([pos2(x, c.y - h / 2.0), pos2(x, c.y + h / 2.0)], s);
            }
        }
        Icon::Text => {
            p.text(c, Align2::CENTER_CENTER, "T", FontId::proportional(18.0), color);
        }
        Icon::RecordDot => {
            p.circle_stroke(c, 8.0, s);
            p.circle_filled(c, 4.0, if color == Color32::WHITE { REC_DOT } else { color });
        }
        Icon::Dots => {
            for dx in [-6.0, 0.0, 6.0] {
                p.circle_filled(c + vec2(dx, 0.0), 1.8, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every section of every area but Show opens something, and each old
    /// Settings page's new home is a section that holds that page.
    #[test]
    fn every_section_opens_something() {
        for a in AREAS.iter().filter(|a| a.id != "show") {
            for t in a.tabs {
                for s in t.sections {
                    assert!(target(a.id, t.id, s.id).is_some(), "{}/{}/{} opens nothing", a.id, t.id, s.id);
                }
            }
        }
        for page in Section::ALL {
            let home = home_of(page);
            let (a, t, s) = indexes(home).unwrap_or_else(|| panic!("{page:?}: {home} isn't a place"));
            assert!(matches!(target_at(a, t, s), Some(Target::Page(p, _)) if p == page), "{page:?}: {home} shows something else");
        }
    }

    /// Every place the code goes to by name ("area/tab/section") is there.
    #[test]
    fn code_goes_only_to_real_places() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        for f in ["ui.rs", "ui/shell.rs", "ui/guide.rs", "ui/panels.rs", "ui/takes.rs", "ui/waiting.rs"] {
            let text = std::fs::read_to_string(src.join(f)).unwrap();
            // (Split so this test's own text doesn't match.)
            for piece in text.split(concat!(".go", "(\"")).skip(1) {
                let path = piece.split('"').next().unwrap_or_default();
                assert!(indexes(path).is_some(), "{f}: go(\"{path}\") isn't a place");
                seen += 1;
            }
        }
        assert!(seen >= 3, "the go(…) calls weren't found");
    }

    #[test]
    fn the_light_says_all_good_or_how_many() {
        assert_eq!(light_text(0), "All good");
        assert_eq!(light_text(2), "2 to check");
    }

    /// ⌘1–⌘7 go to the seven areas, in the rail's order.
    #[test]
    fn area_keys() {
        for (i, a) in AREAS.iter().enumerate() {
            assert_eq!(a.key, Some(i as u8 + 1), "{}", a.id);
            let k = area_key(i as u8 + 1).unwrap();
            assert!(k.command && !k.shift && !k.alt && k.key == DIGITS[i]);
            // Not one the numbered keys (scenes, graphics, pads) use.
            assert!(!k.is_numbered());
        }
    }

    /// The rail and tabs remembered: an area's tab, kept by id, and an
    /// unknown one (from an older or newer Cutaway) falls back to the first.
    #[test]
    fn remembered_tabs() {
        let mut last = HashMap::new();
        last.insert("record".to_string(), ("streaming".to_string(), "where".to_string()));
        last.insert("sound".to_string(), ("gone".to_string(), "x".to_string()));
        let rec = AREAS.iter().position(|a| a.id == "record").unwrap();
        let sound = AREAS.iter().position(|a| a.id == "sound").unwrap();
        let (_, t, _) = place_in(rec, &last);
        assert_eq!(AREAS[rec].tabs[t].id, "streaming");
        assert_eq!(place_in(sound, &last), (sound, 0, 0));
    }
}
