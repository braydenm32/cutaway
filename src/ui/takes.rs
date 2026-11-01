//! A take once it's saved: the "Take saved" card under the Record button
//! (length, size, markers, whether everything was saved), Record › Takes
//! (the recent takes, each with its markers), and what to do with a take
//! next: show it in Finder / Explorer, open it in Resolve, upload it.
//!
//! The card and the list say the same things the same way: the size is
//! that of the files the take recorded (as Take.txt lists them), never the
//! folder with its notes and clips added.

use super::*;
use crate::engine::recorder::TakeReport;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Takes {
    /// The take the card is about (None: no card).
    card: Option<PathBuf>,
    /// The last take seen, so the card shows once for each.
    seen: Option<PathBuf>,
    /// What Open in Resolve did for a take (shown on its card and its row).
    resolve_note: Option<(PathBuf, String)>,
    /// Record › Takes' list, read again when the page opens.
    recent: Vec<RecentTake>,
    /// The frame the list was last drawn (a gap means it just opened).
    recent_frame: u64,
}

/// A take in Record › Takes, from its folder and Take.txt.
#[derive(Clone, Debug)]
pub(super) struct RecentTake {
    pub folder: PathBuf,
    pub recorded: Option<chrono::NaiveDateTime>,
    pub length: Option<Duration>,
    /// The files it recorded (Take.txt's Files list), and their size.
    pub size: u64,
    pub files: usize,
    pub markers: Vec<(f64, String)>,
    /// What Take.txt says went wrong.
    pub problems: Vec<String>,
    /// Rescued after Cutaway stopped mid-take.
    pub recovered: bool,
}

/// "01:02:03" → 1h 2m 3s.
fn parse_hms(s: &str) -> Option<Duration> {
    let p: Vec<u64> = s.trim().split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    match p[..] {
        [h, m, sec] => Some(Duration::from_secs(h * 3600 + m * 60 + sec)),
        [m, sec] => Some(Duration::from_secs(m * 60 + sec)),
        _ => None,
    }
}

/// "  Program.mp4  (1.2 GB)" → "Program.mp4": a file the take recorded, as
/// Take.txt lists them (not its notes: "  Switches.csv  (every scene…)").
fn listed_file(line: &str) -> Option<&str> {
    let (name, size) = line.trim().rsplit_once("  (")?;
    let size = size.strip_suffix(')')?;
    let (n, unit) = size.split_once(' ')?;
    (n.parse::<f64>().is_ok() && ["KB", "MB", "GB", "TB", "B"].contains(&unit) && !name.trim().is_empty()).then(|| name.trim())
}

/// What a take's Take.txt says: when, how long, its files, its markers,
/// any problems. (`size` is filled in from the files.)
fn read_note(folder: &Path, note: &str) -> (RecentTake, Vec<String>) {
    let mut t = RecentTake { folder: folder.to_path_buf(), recorded: None, length: None, size: 0, files: 0, markers: Vec::new(), problems: Vec::new(), recovered: false };
    t.recovered = note.lines().next().is_some_and(|l| l.contains("(recovered)"));
    let mut listed = Vec::new();
    let mut section = "";
    for line in note.lines() {
        if let Some(v) = line.strip_prefix("Recorded: ") {
            t.recorded = chrono::NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%d %H:%M").ok();
        } else if let Some(v) = line.strip_prefix("Length: ") {
            t.length = parse_hms(v);
        } else if !line.starts_with(' ') && line.ends_with(':') {
            section = line;
        } else if section == "Files:" {
            if let Some(f) = listed_file(line) {
                listed.push(f.to_string());
            }
        } else if section == "Problems:" && line.starts_with("  ") {
            t.problems.push(line.trim().to_string());
        } else if section == "Markers:" {
            // "  00:02:00  News  (on air: Wide)"
            let l = line.trim();
            let l = l.rsplit_once("  (on air:").map(|x| x.0).unwrap_or(l);
            if let Some((at, name)) = l.split_once("  ") {
                if let Some(d) = parse_hms(at) {
                    t.markers.push((d.as_secs_f64(), name.trim().to_string()));
                }
            }
        }
    }
    (t, listed)
}

/// Every file under `dir` with its size.
fn files_in(dir: &Path, out: &mut Vec<(PathBuf, u64)>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            if depth < 3 {
                files_in(&e.path(), out, depth + 1);
            }
        } else if !e.file_name().to_string_lossy().starts_with('.') {
            out.push((e.path(), m.len()));
        }
    }
}

/// The newest `n` takes in the recording folder (folders with a Take.txt).
pub(super) fn recent_takes(output: &Path, n: usize) -> Vec<RecentTake> {
    let Ok(rd) = std::fs::read_dir(output) else { return Vec::new() };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = std::fs::metadata(e.path().join("Take.txt")).and_then(|m| m.modified()).ok()?;
            Some((m, e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(n)
        .map(|(_, dir)| {
            let note = std::fs::read_to_string(dir.join("Take.txt")).unwrap_or_default();
            let (mut t, listed) = read_note(&dir, &note);
            let sizes: Vec<u64> = if listed.is_empty() {
                // A rescued take lists its files another way: everything
                // in it but the note.
                let mut files = Vec::new();
                files_in(&dir, &mut files, 0);
                files.into_iter().filter(|f| f.0 != dir.join("Take.txt")).map(|f| f.1).collect()
            } else {
                listed.iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).collect()
            };
            t.size = sizes.iter().sum();
            t.files = sizes.len();
            t
        })
        .collect()
}

/// A take in one line, the same on the card and in Record › Takes:
/// "42m 10s · 3.1 GB · 7 files".
pub(super) fn take_line(length: Option<Duration>, size: u64, files: usize) -> String {
    let mut parts = Vec::new();
    if let Some(l) = length {
        parts.push(fmt_length(l));
    }
    parts.push(crate::util::human_size(size));
    parts.push(if files == 1 { "1 file".to_string() } else { format!("{files} files") });
    parts.join(" · ")
}

/// "◆ 3 markers", after the take line.
pub(super) fn markers_text(n: usize) -> String {
    match n {
        0 => "no markers".to_string(),
        1 => "◆ 1 marker".to_string(),
        n => format!("◆ {n} markers"),
    }
}

/// A saved take as uploads need it, rebuilt from its folder.
pub(super) fn report_from_folder(t: &RecentTake) -> TakeReport {
    let mut files = Vec::new();
    files_in(&t.folder, &mut files, 0);
    files.sort();
    let podcast = files.iter().map(|f| &f.0).find(|p| p.parent() == Some(t.folder.as_path()) && p.to_string_lossy().ends_with("(Podcast).m4a")).cloned();
    use chrono::TimeZone;
    TakeReport {
        folder: t.folder.clone(),
        files,
        problems: Vec::new(),
        duration: t.length.unwrap_or_default(),
        markers: t.markers.clone(),
        recorded_at: t.recorded.and_then(|r| chrono::Local.from_local_datetime(&r).earliest()),
        podcast_pending: false,
        podcast,
    }
}

/// What to do with a take.
pub(super) enum TakeAction {
    Reveal(PathBuf),
    Resolve(PathBuf),
    Upload(TakeReport),
}

pub(super) fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Show the folder"
    }
}

/// A take's length, the way people say it: "42m 10s", "1h 02m".
pub(super) fn fmt_length(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// A note the backup drive was left (the take's folder name, beside it).
pub(super) fn backup_incomplete(cfg: &Config, folder: &Path) -> Option<PathBuf> {
    let b = cfg.output.backup_folder.as_ref().filter(|p| !p.as_os_str().is_empty())?;
    let note = b.join(folder.file_name()?).join("BACKUP INCOMPLETE.txt");
    note.exists().then_some(note)
}

/// Whether the Resolve script is installed, and whether this take has the
/// file it builds the project from.
fn resolve_ready(folder: &Path) -> (bool, bool) {
    let installed = crate::engine::sessions::resolve_scripts_dir().join(crate::engine::sessions::RESOLVE_SCRIPT_NAME).exists();
    let has_take = std::fs::read_dir(folder).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with("(Resolve).lua"))).unwrap_or(false);
    (installed, has_take)
}

/// What Open in Resolve says it did, in one line.
fn resolve_message(opened: bool, script: bool, timeline: bool) -> &'static str {
    match (opened, script, timeline) {
        (true, true, _) => "Opening DaVinci Resolve. For the whole project: Workspace › Scripts › Cutaway (it starts at this take).",
        (true, false, true) => "Opening DaVinci Resolve with this take's timeline. If it doesn't appear: File › Import › Timeline… (its path is copied).",
        (true, false, false) => "Opening DaVinci Resolve. This take has no timeline (Record › Recording › Editing saves one), so its folder is copied.",
        (false, _, true) => "DaVinci Resolve wasn't found. The timeline's path is copied: in Resolve, File › Import › Timeline…",
        (false, _, false) => "DaVinci Resolve wasn't found. The take's folder is copied.",
    }
}

impl App {
    pub(super) fn take_action(&mut self, action: TakeAction, ctx: &egui::Context) {
        match action {
            TakeAction::Reveal(folder) => crate::logging::reveal_file(&folder),
            TakeAction::Resolve(folder) => {
                let note = self.open_in_resolve(&folder, ctx);
                self.takes.resolve_note = Some((folder, note));
            }
            TakeAction::Upload(report) => {
                if self.uploads.offer.as_ref().is_some_and(|o| o.folder == report.folder) {
                    self.uploads.offer = None;
                }
                self.upload_take(report);
            }
        }
    }

    /// Starts DaVinci Resolve with the take: its live-edit timeline (the
    /// .fcpxml), or, with Cutaway's Resolve script set up, ready to build
    /// the whole project from it. The timeline's path (or the folder) is
    /// copied either way, for File › Import if Resolve needs it. Says what
    /// happened.
    fn open_in_resolve(&mut self, folder: &Path, ctx: &egui::Context) -> String {
        let (installed, has_take) = resolve_ready(folder);
        let script = installed && has_take;
        if script {
            // The script starts at the take noted here.
            if let Some(dir) = crate::config::default_config_path().parent() {
                let _ = std::fs::write(crate::engine::sessions::last_take_file(dir), folder.to_string_lossy().as_bytes());
            }
        }
        let timeline = crate::engine::sessions::take_timeline(folder);
        ctx.copy_text(timeline.as_deref().unwrap_or(folder).display().to_string());
        let opened = match crate::engine::sessions::open_in_resolve(if script { None } else { timeline.as_deref() }) {
            Ok(()) => true,
            Err(e) => {
                if let Some(engine) = &self.engine {
                    engine.status.lock_ok().push(LogLevel::Info, format!("Open in Resolve: {e:#}"));
                }
                false
            }
        };
        resolve_message(opened, script, timeline.is_some()).to_string()
    }

    /// The takes list reads the folder again next time it's drawn.
    pub(super) fn look_for_takes_again(&mut self) {
        self.takes.recent_frame = 0;
    }

    /// Record › Takes: the last ten takes in the recording folder, newest
    /// first, each with its markers and the card's buttons. The newest
    /// take's markers are open; a rescued take says so.
    pub(super) fn takes_list(&mut self, ui: &mut egui::Ui) {
        let frame = ui.ctx().cumulative_frame_nr();
        if frame > self.takes.recent_frame + 1 {
            self.takes.recent = recent_takes(&self.cfg.output.folder, 10);
        }
        self.takes.recent_frame = frame;
        let folder = self.cfg.output.folder.clone();
        let mut action = None;
        // One quiet line: where they are.
        ui.horizontal(|ui| {
            let n = self.takes.recent.len();
            let what = match n {
                0 => "No takes yet in".to_string(),
                1 => "1 take in".to_string(),
                n => format!("The last {n} takes in"),
            };
            ui.label(RichText::new(what).color(TEXT_DIM));
            ui.add(egui::Label::new(RichText::new(folder.display().to_string()).color(TEXT_DIM)).truncate());
            if folder.exists() && ui.small_button(reveal_label()).clicked() {
                action = Some(TakeAction::Reveal(folder.clone()));
            }
        });
        ui.add_space(6.0);
        if self.takes.recent.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("Press Record and each take shows here when it stops: how long, how big, its markers, and buttons to open it in Resolve, find it or upload it.").color(TEXT_DIM));
        }
        let upload = self.cfg.upload.ready();
        let sent: Vec<String> = self.uploads.jobs.lock_ok().iter().filter(|j| matches!(j.state, uploads::JobState::Waiting | uploads::JobState::Uploading | uploads::JobState::Done)).map(|j| j.take.clone()).collect();
        let last = self.engine.as_ref().and_then(|e| e.status.lock_ok().last_take.clone());
        for (i, t) in self.takes.recent.iter().enumerate() {
            let name = t.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
            // The take just saved: its report knows what the note can't yet
            // (the podcast copy being made, a backup that didn't finish).
            let report = last.as_ref().filter(|r| r.folder == t.folder);
            let mut problems = t.problems.clone();
            if let Some(note) = backup_incomplete(&self.cfg, &t.folder) {
                if !problems.iter().any(|p| p.contains("BACKUP INCOMPLETE") || p.contains("backup")) {
                    problems.push(format!("The backup copy didn't finish (see {})", note.display()));
                }
            }
            let pending = report.is_some_and(|r| r.podcast_pending);
            egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 330.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&name).size(TEXT_STATE).color(Color32::WHITE)).truncate()).on_hover_text(t.folder.display().to_string());
                            if t.recovered {
                                ui.label(RichText::new("Recovered").size(TEXT_SMALL).color(AMBER)).on_hover_text("Rescued after Cutaway stopped mid-take: the files play up to about the last second written");
                            }
                        });
                        let mut line = Vec::new();
                        if let Some(r) = t.recorded {
                            line.push(r.format(if self.cfg.ui.clock_12h { "%-d %b %-I:%M %p" } else { "%-d %b %H:%M" }).to_string());
                        }
                        line.push(take_line(t.length, t.size, t.files));
                        line.push(markers_text(t.markers.len()));
                        ui.label(RichText::new(line.join(" · ")).size(TEXT_BODY).color(TEXT_DIM));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = |text: &str| egui::Button::new(text).min_size(vec2(0.0, 30.0));
                        if upload {
                            let done = sent.contains(&name);
                            let b = ui.add_enabled(!done && !pending, button(if done { "Upload ✓" } else { "Upload" }));
                            let b = if done { b.on_disabled_hover_text("In the uploads (see UPLOADS)") } else { b.on_disabled_hover_text("After the podcast copy is made") };
                            if b.clicked() {
                                action = Some(TakeAction::Upload(report.cloned().unwrap_or_else(|| report_from_folder(t))));
                            }
                        }
                        if ui.add(button(reveal_label())).clicked() {
                            action = Some(TakeAction::Reveal(t.folder.clone()));
                        }
                        if ui.add(button("Open in Resolve")).on_hover_text("Start DaVinci Resolve with this take's timeline").clicked() {
                            action = Some(TakeAction::Resolve(t.folder.clone()));
                        }
                    });
                });
                for p in problems.iter().take(3) {
                    ui.add(egui::Label::new(RichText::new(format!("⚠ {p}")).size(TEXT_BODY).color(AMBER)).wrap());
                }
                if problems.len() > 3 {
                    ui.label(RichText::new(format!("…and {} more (see Take.txt)", problems.len() - 3)).size(TEXT_BODY).color(AMBER));
                }
                if pending {
                    ui.label(RichText::new(format!("Making the podcast copy{}", waiting::ellipsis(ui.input(|i| i.time) as f32))).size(TEXT_BODY).color(ACCENT));
                }
                if let Some((_, note)) = self.takes.resolve_note.as_ref().filter(|(f, _)| *f == t.folder) {
                    ui.add(egui::Label::new(RichText::new(note).size(TEXT_BODY).color(ACCENT)).wrap());
                }
                if !t.markers.is_empty() {
                    let label = if t.markers.len() == 1 { "1 marker".to_string() } else { format!("{} markers", t.markers.len()) };
                    egui::CollapsingHeader::new(RichText::new(label).size(TEXT_BODY)).id_salt(("take_markers", &t.folder)).default_open(i == 0).show(ui, |ui| {
                        egui::Grid::new(("take_marker_grid", &t.folder)).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                            for (at, m) in &t.markers {
                                ui.label(RichText::new(fmt_clock(Duration::from_secs_f64(*at))).monospace().color(TEXT_DIM));
                                ui.label(RichText::new(m).size(TEXT_BODY));
                                ui.end_row();
                            }
                        });
                    });
                }
            });
            ui.add_space(8.0);
        }
        if let Some(a) = action {
            self.take_action(a, ui.ctx());
        }
    }

    /// Notes a newly saved take for the card; a new recording puts it away.
    pub(super) fn tick_take_card(&mut self, recording: bool, last: Option<&TakeReport>) {
        if recording {
            self.takes.card = None;
            self.takes.resolve_note = None;
            return;
        }
        if let Some(r) = last {
            if self.takes.seen.as_ref() != Some(&r.folder) {
                self.takes.seen = Some(r.folder.clone());
                self.takes.card = Some(r.folder.clone());
                self.takes.resolve_note = None;
                // Record › Takes reads the folder again, so it's there first.
                self.look_for_takes_again();
            }
        }
    }

    /// The "Take saved" card, under the Record button (`anchor`).
    pub(super) fn take_card(&mut self, ctx: &egui::Context, anchor: Rect, last: Option<&TakeReport>) {
        let Some(r) = last.filter(|r| self.takes.card.as_ref() == Some(&r.folder)) else { return };
        let mut problems = r.problems.clone();
        if let Some(note) = backup_incomplete(&self.cfg, &r.folder) {
            if !problems.iter().any(|p| p.contains("BACKUP INCOMPLETE")) {
                problems.push(format!("The backup copy didn't finish (see {})", note.display()));
            }
        }
        let name = r.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
        // The files it recorded: the same figure Record › Takes shows.
        let total: u64 = r.files.iter().map(|f| f.1).sum();
        let upload = self.cfg.upload.ready();
        let sent = self.uploads.jobs.lock_ok().iter().any(|j| j.take == name && matches!(j.state, uploads::JobState::Waiting | uploads::JobState::Uploading | uploads::JobState::Done));
        let note = self.takes.resolve_note.as_ref().filter(|(f, _)| *f == r.folder).map(|(_, n)| n.clone());
        let mut action = None;
        let mut close = false;
        let mut to_takes = false;
        egui::Area::new(egui::Id::unique("take_card")).order(egui::Order::Foreground).fixed_pos(anchor.left_bottom() + vec2(0.0, 8.0)).show(ctx, |ui| {
            egui::Frame::new()
                .fill(RAISED)
                .stroke(Stroke::new(1.0, LINE))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(14))
                .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(120) })
                .show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Take saved").size(TEXT_HEADING).strong().color(Color32::WHITE));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new("✕").frame(false)).on_hover_text("Hide this card (the take stays first in Record › Takes)").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add(egui::Label::new(RichText::new(&name).size(TEXT_SMALL).color(TEXT_DIM)).truncate()).on_hover_text(r.folder.display().to_string());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(take_line(Some(r.duration), total, r.files.len())).size(TEXT_BODY));
                        // The markers stay one click away once the take stops.
                        let n = r.markers.len();
                        if n > 0 {
                            if ui.link(RichText::new(markers_text(n)).size(TEXT_BODY)).on_hover_text("See them in Record › Takes").clicked() {
                                to_takes = true;
                            }
                        } else {
                            ui.label(RichText::new(markers_text(0)).size(TEXT_BODY).color(TEXT_DIM));
                        }
                    });
                    ui.add_space(2.0);
                    if problems.is_empty() {
                        ui.label(RichText::new("✓ All files saved").size(TEXT_BODY).color(GREEN));
                    } else {
                        for p in problems.iter().take(4) {
                            ui.add(egui::Label::new(RichText::new(format!("⚠ {p}")).size(TEXT_BODY).color(AMBER)).wrap());
                        }
                        if problems.len() > 4 {
                            ui.label(RichText::new(format!("…and {} more (see the Log)", problems.len() - 4)).size(TEXT_BODY).color(AMBER));
                        }
                    }
                    if r.podcast_pending {
                        ui.label(RichText::new(format!("Making the podcast copy{}", waiting::ellipsis(ui.input(|i| i.time) as f32))).size(TEXT_BODY).color(ACCENT));
                        ui.ctx().request_repaint_after(Duration::from_millis(250));
                    }
                    if let Some(note) = &note {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(note).size(TEXT_BODY).color(ACCENT)).wrap());
                    }
                    ui.add_space(8.0);
                    let button = |text: &str| egui::Button::new(text).fill(Color32::from_rgb(46, 51, 62)).min_size(vec2(0.0, 28.0));
                    ui.horizontal(|ui| {
                        if ui.add(button("Open in Resolve")).on_hover_text("Start DaVinci Resolve with this take's timeline").clicked() {
                            action = Some(TakeAction::Resolve(r.folder.clone()));
                        }
                        if ui.add(button(reveal_label())).clicked() {
                            action = Some(TakeAction::Reveal(r.folder.clone()));
                        }
                        if upload {
                            let to: Vec<&str> = [(self.cfg.upload.youtube, "YouTube"), (self.cfg.upload.drive, "Google Drive")].iter().filter(|x| x.0).map(|x| x.1).collect();
                            let b = ui.add_enabled(!sent && !r.podcast_pending, button(if sent { "Upload ✓" } else { "Upload" }));
                            let b = if sent { b.on_disabled_hover_text("Sent to the uploads (see UPLOADS)") } else { b.on_disabled_hover_text("After the podcast copy is made") };
                            if b.on_hover_text(format!("Send it to {}", to.join(" and "))).clicked() {
                                action = Some(TakeAction::Upload(r.clone()));
                            }
                        }
                    });
                });
        });
        if close {
            self.takes.card = None;
        }
        if to_takes {
            self.go("record/takes/takes");
        }
        if let Some(a) = action {
            self.take_action(a, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_lengths() {
        assert_eq!(fmt_length(Duration::from_secs(42)), "42s");
        assert_eq!(fmt_length(Duration::from_secs(42 * 60 + 5)), "42m 05s");
        assert_eq!(fmt_length(Duration::from_secs(3600 + 2 * 60 + 59)), "1h 02m");
    }

    #[test]
    fn recent_takes_from_their_notes() {
        let root = std::env::temp_dir().join(format!("cutaway-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (i, name) in ["Show_2026-10-20_19-00-00", "Show_2026-10-21_19-00-00", "Show_2026-10-22_19-00-00"].iter().enumerate() {
            let d = root.join(name);
            std::fs::create_dir_all(d.join("Clips")).unwrap();
            std::fs::create_dir_all(d.join("Proxies")).unwrap();
            std::fs::write(d.join("Program.mp4"), vec![0u8; 1000 * (i + 1)]).unwrap();
            std::fs::write(d.join("Proxies").join("Wide.mp4"), vec![0u8; 200]).unwrap();
            // Not recorded files: a clip, the notes, a timeline.
            std::fs::write(d.join("Clips").join("Clip 01.mp4"), vec![0u8; 500]).unwrap();
            std::fs::write(d.join(format!("{name}.fcpxml")), vec![0u8; 300]).unwrap();
            std::fs::write(d.join("Switches.csv"), vec![0u8; 40]).unwrap();
            let problems = if i == 1 { "\nProblems:\n  Cam 2: 3 frames were dropped\n" } else { "" };
            std::fs::write(d.join("Take.txt"), format!("Cutaway take\nRecorded: 2026-10-2{} 19:00\nLength: 00:42:10\n\nSetup\nFiles:\n  Program.mp4  (1 KB)\n  Proxies/Wide.mp4  (0 KB)\n  Switches.csv  (every scene change, timed from the start of the files)\n  .fcpxml  (open in Final Cut Pro or DaVinci Resolve: a multicam clip and the live edit)\n\nMarkers:\n  00:02:00  News  (on air: Wide)\n  00:10:30  Guest: Sam  (on air: Two shot)\n{problems}\nAll files start at the same instant, so they line up when placed at 00:00 on an editing timeline.\n", i)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        // Not a take: no notes.
        std::fs::create_dir_all(root.join("Other")).unwrap();
        let list = recent_takes(&root, 2);
        assert_eq!(list.len(), 2, "only the newest");
        assert!(list[0].folder.ends_with("Show_2026-10-22_19-00-00"));
        // One size: the files the take recorded, as the card counts them
        // (not the clip, the notes or the timeline).
        assert_eq!((list[0].size, list[0].files), (3000 + 200, 2));
        assert_eq!(list[0].length, Some(Duration::from_secs(42 * 60 + 10)));
        assert_eq!(list[0].recorded.unwrap().format("%Y-%m-%d %H:%M").to_string(), "2026-10-22 19:00");
        assert_eq!(list[0].markers, vec![(120.0, "News".to_string()), (630.0, "Guest: Sam".to_string())]);
        assert!(list[0].problems.is_empty());
        assert_eq!(list[1].problems, vec!["Cam 2: 3 frames were dropped".to_string()]);
        assert!(!list[0].recovered);
        // Rebuilt for uploading: the program where uploads look for it.
        let r = report_from_folder(&list[0]);
        assert!(r.files.iter().any(|f| f.0 == list[0].folder.join("Program.mp4") && f.1 == 3000));
        assert_eq!(r.duration, Duration::from_secs(2530));
        assert!(r.recorded_at.is_some() && !r.podcast_pending);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A take rescued after a crash is listed too, marked, with its files.
    #[test]
    fn rescued_takes_are_listed() {
        let root = std::env::temp_dir().join(format!("cutaway-rescued-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let d = root.join("Show_2026-10-23_19-00-00");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Program.mp4"), vec![0u8; 700]).unwrap();
        std::fs::write(d.join("Mix.wav"), vec![0u8; 300]).unwrap();
        std::fs::write(d.join("Take.txt"), "Cutaway take (recovered)\n\nThis take wasn't stopped normally.\n\nFiles:\n  Program.mp4: recovered 3.1 s\n  Mix.wav: header repaired\n").unwrap();
        let list = recent_takes(&root, 10);
        assert_eq!(list.len(), 1);
        assert!(list[0].recovered && list[0].problems.is_empty());
        assert_eq!((list[0].size, list[0].files), (1000, 2));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The card and the list put a take the same way.
    #[test]
    fn take_lines() {
        assert_eq!(take_line(Some(Duration::from_secs(42 * 60 + 10)), 3_100_000_000, 7), "42m 10s · 3.1 GB · 7 files");
        assert_eq!(take_line(None, 1_000, 1), "1 KB · 1 file");
        assert_eq!(markers_text(0), "no markers");
        assert_eq!(markers_text(1), "◆ 1 marker");
        assert_eq!(markers_text(3), "◆ 3 markers");
        assert_eq!(listed_file("  Program.mp4  (1.2 GB)"), Some("Program.mp4"));
        assert_eq!(listed_file("  Switches.csv  (every scene change, timed from the start of the files)"), None);
        assert_eq!(listed_file("  Program: track 1 is the mix; then each input on its own track (before the fader)"), None);
    }

    /// Open in Resolve says in one line what it did.
    #[test]
    fn resolve_messages() {
        assert!(resolve_message(true, false, true).contains("timeline"));
        assert!(resolve_message(true, true, true).contains("Workspace › Scripts › Cutaway"));
        assert!(resolve_message(false, false, true).starts_with("DaVinci Resolve wasn't found"));
        assert!(resolve_message(false, true, false).contains("folder is copied"));
        for m in [resolve_message(true, false, false), resolve_message(false, false, false)] {
            assert!(!m.contains('\n'));
        }
    }

    #[test]
    fn notices_an_unfinished_backup() {
        let root = std::env::temp_dir().join(format!("cutaway-bk-{}", std::process::id()));
        let main = root.join("main").join("Take_1");
        let backup = root.join("backup");
        std::fs::create_dir_all(backup.join("Take_1")).unwrap();
        let mut cfg = Config::default();
        assert!(backup_incomplete(&cfg, &main).is_none(), "no backup set");
        cfg.output.backup_folder = Some(backup.clone());
        assert!(backup_incomplete(&cfg, &main).is_none());
        std::fs::write(backup.join("Take_1").join("BACKUP INCOMPLETE.txt"), "x").unwrap();
        assert!(backup_incomplete(&cfg, &main).is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
