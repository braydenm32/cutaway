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
//! Pages drawn by the app itself (People & names) use the same rules
//! directly: they change `App::cfg`, call `remember` for Undo and save.

use super::*;
use std::time::{Duration, Instant};

/// The pages that save as you go.
pub const LIVE: &[Section] = &[Section::Sources, Section::Guests, Section::Audio, Section::Mixing, Section::Pads, Section::Syrinscape];

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
        Section::Scenes => a.scenes = b.scenes.clone(),
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
        let mut win = SettingsUi::new(applied.clone()).open_at(Section::Recording);
        win.draft.output.export_timelines = !win.draft.output.export_timelines;
        assert!(win.live_step(&egui::Context::default()).is_none());
        assert!(win.unapplied());
        assert!(!is_live(Section::Recording));
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
}
