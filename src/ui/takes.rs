//! A take once it's saved: the "Take saved" card under the Record button
//! (length, size, whether everything was saved) and what to do with it next:
//! show it in Finder / Explorer, open it in Resolve, upload it.

use super::*;
use crate::engine::recorder::TakeReport;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Takes {
    /// The take the card is about (None: no card).
    card: Option<PathBuf>,
    /// The last take seen, so the card shows once for each.
    seen: Option<PathBuf>,
    /// The take whose "In Resolve…" how-to is showing.
    resolve_note: Option<PathBuf>,
    /// The Recent takes menu's list, read when it opens.
    recent: Vec<RecentTake>,
    /// The frame the menu was last drawn (a gap means it just opened).
    recent_frame: u64,
    /// Open the menu (for screenshots).
    pub open_menu: bool,
}

/// A take in the Recent takes menu, from its folder and Take.txt.
#[derive(Clone, Debug)]
pub(super) struct RecentTake {
    pub folder: PathBuf,
    pub recorded: Option<chrono::NaiveDateTime>,
    pub length: Option<Duration>,
    pub size: u64,
    pub markers: Vec<(f64, String)>,
    /// Take.txt lists problems.
    pub problems: bool,
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

/// What a take's Take.txt says: when, how long, its markers, any problems.
fn read_note(folder: &Path, note: &str) -> RecentTake {
    let mut t = RecentTake { folder: folder.to_path_buf(), recorded: None, length: None, size: 0, markers: Vec::new(), problems: false };
    let mut section = "";
    for line in note.lines() {
        if let Some(v) = line.strip_prefix("Recorded: ") {
            t.recorded = chrono::NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%d %H:%M").ok();
        } else if let Some(v) = line.strip_prefix("Length: ") {
            t.length = parse_hms(v);
        } else if !line.starts_with(' ') && line.ends_with(':') {
            section = line;
            t.problems |= line == "Problems:";
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
    t
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
            let mut t = read_note(&dir, &note);
            let mut files = Vec::new();
            files_in(&dir, &mut files, 0);
            t.size = files.iter().map(|f| f.1).sum();
            t
        })
        .collect()
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

impl App {
    pub(super) fn take_action(&mut self, action: TakeAction, ctx: &egui::Context) {
        match action {
            TakeAction::Reveal(folder) => crate::logging::reveal_file(&folder),
            TakeAction::Resolve(folder) => match resolve_ready(&folder) {
                (true, true) => {
                    // The script starts at the take noted here.
                    if let Some(dir) = crate::config::default_config_path().parent() {
                        let _ = std::fs::write(crate::engine::sessions::last_take_file(dir), folder.to_string_lossy().as_bytes());
                    }
                    ctx.copy_text(folder.display().to_string());
                    // The how-to: on the card when it's the card's take.
                    if self.takes.card.as_ref() == Some(&folder) {
                        self.takes.resolve_note = Some(folder);
                    } else {
                        self.toast("Folder copied. In Resolve: Workspace → Scripts → Cutaway, then pick this take (it starts there).".into(), false);
                    }
                }
                (installed, _) => {
                    self.settings = Some(self.new_settings().unfold(Section::Recording, "rec_editing"));
                    let msg = if !installed {
                        "To open takes in Resolve: under For editing, press \"Install the Resolve script\" (and keep \"a project from each take\" on)."
                    } else {
                        "This take was saved without the Resolve project. Turn on \"a project from each take\" under For editing for the next ones."
                    };
                    self.toast(msg.to_string(), false);
                }
            },
            TakeAction::Upload(report) => {
                if self.uploads.offer.as_ref().is_some_and(|o| o.folder == report.folder) {
                    self.uploads.offer = None;
                }
                self.upload_take(report);
            }
        }
    }

    /// Recent takes: the last ten in the recording folder, each with the
    /// card's buttons. Read again each time it opens.
    pub(super) fn recent_takes_menu(&mut self, ui: &mut egui::Ui) {
        let frame = ui.ctx().cumulative_frame_nr();
        if frame > self.takes.recent_frame + 1 {
            self.takes.recent = recent_takes(&self.cfg.output.folder, 10);
        }
        self.takes.recent_frame = frame;
        ui.set_min_width(440.0);
        ui.label(RichText::new("RECENT TAKES").size(11.0).strong().color(TEXT_DIM).extra_letter_spacing(1.0));
        if self.takes.recent.is_empty() {
            ui.label(RichText::new(format!("No takes yet in {}", self.cfg.output.folder.display())).color(TEXT_DIM));
        }
        let upload = self.cfg.upload.ready();
        let sent: Vec<String> = self.uploads.jobs.lock_ok().iter().filter(|j| matches!(j.state, uploads::JobState::Waiting | uploads::JobState::Uploading | uploads::JobState::Done)).map(|j| j.take.clone()).collect();
        let mut action = None;
        for t in &self.takes.recent {
            let name = t.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
            ui.separator();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    ui.add(egui::Label::new(RichText::new(&name).size(13.0)).truncate()).on_hover_text(t.folder.display().to_string());
                    let mut what = Vec::new();
                    if let Some(r) = t.recorded {
                        what.push(r.format("%-d %b %H:%M").to_string());
                    }
                    if let Some(l) = t.length {
                        what.push(fmt_length(l));
                    }
                    what.push(crate::util::human_size(t.size));
                    let mut line = RichText::new(what.join(" · ")).size(11.5).color(TEXT_DIM);
                    if t.problems {
                        line = RichText::new(format!("{} · ⚠ see Take.txt", what.join(" · "))).size(11.5).color(AMBER);
                    }
                    ui.label(line);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if upload {
                        let done = sent.contains(&name);
                        if ui.add_enabled(!done, egui::Button::new(if done { "Upload ✓" } else { "Upload…" }).small()).on_disabled_hover_text("In the uploads (see UPLOADS)").clicked() {
                            action = Some(TakeAction::Upload(report_from_folder(t)));
                        }
                    }
                    if ui.small_button("Resolve").on_hover_text("Open in Resolve").clicked() {
                        action = Some(TakeAction::Resolve(t.folder.clone()));
                    }
                    if ui.small_button(if cfg!(target_os = "macos") { "Finder" } else if cfg!(windows) { "Explorer" } else { "Folder" }).on_hover_text(reveal_label()).clicked() {
                        action = Some(TakeAction::Reveal(t.folder.clone()));
                    }
                });
            });
        }
        if let Some(a) = action {
            ui.close();
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
            }
        }
    }

    /// The "Take saved" card, under the Record button (`anchor`).
    pub(super) fn take_card(&mut self, ctx: &egui::Context, anchor: Rect, last: Option<&TakeReport>) {
        let Some(r) = last.filter(|r| self.takes.card.as_ref() == Some(&r.folder)) else { return };
        // Not over the Recent takes menu.
        if self.takes.recent_frame + 1 >= ctx.cumulative_frame_nr() {
            return;
        }
        let mut problems = r.problems.clone();
        if let Some(note) = backup_incomplete(&self.cfg, &r.folder) {
            if !problems.iter().any(|p| p.contains("BACKUP INCOMPLETE")) {
                problems.push(format!("The backup copy didn't finish (see {})", note.display()));
            }
        }
        let name = r.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
        let total: u64 = r.files.iter().map(|f| f.1).sum();
        let (installed, has_take) = resolve_ready(&r.folder);
        let upload = self.cfg.upload.ready();
        let sent = self.uploads.jobs.lock_ok().iter().any(|j| j.take == name && matches!(j.state, uploads::JobState::Waiting | uploads::JobState::Uploading | uploads::JobState::Done));
        let note = self.takes.resolve_note.as_ref() == Some(&r.folder);
        let mut action = None;
        let mut close = false;
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
                        ui.label(RichText::new("Take saved").size(16.0).strong().color(Color32::WHITE));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new("✕").frame(false)).on_hover_text("Put this away").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add(egui::Label::new(RichText::new(&name).size(11.5).color(TEXT_DIM)).truncate()).on_hover_text(r.folder.display().to_string());
                    ui.add_space(4.0);
                    let files = if r.files.len() == 1 { "1 file".to_string() } else { format!("{} files", r.files.len()) };
                    ui.label(RichText::new(format!("{} long · {} · {files}", fmt_length(r.duration), crate::util::human_size(total))).size(13.0));
                    ui.add_space(2.0);
                    if problems.is_empty() {
                        ui.label(RichText::new("✓ All files saved").size(13.0).color(GREEN));
                    } else {
                        for p in problems.iter().take(4) {
                            ui.add(egui::Label::new(RichText::new(format!("⚠ {p}")).size(12.5).color(AMBER)).wrap());
                        }
                        if problems.len() > 4 {
                            ui.label(RichText::new(format!("…and {} more (see the Log)", problems.len() - 4)).size(12.0).color(AMBER));
                        }
                    }
                    if r.podcast_pending {
                        ui.label(RichText::new(format!("Making the podcast copy{}", waiting::ellipsis(ui.input(|i| i.time) as f32))).size(12.0).color(ACCENT));
                        ui.ctx().request_repaint_after(Duration::from_millis(250));
                    }
                    if note {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new("In Resolve: Workspace → Scripts → Cutaway, then pick this folder. It starts there, and its path is copied.").size(12.0).color(ACCENT)).wrap());
                    }
                    ui.add_space(8.0);
                    let button = |text: &str| egui::Button::new(text).fill(Color32::from_rgb(46, 51, 62)).min_size(vec2(0.0, 28.0));
                    ui.horizontal(|ui| {
                        if ui.add(button(reveal_label())).clicked() {
                            action = Some(TakeAction::Reveal(r.folder.clone()));
                        }
                        let tip = if installed && has_take { "Build a Resolve project from this take with the Cutaway script" } else { "Set up the Resolve script (Settings → Recording → For editing)" };
                        if ui.add(button("Open in Resolve")).on_hover_text(tip).clicked() {
                            action = Some(TakeAction::Resolve(r.folder.clone()));
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
            std::fs::write(d.join("Program.mp4"), vec![0u8; 1000 * (i + 1)]).unwrap();
            std::fs::write(d.join("Clips").join("Clip 01.mp4"), vec![0u8; 500]).unwrap();
            let problems = if i == 1 { "\nProblems:\n  Cam 2: 3 frames were dropped\n" } else { "" };
            std::fs::write(d.join("Take.txt"), format!("Cutaway take\nRecorded: 2026-10-2{} 19:00\nLength: 00:42:10\n\nSetup\nFiles:\n  Program.mp4  (1 KB)\n\nMarkers:\n  00:02:00  News  (on air: Wide)\n  00:10:30  Guest: Sam  (on air: Two shot)\n{problems}\nAll files start at the same instant.\n", i)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        // Not a take: no notes.
        std::fs::create_dir_all(root.join("Other")).unwrap();
        let list = recent_takes(&root, 2);
        assert_eq!(list.len(), 2, "only the newest");
        assert!(list[0].folder.ends_with("Show_2026-10-22_19-00-00"));
        let note = std::fs::metadata(list[0].folder.join("Take.txt")).unwrap().len();
        assert_eq!(list[0].size, 3500 + note, "everything in it, Clips too");
        assert_eq!(list[0].length, Some(Duration::from_secs(42 * 60 + 10)));
        assert_eq!(list[0].recorded.unwrap().format("%Y-%m-%d %H:%M").to_string(), "2026-10-22 19:00");
        assert_eq!(list[0].markers, vec![(120.0, "News".to_string()), (630.0, "Guest: Sam".to_string())]);
        assert!(!list[0].problems && list[1].problems);
        // Rebuilt for uploading: the program where uploads look for it.
        let r = report_from_folder(&list[0]);
        assert_eq!(r.files.len(), 3);
        assert!(r.files.iter().any(|f| f.0 == list[0].folder.join("Program.mp4") && f.1 == 3000));
        assert_eq!(r.duration, Duration::from_secs(2530));
        assert!(r.recorded_at.is_some() && !r.podcast_pending);
        let _ = std::fs::remove_dir_all(root);
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
