//! Uploading takes to YouTube and Google Drive (Settings → Upload): a queue
//! worked through in the background, one upload at a time, shown in the
//! UPLOADS panel. A take is offered once it's saved (or sent straight away
//! when that's set). The queue is kept in uploads.json beside the settings
//! (no sign-in tokens in it), so uploads cut short by quitting carry on
//! where Google got to at the next start, when you say so.

use crate::util::LockExt;
use super::*;
use crate::config::{DriveFiles, UploadConfig};
use crate::engine::recorder::TakeReport;
use crate::net::google::{self, Auth, Client, Endpoints};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum JobState {
    Waiting,
    Uploading,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Work {
    Youtube { file: PathBuf, title: String, description: String, privacy: String },
    /// Files, each with the folders it goes in (inside the take's folder).
    Drive { base: String, take: String, files: Vec<(PathBuf, Vec<String>)> },
}

#[derive(Clone)]
pub struct Job {
    /// Jobs are found by this, not by place: the list changes while one
    /// is uploading (finished ones cleared, failed ones retried).
    id: u64,
    pub label: String,
    pub take: String,
    pub state: JobState,
    pub sent: u64,
    pub total: u64,
    /// Where it ended up.
    pub link: Option<String>,
    cancel: Arc<AtomicBool>,
    work: Work,
    client: Client,
    refresh: String,
    /// The file being sent and Google's session for it, to carry on.
    session: Option<(PathBuf, String)>,
    /// Files already sent (Drive).
    done: Vec<PathBuf>,
}

/// A job as kept in uploads.json: what's needed to start it again, never
/// the sign-in (that's in the Keychain).
#[derive(Serialize, Deserialize)]
struct SavedJob {
    label: String,
    take: String,
    state: JobState,
    sent: u64,
    total: u64,
    work: Work,
    #[serde(default)]
    session: Option<(PathBuf, String)>,
    #[serde(default)]
    done: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Default)]
struct SavedQueue {
    jobs: Vec<SavedJob>,
}

/// What a running job tells the queue, to be kept.
enum Note {
    Session(PathBuf, String),
    FileDone(PathBuf),
}

/// Jobs that haven't finished, as kept.
fn unfinished(jobs: &[Job]) -> Vec<SavedJob> {
    jobs.iter()
        .filter(|j| matches!(j.state, JobState::Waiting | JobState::Uploading | JobState::Failed(_)))
        .map(|j| SavedJob { label: j.label.clone(), take: j.take.clone(), state: j.state.clone(), sent: j.sent, total: j.total, work: j.work.clone(), session: j.session.clone(), done: j.done.clone() })
        .collect()
}

/// Keep the unfinished jobs (and those waiting for Carry on) in `file`;
/// none left, no file.
fn save_queue(file: &Path, jobs: &[Job], resume: &[Job]) {
    static WRITING: Mutex<()> = Mutex::new(());
    let _w = WRITING.lock_ok();
    let mut q = SavedQueue { jobs: unfinished(resume) };
    q.jobs.extend(unfinished(jobs));
    if q.jobs.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(&q) else { return };
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The jobs kept in `file` (to offer: they start again only when asked).
fn load_queue(file: &Path) -> Vec<Job> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let Ok(q) = serde_json::from_str::<SavedQueue>(&text) else {
        crate::logging::note(&format!("uploads: couldn't read {}", file.display()));
        return Vec::new();
    };
    q.jobs
        .into_iter()
        .map(|j| Job {
            id: new_id(),
            label: j.label,
            take: j.take,
            state: match j.state {
                JobState::Failed(e) => JobState::Failed(e),
                _ => JobState::Waiting,
            },
            sent: j.sent,
            total: j.total,
            link: None,
            cancel: Arc::new(AtomicBool::new(false)),
            work: j.work,
            client: Client { id: String::new(), secret: String::new(), ep: Endpoints::default() },
            refresh: String::new(),
            session: j.session,
            done: j.done,
        })
        .collect()
}

#[derive(Default)]
pub struct Uploads {
    pub jobs: Arc<Mutex<Vec<Job>>>,
    /// Unfinished at the last quit, waiting for Carry on / Forget them.
    resume: Arc<Mutex<Vec<Job>>>,
    /// Where the queue is kept (None: not kept, as in the demo).
    file: Option<PathBuf>,
    running: Arc<AtomicBool>,
    /// The last take seen, so each is offered once.
    seen: Option<PathBuf>,
    /// A saved take waiting for its Upload button.
    pub offer: Option<TakeReport>,
}

fn client_of(u: &UploadConfig) -> Client {
    Client { id: u.client_id.trim().to_string(), secret: u.client_secret.trim().to_string(), ep: Endpoints::default() }
}

/// The take's program file (or another of its files by stem).
fn file_named(report: &TakeReport, stem: &str) -> Option<PathBuf> {
    report.files.iter().map(|f| f.0.clone()).find(|p| p.parent() == Some(report.folder.as_path()) && p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(stem)))
}

/// Every file under `dir`, with the folders between.
fn all_files(dir: &Path, path: Vec<String>, out: &mut Vec<(PathBuf, Vec<String>)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            let mut sub = path.clone();
            sub.push(e.file_name().to_string_lossy().to_string());
            all_files(&p, sub, out);
        } else if !e.file_name().to_string_lossy().starts_with('.') {
            out.push((p, path.clone()));
        }
    }
}

fn new_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The jobs for a take, as the settings say.
fn jobs_for(cfg: &Config, report: &TakeReport) -> (Vec<Job>, Vec<String>) {
    let u = &cfg.upload;
    let mut jobs = Vec::new();
    let mut problems = Vec::new();
    let take = report.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
    let job = |label: String, work: Work| Job {
        id: new_id(),
        label,
        take: take.clone(),
        state: JobState::Waiting,
        sent: 0,
        total: 0,
        link: None,
        cancel: Arc::new(AtomicBool::new(false)),
        work,
        client: client_of(u),
        refresh: u.refresh_token.clone(),
        session: None,
        done: Vec::new(),
    };
    let granted = |g: &str| u.granted.iter().any(|x| x == g);
    if u.youtube {
        match file_named(report, "Program") {
            Some(file) if granted("youtube") => {
                let show = [cfg.podcast.show.trim(), cfg.ui.profile.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("").to_string();
                // The day it was recorded (a show past midnight, or sent the next day).
                let date = report.recorded_at.unwrap_or_else(chrono::Local::now).format("%-d %B %Y").to_string();
                let title = if !u.youtube_title.trim().is_empty() {
                    u.youtube_title.trim().replace("{date}", &date).replace("{show}", &show)
                } else if !show.is_empty() {
                    format!("{show} – {date}")
                } else {
                    take.clone()
                };
                let mut description = u.youtube_description.trim().replace("{date}", &date).replace("{show}", &show);
                if u.youtube_chapters {
                    if let Some(ch) = crate::engine::export::youtube_chapters(&report.markers, report.duration.as_secs_f64()) {
                        if !description.is_empty() {
                            description.push_str("\n\n");
                        }
                        description.push_str(&ch);
                    }
                }
                jobs.push(job("YouTube".into(), Work::Youtube { file, title, description, privacy: u.youtube_privacy.api().to_string() }));
            }
            Some(_) => problems.push("sign in again to allow YouTube (Settings → Upload)".into()),
            None => problems.push("the take has no program file for YouTube".into()),
        }
    }
    if u.drive {
        if !granted("drive") {
            problems.push("sign in again to allow Google Drive (Settings → Upload)".into());
        } else {
            let files: Vec<(PathBuf, Vec<String>)> = match u.drive_files {
                DriveFiles::Program => file_named(report, "Program").into_iter().map(|f| (f, Vec::new())).collect(),
                DriveFiles::ProgramAndMix => ["Program", "Program_Vertical", "Mix"].iter().filter_map(|s| file_named(report, s)).chain(report.podcast.clone().filter(|p| p.exists())).map(|f| (f, Vec::new())).collect(),
                DriveFiles::Everything => {
                    let mut v = Vec::new();
                    all_files(&report.folder, Vec::new(), &mut v);
                    v
                }
            };
            if files.is_empty() {
                problems.push("nothing in the take to send to Google Drive".into());
            } else {
                let base = if u.drive_folder.trim().is_empty() { "Cutaway".to_string() } else { u.drive_folder.trim().to_string() };
                jobs.push(job("Google Drive".into(), Work::Drive { base, take: take.clone(), files }));
            }
        }
    }
    (jobs, problems)
}

/// One job, start to finish (or from where it got to).
fn run(job: &Job, update: &dyn Fn(u64, u64), note: &dyn Fn(Note)) -> Result<String, String> {
    let mut auth = Auth::new(job.client.clone(), job.refresh.clone());
    let cancel = &job.cancel;
    let mut last = Instant::now();
    match &job.work {
        Work::Youtube { file, title, description, privacy } => {
            let meta = google::youtube_metadata(title, description, privacy);
            let mut progress = |s: u64, t: u64| {
                if last.elapsed() > Duration::from_millis(300) || s == t {
                    last = Instant::now();
                    update(s, t);
                }
            };
            let earlier = job.session.as_ref().filter(|s| &s.0 == file).map(|s| s.1.as_str());
            let v = google::resumable_from(&mut auth, &job.client.ep.youtube_upload, &meta, file, google::mime(file), google::CHUNK, &mut progress, cancel, earlier, &mut |url| note(Note::Session(file.clone(), url.to_string())))
                .map_err(|e| format!("{e:#}"))?;
            let id = v["id"].as_str().ok_or("YouTube didn't say where the video is")?;
            Ok(format!("https://youtu.be/{id}"))
        }
        Work::Drive { base, take, files } => {
            let total: u64 = files.iter().filter_map(|(f, _)| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
            let base_id = google::drive_folder(&mut auth, base, None).map_err(|e| format!("{e:#}"))?;
            let take_id = google::drive_folder(&mut auth, take, Some(&base_id)).map_err(|e| format!("{e:#}"))?;
            let mut folders: HashMap<Vec<String>, String> = HashMap::new();
            folders.insert(Vec::new(), take_id.clone());
            let mut done: u64 = 0;
            for (file, path) in files {
                // Sent before Cutaway last quit.
                if job.done.contains(file) {
                    done += std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                    continue;
                }
                // Its folder, made as needed.
                for depth in 1..=path.len() {
                    let key = path[..depth].to_vec();
                    if !folders.contains_key(&key) {
                        let parent = folders[&path[..depth - 1].to_vec()].clone();
                        let id = google::drive_folder(&mut auth, &path[depth - 1], Some(&parent)).map_err(|e| format!("{e:#}"))?;
                        folders.insert(key, id);
                    }
                }
                let parent = folders[path].clone();
                let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
                let meta = serde_json::json!({ "name": name, "parents": [parent] });
                let mut progress = |s: u64, _t: u64| {
                    if last.elapsed() > Duration::from_millis(300) {
                        last = Instant::now();
                        update(done + s, total);
                    }
                };
                let earlier = job.session.as_ref().filter(|s| &s.0 == file).map(|s| s.1.as_str());
                google::resumable_from(&mut auth, &job.client.ep.drive_upload, &meta, file, google::mime(file), google::CHUNK, &mut progress, cancel, earlier, &mut |url| note(Note::Session(file.clone(), url.to_string())))
                    .map_err(|e| format!("{name}: {e:#}"))?;
                note(Note::FileDone(file.clone()));
                done += std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                update(done, total);
            }
            Ok(format!("https://drive.google.com/drive/folders/{take_id}"))
        }
    }
}

impl Uploads {
    /// Keep the queue in `file`, and offer what was left in it.
    pub fn load(&mut self, file: PathBuf) {
        *self.resume.lock_ok() = load_queue(&file);
        self.file = Some(file);
    }

    /// Write the queue down (after it changes).
    fn persist(&self) {
        if let Some(f) = &self.file {
            let jobs = self.jobs.lock_ok().clone();
            let resume = self.resume.lock_ok().clone();
            save_queue(f, &jobs, &resume);
        }
    }

    /// How many uploads are waiting for Carry on.
    pub fn unfinished(&self) -> usize {
        self.resume.lock_ok().len()
    }

    /// Queue jobs and make sure they're being worked on.
    fn add(&mut self, jobs: Vec<Job>, status: Option<Arc<Mutex<crate::engine::Status>>>, waker: Waker) {
        self.jobs.lock_ok().extend(jobs);
        self.persist();
        self.work(status, waker);
    }

    /// Work through the waiting jobs, one at a time, unless that's already
    /// happening.
    fn work(&mut self, status: Option<Arc<Mutex<crate::engine::Status>>>, waker: Waker) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let (list, running) = (self.jobs.clone(), self.running.clone());
        let (resume, file) = (self.resume.clone(), self.file.clone());
        let persist = move |l: &[Job]| {
            if let Some(f) = &file {
                let r = resume.lock_ok().clone();
                save_queue(f, l, &r);
            }
        };
        let spawned = std::thread::Builder::new().name("uploads".into()).spawn(move || {
            loop {
                let next = {
                    let mut l = list.lock_ok();
                    let next = l.iter_mut().find(|j| j.state == JobState::Waiting).map(|j| {
                        j.state = JobState::Uploading;
                        j.clone()
                    });
                    if next.is_none() {
                        // Done, said under the list's lock: a job added
                        // from now on starts a new worker.
                        running.store(false, Ordering::SeqCst);
                    } else {
                        persist(&l);
                    }
                    next
                };
                let Some(job) = next else { break };
                waker();
                let update = |s: u64, t: u64| {
                    if let Some(j) = list.lock_ok().iter_mut().find(|j| j.id == job.id) {
                        (j.sent, j.total) = (s, t);
                    }
                    waker();
                };
                let note = |n: Note| {
                    let mut l = list.lock_ok();
                    if let Some(j) = l.iter_mut().find(|j| j.id == job.id) {
                        match n {
                            Note::Session(file, url) => j.session = Some((file, url)),
                            Note::FileDone(file) => {
                                j.done.push(file);
                                j.session = None;
                            }
                        }
                    }
                    persist(&l);
                };
                let result = run(&job, &update, &note);
                let (level, text) = match &result {
                    Ok(link) => (crate::engine::LogLevel::Info, format!("Uploaded {} to {}: {link}", job.take, job.label)),
                    Err(_) if job.cancel.load(Ordering::Relaxed) => (crate::engine::LogLevel::Info, format!("Upload of {} to {} cancelled", job.take, job.label)),
                    Err(e) => (crate::engine::LogLevel::Warning, format!("Upload of {} to {} failed: {e}", job.take, job.label)),
                };
                {
                    let mut l = list.lock_ok();
                    if let Some(j) = l.iter_mut().find(|j| j.id == job.id) {
                        j.state = match result {
                            Ok(link) => {
                                j.link = Some(link);
                                JobState::Done
                            }
                            Err(_) if job.cancel.load(Ordering::Relaxed) => JobState::Cancelled,
                            Err(e) => JobState::Failed(e),
                        };
                    }
                    persist(&l);
                }
                if let Some(st) = &status {
                    st.lock_ok().push(level, text);
                }
                waker();
            }
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

impl App {
    /// A take was just saved: offer it, or send it.
    pub(super) fn tick_uploads(&mut self) {
        let Some(engine) = &self.engine else { return };
        let report = {
            let st = engine.status.lock_ok();
            match &st.last_take {
                // (Once its podcast is made, so that goes up too.)
                Some(r) if self.uploads.seen.as_ref() != Some(&r.folder) && !r.podcast_pending => r.clone(),
                _ => return,
            }
        };
        self.uploads.seen = Some(report.folder.clone());
        if !self.cfg.upload.ready() {
            return;
        }
        if self.cfg.upload.automatic {
            self.upload_take(report);
        } else {
            self.uploads.offer = Some(report);
        }
    }

    pub(super) fn upload_take(&mut self, report: TakeReport) {
        let (jobs, problems) = jobs_for(&self.cfg, &report);
        for p in problems {
            self.toast(format!("Upload: {p}"), true);
        }
        if !jobs.is_empty() {
            let status = self.engine.as_ref().map(|e| e.status.clone());
            self.uploads.add(jobs, status, self.waker.clone());
        }
    }

    /// UPLOADS: what's going up, and a saved take to send.
    pub(super) fn uploads_panel(&mut self, ui: &mut egui::Ui) {
        let jobs = self.uploads.jobs.lock_ok().clone();
        let offer = self.uploads.offer.clone();
        let left = self.uploads.unfinished();
        if jobs.is_empty() && offer.is_none() && left == 0 {
            return;
        }
        ui.add_space(4.0);
        if !self.section_heading(ui, "uploads", "UPLOADS", Section::Upload) {
            return;
        }
        if left > 0 {
            let (mut go, mut forget) = (false, false);
            let text = if left == 1 { "1 upload didn't finish".to_string() } else { format!("{left} uploads didn't finish") };
            let takes: Vec<String> = self.uploads.resume.lock_ok().iter().map(|j| format!("{} to {}", j.take, j.label)).collect();
            ui.label(RichText::new(text).size(12.5).color(AMBER)).on_hover_text(takes.join("\n"));
            let mut names: Vec<String> = self.uploads.resume.lock_ok().iter().map(|j| j.take.clone()).collect();
            names.dedup();
            ui.add(egui::Label::new(RichText::new(names.join(", ")).size(11.5).color(TEXT_DIM)).truncate());
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Carry on").strong()).on_hover_text("Pick them up where Google got to").clicked() {
                    go = true;
                }
                if ui.small_button("Forget them").clicked() {
                    forget = true;
                }
            });
            if go {
                if !self.cfg.upload.ready() {
                    self.toast("Sign in to Google again in Settings → Upload, then Carry on.".into(), true);
                } else {
                    let mut back = std::mem::take(&mut *self.uploads.resume.lock_ok());
                    for j in &mut back {
                        j.state = JobState::Waiting;
                        j.refresh = self.cfg.upload.refresh_token.clone();
                        j.client = client_of(&self.cfg.upload);
                    }
                    let status = self.engine.as_ref().map(|e| e.status.clone());
                    self.uploads.add(back, status, self.waker.clone());
                }
            } else if forget {
                self.uploads.resume.lock_ok().clear();
                self.uploads.persist();
            }
            ui.add_space(4.0);
        }
        if let Some(r) = &offer {
            let name = r.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
            let mut go = false;
            let mut skip = false;
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(&name).size(12.0)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Not this one").clicked() {
                        skip = true;
                    }
                    let to: Vec<&str> = [(self.cfg.upload.youtube, "YouTube"), (self.cfg.upload.drive, "Drive")].iter().filter(|x| x.0).map(|x| x.1).collect();
                    if ui.button(RichText::new("Upload").strong()).on_hover_text(format!("Send it to {}", to.join(" and "))).clicked() {
                        go = true;
                    }
                });
            });
            if go {
                self.uploads.offer = None;
                self.upload_take(r.clone());
            } else if skip {
                self.uploads.offer = None;
            }
        }
        let mut clear_done = false;
        let mut retry = None;
        for j in &jobs {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&j.label).size(12.0).strong());
                ui.add(egui::Label::new(RichText::new(&j.take).size(11.5).color(TEXT_DIM)).truncate());
            });
            ui.horizontal(|ui| match &j.state {
                JobState::Waiting => {
                    ui.label(RichText::new("waiting").size(11.5).color(TEXT_DIM));
                    if ui.small_button("✕").on_hover_text("Don't upload it").clicked() {
                        j.cancel.store(true, Ordering::Relaxed);
                        if let Some(x) = self.uploads.jobs.lock_ok().iter_mut().find(|x| x.id == j.id && x.state == JobState::Waiting) {
                            x.state = JobState::Cancelled;
                        }
                        self.uploads.persist();
                    }
                }
                JobState::Uploading => {
                    let f = if j.total > 0 { j.sent as f32 / j.total as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(f).desired_width(ui.available_width() - 40.0).text(format!("{:.0}% of {}", f * 100.0, crate::util::human_size(j.total))));
                    if ui.small_button("✕").on_hover_text("Stop this upload").clicked() {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                }
                JobState::Done => {
                    ui.label(RichText::new("✓ uploaded").size(11.5).color(GREEN));
                    if let Some(link) = &j.link {
                        if ui.small_button("Open").on_hover_text(link).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(link));
                        }
                    }
                    clear_done = true;
                }
                JobState::Failed(e) => {
                    ui.add(egui::Label::new(RichText::new(format!("⚠ {e}")).size(11.5).color(AMBER)).wrap());
                    if ui.small_button("Retry").clicked() {
                        retry = Some(j.id);
                    }
                }
                JobState::Cancelled => {
                    ui.label(RichText::new("cancelled").size(11.5).color(TEXT_DIM));
                    clear_done = true;
                }
            });
        }
        if jobs.iter().any(|j| j.state == JobState::Uploading) {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        if let Some(id) = retry {
            // Again, in its place, with the sign-in as it is now (and
            // where Google got to, when it can).
            if let Some(j) = self.uploads.jobs.lock_ok().iter_mut().find(|j| j.id == id) {
                j.state = JobState::Waiting;
                (j.sent, j.total) = (0, 0);
                j.cancel = Arc::new(AtomicBool::new(false));
                j.refresh = self.cfg.upload.refresh_token.clone();
                j.client = client_of(&self.cfg.upload);
            }
            self.uploads.persist();
            let status = self.engine.as_ref().map(|e| e.status.clone());
            self.uploads.work(status, self.waker.clone());
        }
        if clear_done && ui.small_button("Clear finished").clicked() {
            self.uploads.jobs.lock_ok().retain(|j| !matches!(j.state, JobState::Done | JobState::Cancelled));
        }
        ui.add_space(4.0);
        ui.separator();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(dir: &Path) -> TakeReport {
        std::fs::create_dir_all(dir.join("Guests")).unwrap();
        let mut files = Vec::new();
        for (name, n) in [("Program.mp4", 700_000usize), ("Program_Vertical.mp4", 300_000), ("Mix.wav", 200_000), ("ISO_01_Cam_1.mp4", 100_000)] {
            let p = dir.join(name);
            std::fs::write(&p, vec![name.len() as u8; n]).unwrap();
            files.push((p, n as u64));
        }
        std::fs::write(dir.join("Guests").join("Guest_1_backup.webm"), b"guest").unwrap();
        TakeReport { folder: dir.to_path_buf(), files, problems: Vec::new(), duration: Duration::from_secs(600), markers: vec![(0.0, "Welcome".into()), (120.0, "News".into()), (300.0, "Guest".into())], ..Default::default() }
    }

    #[test]
    fn takes_go_where_the_settings_say() {
        let dir = std::env::temp_dir().join(format!("cutaway-uptake-{}", std::process::id())).join("Take_2026-10-20_19-00-00");
        let mut report = take(&dir);
        // Recorded late in the evening, sent the next day: its own date.
        use chrono::TimeZone;
        report.recorded_at = chrono::Local.with_ymd_and_hms(2026, 10, 20, 23, 50, 0).single();
        let mut cfg = Config::default();
        cfg.podcast.show = "The Show".into();
        cfg.upload = UploadConfig { refresh_token: "r1".into(), granted: vec!["youtube".into()], youtube: true, drive: true, youtube_description: "About {show}.".into(), ..Default::default() };
        let (jobs, problems) = jobs_for(&cfg, &report);
        assert_eq!(jobs.len(), 1, "YouTube only: Drive wasn't allowed at sign-in");
        assert!(problems[0].contains("Google Drive"));
        let Work::Youtube { title, description, file, privacy } = &jobs[0].work else { panic!() };
        assert_eq!(title, "The Show – 20 October 2026");
        assert_eq!(description, "About The Show.\n\n0:00 Welcome\n2:00 News\n5:00 Guest");
        assert!(file.ends_with("Program.mp4"));
        assert_eq!(privacy, "private");

        cfg.upload.granted.push("drive".into());
        for (what, want) in [(DriveFiles::Program, 1), (DriveFiles::ProgramAndMix, 3), (DriveFiles::Everything, 5)] {
            cfg.upload.drive_files = what;
            let (jobs, _) = jobs_for(&cfg, &report);
            let Work::Drive { files, take, base } = &jobs[1].work else { panic!() };
            assert_eq!((files.len(), take.as_str(), base.as_str()), (want, "Take_2026-10-20_19-00-00", "Cutaway"), "{what:?}");
            if what == DriveFiles::ProgramAndMix {
                // With the podcast, once it's made.
                let podcast = dir.join("Take (Podcast).m4a");
                std::fs::write(&podcast, b"m4a").unwrap();
                let r = TakeReport { podcast: Some(podcast.clone()), ..report.clone() };
                let (jobs, _) = jobs_for(&cfg, &r);
                let Work::Drive { files, .. } = &jobs[1].work else { panic!() };
                assert!(files.len() == 4 && files[3].0 == podcast);
                std::fs::remove_file(&podcast).unwrap();
            }
            if what == DriveFiles::Everything {
                assert!(files.iter().any(|(f, path)| f.ends_with("Guest_1_backup.webm") && path == &vec!["Guests".to_string()]));
            }
        }
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// The queue outlives a quit, without the sign-in, and only what's left.
    #[test]
    fn unfinished_uploads_are_kept() {
        let dir = std::env::temp_dir().join(format!("cutaway-upkeep-{}", std::process::id())).join("Take_3");
        let report = take(&dir);
        let mut cfg = Config::default();
        cfg.upload = UploadConfig { client_id: "id".into(), client_secret: "shh-secret".into(), refresh_token: "refresh-secret".into(), granted: vec!["youtube".into(), "drive".into()], youtube: true, drive: true, ..Default::default() };
        let (mut jobs, _) = jobs_for(&cfg, &report);
        jobs[0].session = Some((dir.join("Program.mp4"), "https://upload.example/session/1".into()));
        jobs[1].state = JobState::Failed("offline".into());
        jobs[1].done = vec![dir.join("Program.mp4")];
        let finished = Job { id: new_id(), state: JobState::Done, ..jobs[0].clone() };
        jobs.push(finished);
        let file = dir.parent().unwrap().join("uploads.json");
        save_queue(&file, &jobs, &[]);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("secret"), "no sign-in in the file: {text}");
        let back = load_queue(&file);
        assert_eq!(back.len(), 2, "not the finished one");
        assert_eq!(back[0].work, jobs[0].work);
        assert_eq!(back[0].state, JobState::Waiting);
        assert_eq!(back[0].session, jobs[0].session);
        assert_eq!(back[1].state, JobState::Failed("offline".into()));
        assert_eq!(back[1].done, jobs[1].done);
        assert!(back[0].refresh.is_empty() && back[0].client.secret.is_empty());
        // Waiting to be carried on, they're still kept; nothing left, no file.
        let mut up = Uploads::default();
        up.load(file.clone());
        assert_eq!(up.unfinished(), 2);
        up.persist();
        assert_eq!(load_queue(&file).len(), 2);
        up.resume.lock_ok().clear();
        up.persist();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// Carried on after a restart: the Drive files already sent aren't sent again.
    #[test]
    fn carries_on_without_sending_files_twice() {
        let g = crate::net::google::tests::fake_google(0);
        let dir = std::env::temp_dir().join(format!("cutaway-upcarry-{}", std::process::id())).join("Take_4");
        let report = take(&dir);
        let mut cfg = Config::default();
        cfg.upload = UploadConfig { refresh_token: "r1".into(), granted: vec!["drive".into()], drive: true, drive_files: DriveFiles::Everything, ..Default::default() };
        let (mut jobs, _) = jobs_for(&cfg, &report);
        let mut job = jobs.remove(0);
        job.client = crate::net::google::tests::client(g.port);
        let Work::Drive { files, .. } = &job.work else { panic!() };
        job.done = files.iter().take(2).map(|f| f.0.clone()).collect();
        let notes = Mutex::new(0);
        run(&job, &|_, _| {}, &|n| if let Note::FileDone(_) = n { *notes.lock_ok() += 1 }).unwrap();
        assert_eq!(g.files.lock_ok().len(), 3, "the other three");
        assert_eq!(*notes.lock_ok(), 3);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn clearing_finished_ones_while_one_uploads() {
        let g = crate::net::google::tests::fake_google(0);
        let dir = std::env::temp_dir().join(format!("cutaway-upclear-{}", std::process::id())).join("Take_2");
        let report = take(&dir);
        let mut cfg = Config::default();
        cfg.upload = UploadConfig { refresh_token: "r1".into(), granted: vec!["drive".into()], drive: true, ..Default::default() };
        let (mut jobs, _) = jobs_for(&cfg, &report);
        let mut job = jobs.remove(0);
        job.client = crate::net::google::tests::client(g.port);
        let finished = Job { id: new_id(), state: JobState::Done, ..job.clone() };
        let failed = Job { id: new_id(), state: JobState::Failed("offline".into()), ..job.clone() };
        let (job_id, failed_id) = (job.id, failed.id);
        let mut up = Uploads::default();
        up.jobs.lock_ok().extend([finished, failed]);
        // As soon as it starts, "Clear finished" (the list shifts under it).
        let list = up.jobs.clone();
        let cleared = Arc::new(AtomicBool::new(false));
        let c2 = cleared.clone();
        let waker: Waker = Arc::new(move || {
            if !c2.swap(true, Ordering::SeqCst) {
                list.lock_ok().retain(|j| !matches!(j.state, JobState::Done | JobState::Cancelled));
            }
        });
        up.add(vec![job], None, waker);
        let t0 = Instant::now();
        while up.running.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let l = up.jobs.lock_ok();
        assert_eq!(l.len(), 2);
        assert_eq!(l.iter().find(|j| j.id == job_id).unwrap().state, JobState::Done, "the upload finished in its own entry");
        assert_eq!(l.iter().find(|j| j.id == failed_id).unwrap().state, JobState::Failed("offline".into()), "the others untouched");
        drop(l);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn uploads_a_take_to_drive() {
        let g = crate::net::google::tests::fake_google(0);
        let dir = std::env::temp_dir().join(format!("cutaway-updrive-{}", std::process::id())).join("Take_1");
        let report = take(&dir);
        let mut cfg = Config::default();
        cfg.upload = UploadConfig { refresh_token: "r1".into(), granted: vec!["drive".into()], drive: true, drive_files: DriveFiles::Everything, ..Default::default() };
        let (mut jobs, _) = jobs_for(&cfg, &report);
        let mut job = jobs.remove(0);
        job.client = crate::net::google::tests::client(g.port);
        let seen = Mutex::new(Vec::new());
        let link = run(&job, &|s, t| seen.lock_ok().push((s, t)), &|_| {}).unwrap();
        assert_eq!(link, "https://drive.google.com/drive/folders/folder2", "the take's folder, inside Cutaway's");
        // Every file went up whole, into the right folder.
        let files = g.files.lock_ok();
        let meta = g.meta.lock_ok();
        assert_eq!(files.len(), 5);
        let sizes: Vec<usize> = { let mut v: Vec<usize> = files.values().map(|f| f.len()).collect(); v.sort(); v };
        assert_eq!(sizes, vec![5, 100_000, 200_000, 300_000, 700_000]);
        assert!(meta.values().any(|m| m.contains("Guest_1_backup.webm") && m.contains("folder3")), "the Guests folder inside the take's");
        assert!(meta.values().any(|m| m.contains("Program.mp4") && m.contains("folder2")));
        let last = *seen.lock_ok().last().unwrap();
        assert_eq!(last.0, last.1, "all of it");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
