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
                    self.takes.resolve_note = Some(folder);
                }
                (installed, _) => {
                    self.settings = Some(self.new_settings().open_part_at(Section::Recording, "rec_editing"));
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
