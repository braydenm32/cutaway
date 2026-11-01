//! ⌘K (Ctrl+K on Windows), or the magnifier in the top bar: one search box
//! over whatever area is open. It finds every setting the old Settings
//! "Find a setting…" box found (by name, page or what its help says), the
//! areas, tabs and sections of nav.rs (with the old page and group names as
//! other names for them), and the desk's own buttons and controls by their
//! names in routes.rs. Choosing a result goes there: the area and tab, the
//! folded part opened, and a setting's name lit up for a moment. A desk
//! action is only shown, never pressed: nothing goes on air from here.

use super::nav::{self, AREAS};
use super::routes::ROUTES;
use super::shell::{home_of, indexes, target_at, Target};
use super::theme::*;
use super::App;
use crate::control::shortcuts::{Binding, Builtin, KeyCombo};
use crate::settings::{self, Section};
use eframe::egui::{self, vec2, Align2, Color32, FontId, Key, Modifiers, RichText};

/// How many results the box shows.
const SHOWN: usize = 12;

/// The box, kept for the session: the last query is still there when it
/// opens again.
#[derive(Default)]
pub(super) struct Search {
    open: bool,
    query: String,
    /// The result picked with the arrow keys.
    sel: usize,
    /// The query `sel` was picked for (a new query starts at the top).
    sel_for: String,
}

/// Where a result goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Go {
    /// An area's section (indexes into nav::AREAS), with one of its page's
    /// folded parts opened and a setting's name lit.
    Place { at: (usize, usize, usize), part: Option<&'static str>, light: Option<(Section, &'static str)> },
    /// An area, where it was left.
    Area(usize),
    /// A part of the window that's always there (a nav::SHELL part): only
    /// says where.
    Shell(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Hit {
    /// What it's called.
    pub title: String,
    /// Where it is, as "Area › Tab › Section".
    pub place: String,
    pub go: Go,
    /// A line to show once there (where on the desk it is).
    pub note: Option<String>,
    /// A setting from the old Find: its page and name.
    pub setting: Option<(Section, &'static str)>,
}

// ---- where things are ---------------------------------------------------------------

/// Old Settings pages split across tabs: the section for each setting that
/// isn't on its page's home (shell::home_of). Folded parts are placed by
/// the part they're in (PART_HOME, help::part_of).
const SETTING_HOME: &[(Section, &str, &str)] = &[
    // Sources › Close-ups & PTZ
    (Section::Sources, "Cut from", "sources/closeups/split"),
    (Section::Sources, "Zoom limit", "sources/closeups/split"),
    (Section::Sources, "Moves to presets take", "sources/closeups/ptz"),
    (Section::Sources, "Controlled by", "sources/closeups/ptz"),
    (Section::Sources, "Camera's address", "sources/closeups/ptz"),
    (Section::Sources, "Speed", "sources/closeups/ptz"),
    (Section::Sources, "Mounted upside down", "sources/closeups/ptz"),
    (Section::Sources, "Follow", "sources/closeups/framing"),
    (Section::Sources, "Shot", "sources/closeups/framing"),
    (Section::Sources, "Moves take", "sources/closeups/framing"),
    // Sources › People & names
    (Section::Sources, "Name title", "sources/people/who"),
    // Sources › Network & phone feeds
    (Section::Sources, "Feed", "sources/network/feeds"),
    (Section::Sources, "Listen on port", "sources/network/feeds"),
    (Section::Sources, "Address", "sources/network/feeds"),
    (Section::Sources, "Stream key", "sources/network/feeds"),
    (Section::Sources, "NDI source", "sources/network/feeds"),
    (Section::Sources, "Buffer", "sources/network/feeds"),
    // Sources › Media & playlists
    (Section::Sources, "Loop", "sources/media/media"),
    (Section::Sources, "At start", "sources/media/media"),
    (Section::Sources, "When taken", "sources/media/media"),
    (Section::Sources, "Show images for", "sources/media/media"),
    // Graphics › Clocks & timers
    (Section::Graphics, "Shows", "graphics/timers/timers"),
    (Section::Graphics, "Format", "graphics/timers/timers"),
    (Section::Graphics, "Counting down to", "graphics/timers/timers"),
    (Section::Graphics, "From", "graphics/timers/timers"),
    // Graphics › Tickers & web overlays
    (Section::Graphics, "Speed", "graphics/web/web"),
    (Section::Graphics, "Address", "graphics/web/web"),
    (Section::Graphics, "Page size", "graphics/web/web"),
    (Section::Graphics, "Drawn by", "graphics/web/web"),
];

/// Folded parts that no section opens by itself: the section they're on.
const PART_HOME: &[(&str, &str)] = &[
    ("ftb", "scenes/stinger/stinger"),
    ("midi", "more/control/oscmidi"),
    ("mix_talk", "sound/phones/phones"),
    ("mix_outs", "sound/phones/phones"),
];

/// "area/tab/section" or "area/tab" (its first section) as indexes.
fn at(path: &str) -> Option<(usize, usize, usize)> {
    indexes(path).or_else(|| indexes(&format!("{path}/{}", nav_first_section(path)?)))
}

fn nav_first_section(path: &str) -> Option<&'static str> {
    let (a, t) = path.split_once('/')?;
    Some(nav::area(a)?.tabs.iter().find(|x| x.id == t)?.sections.first()?.id)
}

/// Where a setting from the help table lives now, and the folded part it's in.
pub(super) fn setting_at(page: Section, label: &str) -> ((usize, usize, usize), Option<&'static str>) {
    let home = || at(home_of(page)).expect("every page has a home");
    if let Some(part) = settings::folded_part(page, label) {
        let by_section = sections().find(|&(a, t, s)| target_at(a, t, s) == Some(Target::Page(page, Some(part))));
        let by_table = PART_HOME.iter().find(|(p, _)| *p == part).and_then(|(_, path)| at(path));
        return (by_section.or(by_table).unwrap_or_else(home), Some(part));
    }
    let place = SETTING_HOME.iter().find(|(p, l, _)| *p == page && *l == label).and_then(|(_, _, path)| at(path));
    (place.unwrap_or_else(home), None)
}

/// Every section, as indexes into nav::AREAS.
fn sections() -> impl Iterator<Item = (usize, usize, usize)> {
    AREAS.iter().enumerate().flat_map(|(a, area)| area.tabs.iter().enumerate().flat_map(move |(t, tab)| (0..tab.sections.len()).map(move |s| (a, t, s))))
}

/// "Sources › Close-ups & PTZ › PTZ presets" (the section only when the
/// tab has more than one).
pub(super) fn breadcrumb((a, t, s): (usize, usize, usize)) -> String {
    let (area, tab) = (&AREAS[a], &AREAS[a].tabs[t]);
    if tab.sections.len() > 1 {
        format!("{} › {} › {}", area.label, tab.label, tab.sections[s].label)
    } else {
        format!("{} › {}", area.label, tab.label)
    }
}

/// Where on Show a section of its tabs is, in words.
fn on_show(sect: &str) -> &'static str {
    match sect {
        "program" => "on the program monitor, on Show",
        "multiview" => "on the multiview, on Show (right-click or ⋯ on a tile)",
        "bank" => "in the scene bank at the bottom of Show",
        "column" => "in the panels on the right of Show",
        _ => "in Edit layout, on Show",
    }
}

/// Where a part of the window that's always there is, in words.
fn on_shell(part: &str) -> &'static str {
    match part {
        "rail" => "on the rail down the left",
        "status" => "at the top right",
        "page" => "at the bottom of each settings page",
        _ => "on the main window",
    }
}

/// Routes that are questions or parts of the old window, not places to go.
fn searchable(id: &str) -> bool {
    !(id.starts_with("dlg.confirm.") || id.starts_with("set.modal.") || id.starts_with("set.frame."))
}

/// Where a route (routes.rs) is now: the section it's in, its visible twin
/// (nav::VISIBLE), or the part of the window it's in; with a line about
/// where to look once there.
fn route_go(id: &'static str, label: &str) -> Option<(Go, String, Option<String>)> {
    let place = |(a, t, s): (usize, usize, usize)| {
        let note = (a == 0).then(|| format!("{label}: {}.", on_show(AREAS[a].tabs[t].sections[s].id)));
        (Go::Place { at: (a, t, s), part: None, light: None }, breadcrumb((a, t, s)), note)
    };
    if let Some(p) = sections().find(|&(a, t, s)| AREAS[a].tabs[t].sections[s].routes.contains(&id)) {
        return Some(place(p));
    }
    let is_shell = |to: &str| nav::SHELL.iter().any(|(p, _)| *p == to);
    let twin = nav::VISIBLE.iter().find(|(r, _)| *r == id).map(|(_, to)| *to);
    if let Some(to) = twin.filter(|to| !is_shell(to) && !to.starts_with("phone/")) {
        if let Some(p) = at(to) {
            return Some(place(p));
        }
        if let Some(i) = AREAS.iter().position(|x| x.id == to) {
            return Some((Go::Area(i), AREAS[i].label.to_string(), None));
        }
    }
    // A part of the window that's always on screen.
    let shell = twin.filter(|to| is_shell(to)).or_else(|| nav::SHELL.iter().find(|(_, r)| r.contains(&id)).map(|(p, _)| *p));
    if let Some(part) = shell {
        let place = match part {
            "rail" => "The rail",
            "status" => "The top bar",
            _ => "Every page",
        };
        return Some((Go::Shell(part), place.to_string(), Some(format!("{label}: {}.", on_shell(part)))));
    }
    // The phone remote (and the pages it opens): set up from More › Control.
    if twin.is_some_and(|to| to.starts_with("phone/")) || nav::PHONE.iter().any(|(_, r)| r.contains(&id)) {
        let p = at("more/control/remote")?;
        let note = format!("{label}: on the phone remote. Turn it on and open it from here.");
        return Some((Go::Place { at: p, part: Some("remote"), light: None }, format!("Phone remote · {}", breadcrumb(p)), Some(note)));
    }
    None
}

/// What a route is called in the results ("ftb" is a command's name).
fn route_title(id: &str, label: &str) -> String {
    if id.starts_with("cmd.") {
        format!("{label} (command for keys, Stream Deck, MIDI, OSC)")
    } else {
        label.to_string()
    }
}

// ---- searching ------------------------------------------------------------------------

/// Everything matching what was typed (every word of it, in any order),
/// best first: setting names; areas, tabs, sections and the old page and
/// group names; the desk's buttons and controls; then what a setting's
/// help says. At most `max`.
pub(super) fn search(query: &str, max: usize) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let has = |hay: &str| {
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    let mut out: Vec<Hit> = Vec::new();
    let push = |h: Hit, out: &mut Vec<Hit>| {
        // The same name going to the same place once (a page found by its
        // old name and by its tab, say).
        if !out.iter().any(|o| o.go == h.go && o.title.to_lowercase() == h.title.to_lowercase()) && !out.contains(&h) {
            out.push(h);
        }
    };
    let setting = |page: Section, label: &'static str| {
        let (p, part) = setting_at(page, label);
        Hit { title: label.to_string(), place: breadcrumb(p), go: Go::Place { at: p, part, light: Some((page, label)) }, note: None, setting: Some((page, label)) }
    };
    let entries: Vec<(Section, &'static str, &'static str)> = settings::setting_entries().collect();
    // 1. Setting names.
    for &(page, label, _) in &entries {
        if has(label) {
            push(setting(page, label), &mut out);
        }
    }
    // 2. Places: areas, tabs, sections; the old pages and groups by name.
    for (a, area) in AREAS.iter().enumerate() {
        if has(area.label) {
            push(Hit { title: area.label.to_string(), place: "Area".into(), go: Go::Area(a), note: None, setting: None }, &mut out);
        }
        for (t, tab) in area.tabs.iter().enumerate() {
            if has(tab.label) || has(&format!("{} {}", area.label, tab.label)) {
                let p = (a, t, 0);
                let note = (a == 0).then(|| format!("{}: {}.", tab.label, on_show(tab.sections[0].id)));
                push(Hit { title: tab.label.to_string(), place: format!("{} › {}", area.label, tab.label), go: Go::Place { at: p, part: None, light: None }, note, setting: None }, &mut out);
            }
            if tab.sections.len() > 1 {
                for (s, sect) in tab.sections.iter().enumerate() {
                    if has(sect.label) {
                        let note = (a == 0).then(|| format!("{}: {}.", sect.label, on_show(sect.id)));
                        push(Hit { title: sect.label.to_string(), place: breadcrumb((a, t, s)), go: Go::Place { at: (a, t, s), part: None, light: None }, note, setting: None }, &mut out);
                    }
                }
            }
        }
    }
    for page in Section::ALL {
        if has(page.label()) {
            push(page_hit(page, page.label()), &mut out);
        }
    }
    for (label, summary, page) in settings::old_groups() {
        if has(label) || has(summary) {
            let mut h = page_hit(page, label);
            h.title = format!("{label} ({summary})");
            push(h, &mut out);
        }
    }
    // 3. The desk's buttons and controls, and the other ways in.
    for r in ROUTES.iter().filter(|r| searchable(r.id)) {
        if has(r.label) {
            if let Some((go, place, note)) = route_go(r.id, r.label) {
                push(Hit { title: route_title(r.id, r.label), place, go, note, setting: None }, &mut out);
            }
        }
    }
    // 4. What a setting's help says.
    for &(page, label, text) in &entries {
        if has(&format!("{label} {text}")) {
            push(setting(page, label), &mut out);
        }
    }
    out.truncate(max);
    out
}

/// An old Settings page, where it lives now.
fn page_hit(page: Section, title: &str) -> Hit {
    let p = at(home_of(page)).expect("every page has a home");
    Hit { title: title.to_string(), place: breadcrumb(p), go: Go::Place { at: p, part: None, light: None }, note: None, setting: None }
}

// ---- the box ----------------------------------------------------------------------------

impl App {
    /// ⌘K, even while typing in a box (only ⌘K: everything else typed
    /// stays in the box). Taken out of the frame's keys, so a text box
    /// doesn't also act on it (Ctrl+K deletes to the end of the line).
    pub(super) fn search_key(&mut self, ctx: &egui::Context) {
        // Not over a question, or while a key is being set on the Shortcuts page.
        if self.settings.as_ref().is_some_and(|s| s.busy()) || self.confirm.is_some() || self.title_edit.is_some() || self.marker_name.is_some() || self.precheck.is_some() || self.sync.is_some() {
            return;
        }
        let Some((_, map)) = &self.keymap else { return };
        let mut hit = false;
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed, modifiers, .. } if !(modifiers.ctrl && !modifiers.command) => {
                    let ours = map.find(KeyCombo::new(*key, *modifiers)) == Some(&Binding::Builtin(Builtin::Search));
                    if ours && *pressed {
                        hit = true;
                    }
                    !ours
                }
                _ => true,
            })
        });
        if hit {
            self.toggle_search();
        }
    }

    pub(super) fn toggle_search(&mut self) {
        self.search.open = !self.search.open;
    }

    pub(super) fn search_open(&self) -> bool {
        self.search.open
    }

    /// For the UI self-test: the box open with this typed, or (`go`) gone
    /// to its first result.
    pub(super) fn search_for(&mut self, query: &str, go: bool) {
        self.search.query = query.to_string();
        self.search.open = !go;
        if go {
            if let Some(h) = search(query, 1).into_iter().next() {
                self.search_go(h);
            }
        }
    }

    /// The magnifier in the top bar.
    pub(super) fn search_button(&mut self, ui: &mut egui::Ui) {
        let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
        let color = if r.hovered() || self.search.open { Color32::WHITE } else { TEXT_DIM };
        if r.hovered() || self.search.open {
            ui.painter().rect_filled(rect, 4.0, RAISED);
        }
        // Drawn, not a font glyph, so it's the same on every computer.
        let c = rect.center() - vec2(2.0, 2.0);
        let s = egui::Stroke::new(1.8, color);
        ui.painter().circle_stroke(c, 6.0, s);
        ui.painter().line_segment([c + vec2(4.4, 4.4), c + vec2(9.0, 9.0)], s);
        let r = r.on_hover_text(format!("Search settings, pages and the desk{}", self.key_hint(Builtin::Search)));
        if r.clicked() {
            self.toggle_search();
        }
    }

    /// The box, under the top bar, over whatever's open.
    pub(super) fn search_window(&mut self, ctx: &egui::Context) {
        if !self.search.open {
            return;
        }
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).min(600.0);
        let list_h = (screen.height() - super::shell::TOP_BAR - 150.0).clamp(120.0, 420.0);
        let area = egui::Modal::default_area(egui::Id::unique("search")).anchor(Align2::CENTER_TOP, vec2(0.0, super::shell::TOP_BAR + 6.0));
        let frame = egui::Frame::popup(&ctx.global_style()).fill(PANEL).inner_margin(egui::Margin::same(12));
        let mut chosen: Option<Hit> = None;
        let r = egui::Modal::new(egui::Id::unique("search")).area(area).frame(frame).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
            ui.set_width(width);
            // The arrows choose a result, before the box can use them.
            let (down, up, enter) = ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::ArrowUp), i.key_pressed(Key::Enter)));
            let field = egui::TextEdit::singleline(&mut self.search.query)
                .hint_text("Search settings, pages and the desk…")
                .font(FontId::proportional(TEXT_HEADING))
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 8.0));
            ui.add(field).request_focus();
            if self.search.sel_for != self.search.query {
                self.search.sel = 0;
                self.search.sel_for = self.search.query.clone();
            }
            let hits = search(&self.search.query, SHOWN);
            ui.add_space(6.0);
            if self.search.query.trim().is_empty() {
                ui.label(RichText::new("Any setting by its name or what it does, any page, and the desk's buttons: try \"fade to black\", \"stream key\", \"headphones\" or \"12-hour\".").color(TEXT_DIM));
            } else if hits.is_empty() {
                ui.label(RichText::new(format!("Nothing found for \"{}\".", self.search.query.trim())).color(TEXT_DIM));
            } else {
                if down {
                    self.search.sel = (self.search.sel + 1).min(hits.len() - 1);
                }
                if up {
                    self.search.sel = self.search.sel.saturating_sub(1);
                }
                self.search.sel = self.search.sel.min(hits.len() - 1);
                egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, h) in hits.iter().enumerate() {
                        let mut job = egui::text::LayoutJob::default();
                        job.append(&h.title, 0.0, egui::TextFormat::simple(FontId::proportional(TEXT_STATE), Color32::WHITE));
                        job.append(&format!("   {}", h.place), 0.0, egui::TextFormat::simple(FontId::proportional(TEXT_BODY), TEXT_DIM));
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        job.wrap.max_width = width - 20.0;
                        let b = egui::Button::selectable(i == self.search.sel, job).min_size(vec2(width, 30.0));
                        let r = ui.add(b);
                        if (down || up) && i == self.search.sel {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            chosen = Some(h.clone());
                        }
                    }
                });
                if enter && chosen.is_none() {
                    chosen = hits.get(self.search.sel).cloned();
                }
            }
            ui.add_space(6.0);
            let key = self.key_label_or(Builtin::Search, "");
            let close = if key.is_empty() { "Esc to close".to_string() } else { format!("Esc or {key} to close") };
            ui.label(RichText::new(format!("↑ ↓ to choose · Enter to go there · {close}. Going to a button never presses it.")).size(TEXT_SMALL).color(TEXT_DIM));
        });
        if let Some(h) = chosen {
            self.search.open = false;
            self.search_go(h);
        } else if r.should_close() {
            self.search.open = false;
        }
    }

    /// Go where a result is: its area, tab and section, its folded part
    /// opened and its setting lit. Never presses anything.
    pub(super) fn search_go(&mut self, h: Hit) {
        match h.go {
            Go::Place { at, part, light } => {
                self.go_to(at.0, at.1, at.2);
                if let (Some(p), Some(win)) = (part, &self.settings) {
                    win.unfold(p);
                }
                if let Some((page, label)) = light {
                    settings::light_setting(page, label);
                }
            }
            Go::Area(a) => self.go_area(a),
            Go::Shell(_) => {}
        }
        if let Some(n) = h.note {
            self.toast(n, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The old "Find a setting" found these; ⌘K finds each of them too,
    /// going to the section that holds its page.
    #[test]
    fn search_finds_everything_find_found() {
        let entries: Vec<_> = settings::setting_entries().collect();
        assert!(entries.len() > 280, "{}", entries.len());
        for &(page, label, _) in &entries {
            let hit = search(label, 50).into_iter().find(|h| h.setting == Some((page, label)));
            let hit = hit.unwrap_or_else(|| panic!("{label} on {page:?} not found"));
            let Go::Place { at: (a, t, s), light, .. } = hit.go else { panic!("{label}: goes nowhere") };
            assert!(AREAS[a].tabs[t].sections[s].help.contains(&page), "{label} ({page:?}) lands on the wrong tab: {}", hit.place);
            // (People & names is the app's own page for the sources' names.)
            assert!(matches!(target_at(a, t, s), Some(Target::Page(p, _)) if p == page) || (page == Section::Sources && target_at(a, t, s) == Some(Target::People)), "{label} ({page:?}): {} doesn't show its page", hit.place);
            assert_eq!(light, Some((page, label)), "{label}: not lit");
        }
        for p in Section::ALL {
            let hits = search(p.label(), 8);
            let home = at(home_of(p)).unwrap();
            assert!(hits.iter().any(|h| matches!(h.go, Go::Place { at, .. } if at == home)), "old page name {} doesn't go to its page", p.label());
        }
        for (label, summary, _) in settings::old_groups() {
            assert!(!search(label, 8).is_empty() && !search(summary, 8).is_empty(), "old group {label} finds nothing");
        }
    }

    /// Every query the old box answered, answered the same way or better:
    /// each name, each page name and every word in the names and help, and
    /// every result the old box gave for it is among the new ones.
    #[test]
    fn every_old_find_still_resolves() {
        let mut queries: HashSet<String> = HashSet::new();
        for (page, label, text) in settings::setting_entries() {
            queries.insert(label.to_string());
            queries.insert(page.label().to_string());
            for w in format!("{label} {text}").split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
                queries.insert(w.to_lowercase());
            }
        }
        assert!(queries.len() > 1000, "{}", queries.len());
        let mut lost = Vec::new();
        for q in &queries {
            let new = search(q, usize::MAX);
            for old in settings::old_find(q, usize::MAX) {
                let found = match &old {
                    (page, Some(label)) => new.iter().any(|h| h.setting.is_some_and(|(p, l)| p == *page && l == label.as_str())),
                    (page, None) => {
                        let home = at(home_of(*page)).unwrap();
                        new.iter().any(|h| matches!(h.go, Go::Place { at, .. } if at == home))
                    }
                };
                if !found {
                    lost.push(format!("{q:?} → {old:?}"));
                }
            }
        }
        assert!(lost.is_empty(), "the old Find found these and ⌘K doesn't: {lost:?}");
    }

    /// Desk actions are found by name, and go to where they're drawn
    /// without doing anything.
    #[test]
    fn desk_actions_are_found() {
        let first = |q: &str| search(q, 20).into_iter().find(|h| h.setting.is_none() && h.title.to_lowercase().contains(&q.to_lowercase())).unwrap_or_else(|| panic!("{q}: nothing"));
        let ftb = first("Fade to black");
        assert!(search("fade to black", 20).iter().any(|h| h.place == "Show › Switch › Scene bank" && h.note.as_deref().is_some_and(|n| n.contains("scene bank"))));
        assert!(matches!(ftb.go, Go::Place { .. }));
        // The top bar's own: says where, goes nowhere.
        assert!(search("record", 40).iter().any(|h| h.title == "Record" && h.go == Go::Shell("status")));
        // The phone remote's: its page in More › Control.
        assert!(search("phone fade", 10).iter().any(|h| h.place.starts_with("Phone remote")));
        // A way in with no screen of its own goes to its visible twin.
        assert!(search("--list-devices", 5).iter().any(|h| h.place == "More › App › Diagnostics"));
        // An area by name.
        let sound = AREAS.iter().position(|a| a.id == "sound").unwrap();
        assert!(search("sound", 8).iter().any(|h| h.go == Go::Area(sound)));
        // Nothing in the questions or the old window's frame.
        assert!(search("stop recording?", 10).iter().all(|h| !h.title.ends_with('?')));
        assert!(search("zzzqqq", 8).is_empty());
        assert!(search("  ", 8).is_empty());
        assert!(search("e", 8).len() <= 8);
    }

    /// Every route a person could look for is found by its own name.
    #[test]
    fn every_route_is_found_by_name() {
        let mut missing = Vec::new();
        for r in ROUTES.iter().filter(|r| searchable(r.id)) {
            if route_go(r.id, r.label).is_none() && !nav::NO_SCREEN.contains(&r.id) {
                missing.push(r.id);
            }
        }
        assert!(missing.is_empty(), "routes search can't take you to: {missing:?}");
    }

    /// The tables name real places, and settings that are really there.
    #[test]
    fn tables_are_real() {
        let entries: HashSet<(Section, &str)> = settings::setting_entries().map(|(p, l, _)| (p, l)).collect();
        for (page, label, path) in SETTING_HOME {
            assert!(entries.contains(&(*page, *label)), "{label} isn't a setting on {page:?}");
            let (a, t, s) = at(path).unwrap_or_else(|| panic!("{path} isn't a place"));
            assert!(matches!(target_at(a, t, s), Some(Target::Page(p, _)) if p == *page) || (*page == Section::Sources && target_at(a, t, s) == Some(Target::People)), "{path} doesn't show {page:?}");
        }
        for (part, path) in PART_HOME {
            assert!(at(path).is_some(), "{part}: {path} isn't a place");
        }
        // A sample, by hand.
        assert_eq!(breadcrumb(setting_at(Section::Sources, "Follow").0), "Sources › Close-ups & PTZ › Auto-framing");
        assert_eq!(setting_at(Section::Mixing, "Ducking").1, Some("mix_duck"));
        assert_eq!(breadcrumb(setting_at(Section::Recording, "Timecode").0), "Record › Recording › Editing");
    }
}
