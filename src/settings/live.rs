//! "Saves as you go": how an area page applies its changes as they're
//! made, with Undo, instead of the old OK / Apply / Cancel / Revert bar.
//!
//! The mechanism, for the pages listed in `LIVE` (the calm redesign moves
//! more pages here area by area; to move one, add its `Section` to `LIVE`
//! and make sure `copy_section` names every field the page edits):
//!
//! * The page still edits `SettingsUi::draft`, so its drawing code doesn't
//!   change. After each frame `live_step` sends what changed as
//!   `SettingsAction::Live`, and the app applies it the way Apply always
//!   has (`Config::with_edits`, a three-way merge, so a fader moved or a
//!   preset saved on the desk meanwhile is kept), records an Undo step (⌘Z,
//!   or the page's ↶ Undo; edits close together are one step) and saves.
//! * Text commits when you leave the box or press Enter, not at every
//!   letter; a slider being dragged applies at most every 250 ms; nothing
//!   is sent while the page has a problem to fix, and a change that failed
//!   isn't sent again until it changes. One change is applied at a time,
//!   on the UI thread, so saves never race.
//! * Changes that would reopen a source (its device, size or rate, a new
//!   name), add, remove or reorder sources or inputs, or rebuild the
//!   engine are held back: the page's bar lists them with "Apply (restarts
//!   cameras)" and "Undo these" (`held`). The bar's Apply is the old Apply,
//!   with its questions while recording or live.
//! * While a live page shows, `rebase` keeps the draft on top of the
//!   settings in effect, so the page shows what the desk changed too.
//! * Leaving the page (another area, Show, Esc, quitting) sends whatever
//!   is waiting for its box to lose focus (`take_live`); held changes ask
//!   first, as unapplied changes always have.
//!
//! Inputs work the same way: a level, ON, processing, ducking or auto-mix
//! applies straight away; a new, removed or moved input, or a change to
//! its device, channels, name or delay (which reopens it) waits on the bar
//! as "Apply (restarts inputs)".
//!
//! The Scenes area works the same way: a scene renamed, moved, copied or
//! deleted on Scenes › Scenes, a transition or an auto-switching rule on
//! Switching, applies as made (the bank's CUT / MIX / ⋯ and the length
//! are the same settings as Scenes › Switching, so each shows the other's
//! change at once); the program's size or frame rate restarts the cameras,
//! so it waits on the bar.
//!
//! Pages drawn by the app itself (People & names) use the same rules
//! directly: they change `App::cfg`, call `remember` for Undo and save.

use super::*;
use std::time::{Duration, Instant};

/// The pages that save as you go.
pub const LIVE: &[Section] = &[Section::Sources, Section::Guests, Section::Audio, Section::Mixing, Section::Pads, Section::Syrinscape, Section::Recording, Section::Streaming, Section::Scenes, Section::Switching, Section::Program];

/// Whether a page saves as you go.
pub fn is_live(s: Section) -> bool {
    LIVE.contains(&s)
}

/// While a slider is dragged, it's applied at most this often.
const DRAG_EVERY: Duration = Duration::from_millis(250);

/// Copy the settings a page edits from `from` into `into` (the same fields
/// `SettingsUi::section_dirty` compares).
pub fn copy_section(s: Section, from: &Config, into: &mut Config) {
    let (a, b) = (into, from);
    match s {
        Section::General => a.ui = b.ui.clone(),
        Section::Sources => {
            a.sources = b.sources.clone();
            // (A phone camera turns guests on.)
            a.guests = b.guests.clone();
        }
        Section::Audio => a.audio = b.audio.clone(),
        Section::Mixing => {
            a.mix = b.mix.clone();
            // (Which inputs duck and auto-mix are ticked here too.)
            a.audio = b.audio.clone();
        }
        Section::Pads => {
            a.pads = b.pads.clone();
            // (Adds the Sound pads input when there's none.)
            a.audio = b.audio.clone();
        }
        Section::Syrinscape => a.syrinscape = b.syrinscape.clone(),
        Section::Rundown => a.rundown = b.rundown.clone(),
        Section::Scenes => {
            // A scene renamed, moved or deleted takes what picks it along
            // (macros, MIDI, keys, the rundown, auto-switching...), as the
            // page did to the draft; only that, so changes waiting on
            // another page's Apply stay there.
            let names = |l: &[crate::scene::Scene]| l.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
            let (was, now) = (names(&a.scenes), names(&b.scenes));
            crate::control::follow_scenes(a, &was, &now);
            a.scenes = b.scenes.clone();
        }
        Section::Macros => a.macros = b.macros.clone(),
        Section::Prompter => a.prompter = b.prompter.clone(),
        Section::Comments => a.comments = b.comments.clone(),
        Section::Guests => {
            a.guests = b.guests.clone();
            a.sources = b.sources.clone();
        }
        Section::Program => {
            a.program = b.program.clone();
            a.style = b.style.clone();
        }
        Section::Switching => {
            a.transition = b.transition.clone();
            a.auto_switch = b.auto_switch.clone();
            a.replay = b.replay.clone();
        }
        Section::Graphics => {
            a.graphics = b.graphics.clone();
            a.rotations = b.rotations.clone();
        }
        Section::Captions => a.captions = b.captions.clone(),
        Section::Recording => {
            a.output = b.output.clone();
            a.podcast = b.podcast.clone();
        }
        Section::Streaming => {
            a.streaming = b.streaming.clone();
            a.schedule = b.schedule.clone();
        }
        Section::Vertical => a.vertical = b.vertical.clone(),
        Section::Upload => a.upload = b.upload.clone(),
        Section::Outputs => {
            a.display = b.display.clone();
            a.ndi_out = b.ndi_out.clone();
        }
        Section::Control => {
            a.remote = b.remote.clone();
            a.midi = b.midi.clone();
            a.osc = b.osc.clone();
        }
        Section::Shortcuts => a.keys = b.keys.clone(),
        Section::Diagnostics => {}
    }
}

/// `edited` without what would reopen, add, remove or move a source or an
/// input, or rebuild the engine (compared with `applied`): what can be
/// applied straight away without interrupting anything.
pub fn without_reopening(applied: &Config, edited: &Config) -> Config {
    use crate::config::{audio_key, source_key};
    let mut out = edited.clone();
    out.program = applied.program.clone();
    out.sources = applied
        .sources
        .iter()
        .map(|o| match edited.sources.iter().find(|d| d.uid != 0 && d.uid == o.uid) {
            Some(d) if source_key(d) == source_key(o) => d.clone(),
            // Reopens: only what changes while it runs.
            Some(d) => SourceConfig {
                look: d.look.clone(),
                media: d.media.clone(),
                framing: d.framing.clone(),
                ptz: d.ptz.clone(),
                record: d.record,
                person: d.person.clone(),
                role: d.role.clone(),
                on_drop_scene: d.on_drop_scene.clone(),
                ..o.clone()
            },
            None => o.clone(),
        })
        .collect();
    out.audio = applied
        .audio
        .iter()
        .map(|o| match edited.audio.iter().find(|d| d.uid != 0 && d.uid == o.uid) {
            Some(d) if audio_key(d) == audio_key(o) => d.clone(),
            _ => o.clone(),
        })
        .collect();
    out
}

/// What the bar's Apply restarts, if anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Restarts {
    Nothing,
    /// An input reopens (its sound drops for a moment).
    Inputs,
    /// A camera or the program picture restarts.
    Cameras,
}

/// What a held change does, for the bar: "restarts Couch", "adds Couch
/// left", "adds Sound pads".
pub fn held_list(applied: &Config, edited: &Config) -> (Vec<String>, Restarts) {
    use crate::config::{audio_key, source_key};
    let mut out = Vec::new();
    let names = |v: Vec<String>| v.join(", ");
    let restarting: Vec<String> = edited
        .sources
        .iter()
        .filter(|d| d.uid != 0 && applied.sources.iter().any(|o| o.uid == d.uid && source_key(o) != source_key(d)))
        .map(|d| d.name.clone())
        .collect();
    let added: Vec<String> = edited.sources.iter().filter(|d| d.uid == 0 || !applied.sources.iter().any(|o| o.uid == d.uid)).map(|d| d.name.clone()).collect();
    let removed: Vec<String> = applied.sources.iter().filter(|o| !edited.sources.iter().any(|d| d.uid == o.uid)).map(|o| o.name.clone()).collect();
    if !restarting.is_empty() {
        out.push(format!("restarts {}", names(restarting.clone())));
    }
    if !added.is_empty() {
        out.push(format!("adds {}", names(added)));
    }
    if !removed.is_empty() {
        out.push(format!("removes {}", names(removed)));
    }
    let order = |c: &Config| c.sources.iter().map(|s| s.uid).filter(|u| *u != 0).collect::<Vec<_>>();
    let kept = |c: &Config, other: &Config| order(c).into_iter().filter(|u| other.sources.iter().any(|s| s.uid == *u)).collect::<Vec<_>>();
    if kept(applied, edited) != kept(edited, applied) {
        out.push("changes the order of the sources".into());
    }
    // The same for sound inputs.
    let reopening: Vec<String> = edited
        .audio
        .iter()
        .filter(|d| d.uid != 0 && applied.audio.iter().any(|o| o.uid == d.uid && audio_key(o) != audio_key(d)))
        .map(|d| d.name.clone())
        .collect();
    let added: Vec<String> = edited.audio.iter().filter(|d| d.uid == 0 || !applied.audio.iter().any(|o| o.uid == d.uid)).map(|d| d.name.clone()).collect();
    let removed: Vec<String> = applied.audio.iter().filter(|o| !edited.audio.iter().any(|d| d.uid == o.uid)).map(|o| o.name.clone()).collect();
    if !reopening.is_empty() {
        out.push(format!("restarts {}", names(reopening.clone())));
    }
    if !added.is_empty() {
        out.push(format!("adds {}", names(added)));
    }
    if !removed.is_empty() {
        out.push(format!("removes {}", names(removed)));
    }
    let order = |c: &Config| c.audio.iter().map(|a| a.uid).filter(|u| *u != 0).collect::<Vec<_>>();
    let kept = |c: &Config, other: &Config| order(c).into_iter().filter(|u| other.audio.iter().any(|a| a.uid == *u)).collect::<Vec<_>>();
    if kept(applied, edited) != kept(edited, applied) {
        out.push("changes the order of the inputs".into());
    }
    if applied.program != edited.program {
        out.push("restarts the program picture".into());
    }
    let restarts = if !restarting.is_empty() || applied.program != edited.program {
        Restarts::Cameras
    } else if !reopening.is_empty() {
        Restarts::Inputs
    } else {
        Restarts::Nothing
    };
    (out, restarts)
}

/// An Undo step's name for a change to the scenes: "Rename scene Wide",
/// "Delete scene Grid", "New scene Scene 10", "Move scene Maya".
fn scenes_label(before: &[crate::scene::Scene], after: &[crate::scene::Scene]) -> String {
    let names = |l: &[crate::scene::Scene]| l.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    let (b, a) = (names(before), names(after));
    if a.len() > b.len() {
        let new = a.iter().find(|n| !b.contains(n)).cloned().unwrap_or_default();
        return format!("New scene {new}");
    }
    if a.len() < b.len() {
        let gone = b.iter().find(|n| !a.contains(n)).cloned().unwrap_or_default();
        return format!("Delete scene {gone}");
    }
    if let Some((old, _)) = crate::control::scene_renames(&b, &a).first() {
        return format!("Rename scene {old}");
    }
    if b != a {
        // Moved: the one that moved furthest (↑ and ↓ swap two: the first).
        let dist = |n: &String, i: usize| a.iter().position(|x| x == n).map_or(0, |j| j.abs_diff(i));
        let moved = b.iter().enumerate().fold((0, None), |best, (i, n)| if dist(n, i) > best.0 { (dist(n, i), Some(n)) } else { best }).1.cloned().unwrap_or_default();
        return format!("Move scene {moved}");
    }
    "Scenes".into()
}

/// The audio inputs as far as reopening goes (not their levels, which an
/// old page may have changed and which apply live).
fn audio_keys(c: &Config) -> Vec<(u64, crate::config::AudioConfig)> {
    c.audio.iter().map(|a| (a.uid, crate::config::audio_key(a))).collect()
}

impl SettingsUi {
    /// The settings in effect plus every change on a live page that can be
    /// applied without interrupting anything.
    pub(super) fn live_part(&self) -> Config {
        let mut c = self.original.clone();
        for s in LIVE {
            copy_section(*s, &self.draft, &mut c);
        }
        without_reopening(&self.original, &c)
    }

    /// The draft without its live pages' changes held back for Apply.
    fn draft_compared(&self) -> Config {
        self.draft.with_framed_rates()
    }

    /// Changes not applied yet that the page doesn't save by itself: held
    /// back on a live page, or anything on an old one.
    pub(super) fn unapplied(&self) -> bool {
        self.draft_compared() != self.live_part().with_framed_rates() || self.scene_rename_pending()
    }

    /// Changes on a live page held back for "Apply (restarts cameras)".
    pub(super) fn held(&self, s: Section) -> bool {
        let (lp, d) = (self.live_part().with_framed_rates(), self.draft_compared());
        let mut a = Config::default();
        let mut b = Config::default();
        copy_section(s, &lp, &mut a);
        copy_section(s, &d, &mut b);
        a != b || (is_live(s) && audio_keys(&lp) != audio_keys(&d))
    }

    /// Keep the draft on top of the settings in effect (`now`): what was
    /// changed elsewhere shows here, and what's being edited here stays.
    pub fn rebase(&mut self, now: &Config) {
        if !is_live(self.section) || self.original == *now {
            return;
        }
        self.draft = now.with_edits(&self.original, &self.draft);
        self.original = now.clone();
    }

    /// This frame's change to apply, if it's time (see the module's notes).
    pub(super) fn live_step(&mut self, ctx: &egui::Context) -> Option<SettingsAction> {
        if !is_live(self.section) {
            return None;
        }
        let lp = self.live_part();
        if lp == self.original || self.failed.as_deref() == Some(&lp) || lp.check().is_err() {
            return None;
        }
        // Still typing: wait for Enter or a click elsewhere.
        if ctx.egui_wants_keyboard_input() {
            return None;
        }
        if ctx.input(|i| i.pointer.any_down()) && self.last_live.is_some_and(|t| t.elapsed() < DRAG_EVERY) {
            ctx.request_repaint_after(DRAG_EVERY);
            return None;
        }
        self.last_live = Some(Instant::now());
        let label = self.live_label(&lp);
        Some(SettingsAction::Live { cfg: lp, base: Box::new(self.original.clone()), label })
    }

    /// Leaving: what's waiting to be applied, whatever has the keyboard.
    pub fn take_live(&mut self) -> Option<SettingsAction> {
        // (A scene's new name still in its box counts.)
        if is_live(self.section) {
            self.commit_scene_rename();
        }
        let lp = self.live_part();
        if lp == self.original || lp.check().is_err() {
            return None;
        }
        let label = self.live_label(&lp);
        let base = Box::new(self.original.clone());
        // Counted as applied from here (the app applies it next).
        self.original = lp.clone();
        Some(SettingsAction::Live { cfg: lp, base, label })
    }

    /// Called by the app after it applied a live change (`sent`), with the
    /// settings now in effect.
    pub fn applied_live(&mut self, result: Result<(), String>, sent: &Config, now: &Config) {
        match result {
            Ok(()) => {
                self.failed = None;
                self.message = None;
                self.rebase_any(now);
            }
            Err(e) => {
                self.message = Some((e, true));
                self.failed = Some(Box::new(sent.clone()));
                // (The draft keeps the change, so it shows as not saved.)
                self.original = now.clone();
            }
        }
    }

    /// An Undo or Redo put settings back (`now`): a page that saves as you
    /// go keeps what's held back on it; the others count as applied, as
    /// they always have.
    pub fn restored(&mut self, now: &Config) {
        if is_live(self.section) {
            self.rebase_any(now);
        } else {
            self.applied(Ok(()), now);
        }
    }

    fn rebase_any(&mut self, now: &Config) {
        if self.original != *now {
            self.draft = now.with_edits(&self.original, &self.draft);
            self.original = now.clone();
        }
    }

    /// What an Undo step of this change is called ("Undone: …").
    fn live_label(&self, lp: &Config) -> String {
        let o = &self.original;
        let changed: Vec<&SourceConfig> = lp.sources.iter().filter(|s| !o.sources.iter().any(|x| x == *s)).collect();
        let inputs: Vec<&AudioConfig> = lp.audio.iter().filter(|a| !o.audio.iter().any(|x| x == *a)).collect();
        match (changed.as_slice(), inputs.as_slice()) {
            ([one], []) => format!("{} settings", one.name),
            ([], [one]) if lp.mix == o.mix && lp.pads == o.pads => format!("{} settings", one.name),
            ([], _) if lp.guests != o.guests => "Remote guests settings".into(),
            ([], _) if lp.pads != o.pads => "Sound pads".into(),
            ([], _) if lp.syrinscape != o.syrinscape => "Syrinscape settings".into(),
            ([], _) if lp.mix.monitor_device != o.mix.monitor_device || lp.mix.monitor_source != o.mix.monitor_source || lp.mix.monitor_volume_db != o.mix.monitor_volume_db => "Headphones".into(),
            ([], _) if lp.mix != o.mix => "Mixer settings".into(),
            ([], _) if lp.scenes != o.scenes => scenes_label(&o.scenes, &lp.scenes),
            ([], _) if lp.transition.preview_mode != o.transition.preview_mode => "Preview".into(),
            ([], _) if lp.transition != o.transition => "Transition".into(),
            ([], _) if lp.auto_switch != o.auto_switch => "Auto-switching".into(),
            ([], _) if lp.replay != o.replay => "Replay settings".into(),
            ([], _) if lp.style != o.style => "Program picture".into(),
            ([], []) => "Settings change".into(),
            ([], _) => "Sound inputs".into(),
            _ => "Sources settings".into(),
        }
    }

    /// "Undo these": the held changes go; what's applied stays.
    fn drop_held(&mut self) {
        let lp = self.live_part();
        for s in LIVE {
            copy_section(*s, &lp, &mut self.draft);
        }
        // (Inputs added, removed or reopened go; a level changed on Sound stays.)
        self.draft.audio = without_reopening(&self.original, &self.draft).audio;
        self.draft.program = self.original.program.clone();
    }

    /// The bar under a live page: Undo and Redo, how saving is going, and
    /// the held changes with their Apply.
    pub(super) fn live_bar(&mut self, ui: &mut egui::Ui, info: &SettingsContext) -> Option<SettingsAction> {
        let mut action = None;
        let valid = self.draft.check();
        let (held, restarts) = held_list(&self.original, &self.draft);
        let held = if self.held(self.section) { held } else { Vec::new() };
        let live = info.recording || info.streaming;
        let typing = ui.ctx().egui_wants_keyboard_input() && self.live_part() != self.original;
        let failed = match (&self.message, &self.failed) {
            (Some((m, true)), Some(_)) => Some(m.clone()),
            _ => None,
        };
        ui.horizontal(|ui| {
            let undo = ui.add_enabled(info.undo.is_some(), egui::Button::new("↶ Undo")).on_hover_text(match &info.undo {
                Some(l) => format!("Undo: {l} (⌘Z when you're not typing)"),
                None => "Nothing to undo".into(),
            });
            if undo.clicked() {
                action = Some(SettingsAction::Undo);
            }
            let redo = ui.add_enabled(info.redo.is_some(), egui::Button::new("↷ Redo")).on_hover_text(match &info.redo {
                Some(l) => format!("Redo: {l}"),
                None => "Nothing to redo".into(),
            });
            if redo.clicked() {
                action = Some(SettingsAction::Redo);
            }
            ui.add_space(8.0);
            if let Err(e) = &valid {
                let r = ui.add(egui::Label::new(RichText::new(format!("⚠ {}", e.message)).color(AMBER)).sense(egui::Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click to go there").clicked() {
                    match e.at {
                        crate::config::ProblemAt::Source(i) => {
                            self.sel_source = i;
                            action = Some(SettingsAction::Go("sources/cameras/list"));
                        }
                        crate::config::ProblemAt::Audio(i) => {
                            self.sel_audio = i;
                            action = Some(SettingsAction::Go("sound/inputs/inputs"));
                        }
                        _ => {}
                    }
                }
            } else if let Some(m) = &failed {
                ui.label(RichText::new(m).color(ERROR));
            } else if typing {
                ui.label(RichText::new("Saves when you press Enter or click elsewhere").color(TEXT_DIM));
            } else if held.is_empty() {
                ui.label(RichText::new("✓ Changes save as you make them").color(TEXT_DIM));
            }
            if !held.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let blocked = live && self.original.needs_restart(&self.draft);
                    let can = valid.is_ok() && !blocked;
                    let text = match restarts {
                        Restarts::Cameras => "Apply (restarts cameras)",
                        Restarts::Inputs => "Apply (restarts inputs)",
                        Restarts::Nothing => "Apply",
                    };
                    let b = egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(0.0, 28.0));
                    if ui.add_enabled(can, b).on_hover_text("These changes open, close or restart sources or sound inputs, so they wait for you").on_disabled_hover_text("Fix what's marked first").clicked() {
                        action = Some(self.apply(false, info));
                    }
                    if ui.button("Undo these").on_hover_text("Forget the changes waiting here (what's already saved stays)").clicked() {
                        self.drop_held();
                    }
                    let what = format!("Waiting: {}", held.join(" · "));
                    ui.add(egui::Label::new(RichText::new(what.clone()).color(AMBER)).truncate()).on_hover_text(what);
                });
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Config {
        let mut c = Config::default();
        c.sources = vec![
            SourceConfig { name: "Couch".into(), device: "test".into(), ..Default::default() },
            SourceConfig { name: "Host".into(), device: "test".into(), ..Default::default() },
        ];
        c.assign_uids();
        c
    }

    /// Who's in a picture and its look go straight on; a new device waits.
    #[test]
    fn reopening_changes_are_held_back() {
        let applied = two();
        let mut edited = applied.clone();
        edited.sources[0].person = "Maya".into();
        edited.sources[0].look.mirror = true;
        edited.sources[1].device = "test:bars".into();
        edited.sources.push(SourceConfig { name: "New".into(), device: "test".into(), ..Default::default() });
        let now = without_reopening(&applied, &edited);
        assert_eq!(now.sources.len(), 2);
        assert_eq!(now.sources[0].person, "Maya");
        assert!(now.sources[0].look.mirror);
        assert_eq!(now.sources[1].device, "test");
        assert!(!applied.lineup_changed(&now), "nothing reopens");
        let (list, restarts) = held_list(&applied, &edited);
        assert_eq!(restarts, Restarts::Cameras);
        assert_eq!(list, vec!["restarts Host".to_string(), "adds New".to_string()]);
    }

    /// The page applies its live changes, keeps the held ones, and a desk
    /// change made meanwhile shows on the page and isn't undone.
    #[test]
    fn saves_as_you_go_and_keeps_desk_changes() {
        let applied = two();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Sources);
        win.draft.sources[0].person = "Maya".into();
        win.draft.sources[1].device = "test:bars".into();
        assert!(win.held(Section::Sources));
        let ctx = egui::Context::default();
        let Some(SettingsAction::Live { cfg, base, .. }) = win.live_step(&ctx) else { panic!("nothing to save") };
        // Meanwhile on the desk: a PTZ preset saved on the Host camera.
        let mut live = applied.clone();
        live.sources[1].ptz.presets.push(crate::config::PtzPreset { name: "Close".into(), ..Default::default() });
        let now = live.with_edits(&base, &cfg);
        assert_eq!(now.sources[0].person, "Maya");
        assert_eq!(now.sources[1].ptz.presets.len(), 1, "the desk's preset stays");
        win.applied_live(Ok(()), &cfg, &now);
        // Applied: nothing more to send, the held change still waits, the preset shows.
        assert!(win.live_step(&ctx).is_none());
        assert_eq!(win.draft.sources[1].device, "test:bars");
        assert_eq!(win.draft.sources[1].ptz.presets.len(), 1);
        assert!(win.held(Section::Sources));
        win.drop_held();
        assert!(!win.held(Section::Sources));
        assert!(!win.unapplied());
    }

    /// A change that failed isn't sent again until it changes; leaving
    /// sends what's waiting even while typing.
    #[test]
    fn failed_changes_wait_and_leaving_sends_the_rest() {
        let applied = two();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Sources);
        win.draft.sources[0].role = "Host".into();
        let ctx = egui::Context::default();
        let Some(SettingsAction::Live { cfg, .. }) = win.live_step(&ctx) else { panic!() };
        win.applied_live(Err("no".into()), &cfg, &applied);
        assert!(win.live_step(&ctx).is_none(), "not sent again");
        win.draft.sources[0].role = "Hostess".into();
        assert!(win.take_live().is_some());
        assert!(!win.unapplied(), "counted as applied once handed over");
    }

    /// The source picked last time comes back by name (forensics settings
    /// #2), unless one was asked for; a gone one leaves the first.
    #[test]
    fn remembers_the_picked_source_by_name() {
        let mut c = two();
        let mut win = SettingsUi::new(c.clone());
        win.restore_source("Host");
        assert_eq!(win.source_name().as_deref(), Some("Host"));
        let mut win = SettingsUi::new(c.clone()).select_source(0);
        win.restore_source("Host");
        assert_eq!(win.source_name().as_deref(), Some("Couch"), "an asked-for source stays");
        c.sources.swap(0, 1);
        let mut win = SettingsUi::new(c.clone());
        win.restore_source("Gone");
        assert_eq!(win.sel_source, 0);
    }

    /// A level changed on an old page isn't dropped by "Undo these".
    #[test]
    fn undo_these_keeps_other_pages_levels() {
        let mut applied = two();
        applied.audio = vec![crate::config::AudioConfig { name: "Mic".into(), device: "test".into(), ..Default::default() }];
        applied.assign_uids();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Sources);
        win.draft.audio[0].gain_db = 6.0;
        win.draft.sources[1].device = "test:bars".into();
        assert!(win.held(Section::Sources));
        win.drop_held();
        assert_eq!(win.draft.audio[0].gain_db, 6.0);
        assert!(!win.held(Section::Sources));
    }

    /// An old page (not saving as you go) keeps its draft until Apply.
    #[test]
    fn old_pages_wait_for_apply() {
        let applied = two();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Prompter);
        win.draft.prompter.mirror = !win.draft.prompter.mirror;
        assert!(win.live_step(&egui::Context::default()).is_none());
        assert!(win.unapplied());
        assert!(!is_live(Section::Prompter));
    }

    fn with_mics() -> Config {
        let mut c = two();
        c.audio = vec![
            crate::config::AudioConfig { name: "Host mic".into(), device: "test".into(), ..Default::default() },
            crate::config::AudioConfig { name: "Guest mic".into(), device: "test".into(), ..Default::default() },
        ];
        c.assign_uids();
        c
    }

    /// Sound › Inputs: a level, ON or processing goes straight on; a new
    /// input or a new device waits on the bar, which says what it does.
    #[test]
    fn inputs_save_as_you_go_and_new_ones_wait() {
        let applied = with_mics();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Audio);
        win.draft.audio[0].gain_db = -52.5;
        win.draft.audio[0].processing.enabled = true;
        win.draft.audio[1].device = "test:other".into();
        win.draft.audio.push(crate::config::AudioConfig { name: "USB mic".into(), device: "test".into(), ..Default::default() });
        assert!(win.held(Section::Audio));
        let (list, restarts) = held_list(&win.original, &win.draft);
        assert_eq!(list, vec!["restarts Guest mic".to_string(), "adds USB mic".to_string()]);
        assert_eq!(restarts, Restarts::Inputs);
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&egui::Context::default()) else { panic!("nothing to save") };
        assert_eq!(label, "Host mic settings");
        assert_eq!(cfg.audio.len(), 2, "the new input waits");
        assert_eq!(cfg.audio[0].gain_db, -52.5, "not rounded or pulled up to −40 (numbers audit #2)");
        assert_eq!(cfg.audio[1].device, "test", "the new device waits");
        assert!(!applied.lineup_changed(&cfg), "nothing reopens");
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(win.held(Section::Audio));
        win.drop_held();
        assert!(!win.held(Section::Audio));
        assert_eq!(win.draft.audio[0].gain_db, -52.5, "what's saved stays");
    }

    /// Sound › Mixer and Headphones & talkback apply as made; Sound › Pads
    /// saves its pads, and the Sound pads input it adds waits for Apply.
    #[test]
    fn mixer_and_pads_save_as_you_go() {
        let applied = with_mics();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Mixing);
        win.draft.mix.limiter = !win.draft.mix.limiter;
        win.draft.audio[1].duck = true;
        let ctx = egui::Context::default();
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&ctx) else { panic!() };
        assert_eq!(cfg.mix.limiter, !applied.mix.limiter);
        assert!(cfg.audio[1].duck, "the ticks on the inputs go with it");
        assert_eq!(label, "Mixer settings");
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(!win.unapplied());

        let applied = cfg;
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Pads);
        win.draft.pads.push(crate::config::Pad { name: "Applause".into(), ..Default::default() });
        win.draft.audio.push(crate::config::AudioConfig { name: "Sound pads".into(), device: "pads".into(), ..Default::default() });
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&ctx) else { panic!() };
        assert_eq!(label, "Sound pads");
        assert_eq!(cfg.pads.len(), 1);
        assert_eq!(cfg.audio.len(), 2, "the input waits");
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(win.held(Section::Pads));
        assert_eq!(held_list(&win.original, &win.draft), (vec!["adds Sound pads".to_string()], Restarts::Nothing));
    }
    fn with_scenes() -> Config {
        let mut c = two();
        c.scenes = ["Wide", "Host", "Grid"].iter().map(|n| crate::scene::Scene { name: n.to_string(), ..Default::default() }).collect();
        c.rundown.segments.push(crate::config::Segment { name: "Intro".into(), scene: "Host".into(), ..Default::default() });
        c.macros.push(crate::config::Macro { name: "Go".into(), commands: vec!["scene:3".into()], ..Default::default() });
        c
    }

    /// Scenes › Scenes: a rename applies when the box is left, with what
    /// picks the scene by name; a move takes "scene:3" along; a rundown
    /// edit waiting on its own page's Apply isn't sent with them.
    #[test]
    fn scenes_save_as_you_go_with_what_picks_them() {
        let applied = with_scenes();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Scenes);
        // Waiting on the old Rundown page (not saving as you go).
        win.draft.rundown.segments[0].name = "Opening".into();
        win.scene_rename = Some((1, "Presenter".into()));
        win.commit_scene_rename();
        let ctx = egui::Context::default();
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&ctx) else { panic!("nothing to save") };
        assert_eq!(label, "Rename scene Host");
        assert_eq!(cfg.scenes[1].name, "Presenter");
        assert_eq!(cfg.rundown.segments[0].scene, "Presenter", "the rundown follows the name");
        assert_eq!(cfg.rundown.segments[0].name, "Intro", "the Rundown page's edit waits for its Apply");
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(!win.held(Section::Scenes));
        // Grid moved to the top: the macro that took the third scene follows it.
        let before: Vec<String> = win.draft.scenes.iter().map(|s| s.name.clone()).collect();
        let grid = win.draft.scenes.remove(2);
        win.draft.scenes.insert(0, grid);
        let after: Vec<String> = win.draft.scenes.iter().map(|s| s.name.clone()).collect();
        crate::control::follow_scenes(&mut win.draft, &before, &after);
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&ctx) else { panic!("nothing to save") };
        assert_eq!(label, "Move scene Grid");
        assert_eq!(cfg.macros[0].commands, vec!["scene:1".to_string()]);
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(!win.held(Section::Scenes));
        assert!(win.unapplied(), "only the rundown's own edit is left");
    }

    /// A name still being typed is applied when you leave the page.
    #[test]
    fn leaving_scenes_keeps_a_name_being_typed() {
        let applied = with_scenes();
        let mut win = SettingsUi::new(applied).open_at(Section::Scenes);
        win.scene_rename = Some((0, "Wide shot".into()));
        let Some(SettingsAction::Live { cfg, .. }) = win.take_live() else { panic!("nothing handed over") };
        assert_eq!(cfg.scenes[0].name, "Wide shot");
        assert!(!win.unapplied());
    }

    /// Forensics settings #6: the transition is one setting. CUT / MIX and
    /// the length on the desk show on Scenes › Switching at once, and a
    /// change there applies at once (so the bank shows it).
    #[test]
    fn switching_and_the_bank_are_one_setting() {
        use crate::config::TransitionMode;
        let applied = two();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Switching);
        // On the desk: MIX, 700 ms.
        let mut desk = applied.clone();
        desk.transition.mode = TransitionMode::Mix;
        desk.transition.duration_ms = 700;
        win.rebase(&desk);
        assert_eq!(win.draft.transition.mode, TransitionMode::Mix);
        assert_eq!(win.draft.transition.duration_ms, 700);
        assert!(!win.unapplied(), "nothing to apply: the page shows what's in effect");
        // On the page: Wipe.
        win.draft.transition.mode = TransitionMode::Wipe;
        let ctx = egui::Context::default();
        let Some(SettingsAction::Live { cfg, base, label }) = win.live_step(&ctx) else { panic!("nothing to save") };
        assert_eq!(label, "Transition");
        // Meanwhile on the desk the length went to 900: kept.
        let mut desk2 = desk.clone();
        desk2.transition.duration_ms = 900;
        let now = desk2.with_edits(&base, &cfg);
        assert_eq!(now.transition.mode, TransitionMode::Wipe);
        assert_eq!(now.transition.duration_ms, 900);
        win.applied_live(Ok(()), &cfg, &now);
        assert_eq!(win.draft.transition.duration_ms, 900);
        assert!(!win.unapplied());
        // The bank and the page grey the length for the same reasons.
        assert!(TransitionMode::Cut.fixed_length().is_some() && TransitionMode::Stinger.fixed_length().is_some());
        assert!(TransitionMode::Mix.fixed_length().is_none());
    }

    /// Scenes › More › Program picture: the frame style applies at once;
    /// a new size restarts the cameras, so it waits on the bar.
    #[test]
    fn program_size_waits_and_frame_style_goes_on() {
        let applied = two();
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Program);
        win.draft.style.corner_radius = 12.0;
        win.draft.program.width = 1280;
        win.draft.program.height = 720;
        let Some(SettingsAction::Live { cfg, label, .. }) = win.live_step(&egui::Context::default()) else { panic!() };
        assert_eq!(label, "Program picture");
        assert_eq!(cfg.style.corner_radius, 12.0);
        assert!(!applied.needs_restart(&cfg), "nothing restarts");
        win.applied_live(Ok(()), &cfg, &cfg);
        assert!(win.held(Section::Program));
        let (list, restarts) = held_list(&win.original, &win.draft);
        assert_eq!((list, restarts), (vec!["restarts the program picture".to_string()], Restarts::Cameras));
    }

    #[test]
    fn scene_changes_are_named_for_undo() {
        let sc = |names: &[&str]| names.iter().map(|n| crate::scene::Scene { name: n.to_string(), ..Default::default() }).collect::<Vec<_>>();
        assert_eq!(scenes_label(&sc(&["A", "B"]), &sc(&["A", "B", "C"])), "New scene C");
        assert_eq!(scenes_label(&sc(&["A", "B"]), &sc(&["B"])), "Delete scene A");
        assert_eq!(scenes_label(&sc(&["A", "B"]), &sc(&["A", "Bee"])), "Rename scene B");
        assert_eq!(scenes_label(&sc(&["A", "B"]), &sc(&["B", "A"])), "Move scene A");
    }
}
