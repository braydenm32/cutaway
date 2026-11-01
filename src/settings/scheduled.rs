//! Record › Scheduled: the next show set up ahead,
//! with its YouTube event (a page to share before you go live) and going
//! live by itself at the start time.

use super::upload::{client_of, GoogleSignIn};
use super::*;
use crate::config::{Service, StreamDestination, YoutubePrivacy};
use crate::net::google::Auth;
use crate::net::youtube_live::{self, Plan, Scheduled};
use crate::util::LockExt;

/// Talking to YouTube in the background.
pub(super) struct ScheduleJob {
    pub result: Arc<Mutex<Option<JobDone>>>,
}

pub(super) enum JobDone {
    Scheduled(Result<Scheduled, String>),
    Cancelled(Result<(), String>),
}

impl ScheduleJob {
    fn run(ctx: &egui::Context, job: impl FnOnce() -> JobDone + Send + 'static) -> ScheduleJob {
        let result = Arc::new(Mutex::new(None));
        let (r2, ctx2) = (result.clone(), ctx.clone());
        let _ = std::thread::Builder::new().name("youtube live".into()).spawn(move || {
            let done = job();
            *r2.lock_ok() = Some(done);
            ctx2.request_repaint();
        });
        ScheduleJob { result }
    }
}

/// "Friday 30 October, 7:30 PM (in 2 days)", in the show's clock format.
pub(crate) fn describe_start(at: chrono::DateTime<chrono::Local>, twelve: bool) -> String {
    let now = chrono::Local::now();
    let left = at - now;
    let when = if left.num_seconds() < 0 {
        "already past".to_string()
    } else if left.num_minutes() < 60 {
        format!("in {} min", left.num_minutes().max(1))
    } else if left.num_hours() < 48 {
        format!("in {} h {} min", left.num_hours(), left.num_minutes() % 60)
    } else {
        format!("in {} days", left.num_days())
    };
    let day = match (at.date_naive() - now.date_naive()).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        _ => at.format("%A %-d %B").to_string(),
    };
    format!("{day}, {} ({when})", at.format(crate::engine::timers::time_format(twelve, false)))
}

impl SettingsUi {
    pub(super) fn scheduled(&mut self, ui: &mut egui::Ui) {
        self.google_finished();
        self.schedule_finished();
        let start = self.draft.schedule.start_time();
        let state = match (&start, self.draft.schedule.youtube_id.is_empty(), self.draft.schedule.go_live) {
            (None, true, _) => "none".to_string(),
            (Some(t), yt, go) => {
                let mut parts = vec![format!("{} {}", t.format("%a %-d %b"), t.format(crate::engine::timers::time_format(self.draft.ui.clock_12h, false)))];
                if !yt {
                    parts.push("on YouTube".into());
                }
                if go {
                    parts.push("goes live by itself".into());
                }
                parts.join(", ")
            }
            (None, false, _) => "on YouTube".to_string(),
        };
        let open = !self.draft.schedule.youtube_id.is_empty() || self.draft.schedule.go_live;
        let mut schedule = false;
        let mut cancel_event = false;
        let mut allow = false;
        let busy = self.schedule_job.is_some();
        part(ui, "scheduled", "Scheduled stream", &state, open, |ui| {
            hint(ui, "Set the next show up ahead: its YouTube page (to share before you go live) with the title, description and picture, and Cutaway can go live by itself when it's time.");
            let s = &mut self.draft.schedule;
            egui::Grid::new("schedule_grid").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Title");
                ui.add(egui::TextEdit::singleline(&mut s.title).desired_width(380.0).hint_text("what the stream is called"));
                ui.end_row();
                label(ui, "Description");
                ui.add(egui::TextEdit::multiline(&mut s.description).desired_width(380.0).desired_rows(3));
                ui.end_row();
                label(ui, "Starts");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.start).desired_width(160.0).hint_text("2026-10-30 19:30"));
                        if s.start.trim().is_empty() && ui.small_button("Tomorrow at this time").clicked() {
                            s.start = (chrono::Local::now() + chrono::Duration::days(1)).format("%Y-%m-%d %H:00").to_string();
                        }
                    });
                    match s.start_time() {
                        Some(t) => hint(ui, &describe_start(t, self.draft.ui.clock_12h)),
                        None if s.start.trim().is_empty() => hint(ui, "Year-month-day and the time, on this computer's clock."),
                        None => {
                            ui.label(RichText::new("⚠ Write it like 2026-10-30 19:30").color(AMBER));
                        }
                    }
                });
                ui.end_row();
                label(ui, "Picture");
                ui.horizontal(|ui| {
                    let shown = s.thumbnail.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "none".into());
                    ui.label(shown);
                    if ui.button("Choose…").on_hover_text("JPEG or PNG, 1280×720 is best, up to 2 MB").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Pictures", &["jpg", "jpeg", "png"]).pick_file() {
                            s.thumbnail = Some(f);
                        }
                    }
                    if s.thumbnail.is_some() && ui.small_button("Clear").clicked() {
                        s.thumbnail = None;
                    }
                });
                ui.end_row();
                label(ui, "Who can watch");
                ui.horizontal(|ui| {
                    for p in YoutubePrivacy::ALL {
                        ui.radio_value(&mut s.privacy, p, p.label());
                    }
                });
                ui.end_row();
                label(ui, "At the start time");
                ui.checkbox(&mut s.go_live, "go live by itself (if Cutaway is open)");
                ui.end_row();
            });

            ui.add_space(6.0);
            let u = &self.draft.upload;
            let has = |g: &str| u.granted.iter().any(|x| x == g);
            ui.horizontal_wrapped(|ui| {
                if busy {
                    ui.spinner();
                    ui.label(RichText::new("Talking to YouTube…").color(TEXT_DIM));
                } else if u.client_id.trim().is_empty() || u.client_secret.trim().is_empty() {
                    hint(ui, "To make the YouTube page, set up your Google sign-in first (Record › Upload, \"Setting up your Google sign-in\").");
                } else if self.google.is_some() {
                    ui.spinner();
                    ui.label(RichText::new("Finish signing in in your browser…").color(TEXT_DIM));
                } else if !u.signed_in() || !has("live") {
                    if ui.button("Allow Cutaway to schedule YouTube streams").on_hover_text("Signs in with Google again, asking to manage your YouTube live streams").clicked() {
                        allow = true;
                    }
                } else {
                    let s = &self.draft.schedule;
                    let ready = !s.title.trim().is_empty() && s.start_time().is_some();
                    let text = if s.youtube_id.is_empty() { "Schedule on YouTube" } else { "Update on YouTube" };
                    if ui.add_enabled(ready, egui::Button::new(text)).on_disabled_hover_text("Give it a title and a start time first").clicked() {
                        schedule = true;
                    }
                    if !s.youtube_id.is_empty() {
                        let url = youtube_live::watch_url(&s.youtube_id);
                        ui.hyperlink_to(&url, &url);
                        if ui.small_button("Copy link").clicked() {
                            ui.ctx().copy_text(url);
                        }
                        if ui.small_button("Take it off YouTube").clicked() {
                            cancel_event = true;
                        }
                    }
                }
            });
            if !self.draft.schedule.youtube_id.is_empty() {
                hint(ui, "When you go live, YouTube starts this event by itself, and ends it when you stop. Twitch, Facebook and the others go live at the same moment; their titles are set on their own sites.");
            }
        });

        if allow {
            match GoogleSignIn::start(ui.ctx(), &self.draft.upload, true) {
                Ok(g) => self.google = Some(g),
                Err(e) => self.message = Some((format!("Can't sign in: {e}"), true)),
            }
        }
        if schedule {
            let s = &self.draft.schedule;
            let yt_key = self.draft.streaming.destinations.iter().find(|d| d.service == Service::Youtube && d.enabled).map(|d| d.key.clone());
            let plan = Plan {
                title: s.title.trim().to_string(),
                description: s.description.clone(),
                start: youtube_live::to_utc(&s.start).unwrap_or_default(),
                privacy: s.privacy.api().to_string(),
                thumbnail: s.thumbnail.clone(),
                stream_key: yt_key,
                existing: (!s.youtube_id.is_empty()).then(|| s.youtube_id.clone()),
            };
            let mut auth = Auth::new(client_of(&self.draft.upload), self.draft.upload.refresh_token.clone());
            self.schedule_job = Some(ScheduleJob::run(ui.ctx(), move || JobDone::Scheduled(youtube_live::schedule(&mut auth, &plan).map_err(|e| format!("{e:#}")))));
        }
        if cancel_event {
            let id = self.draft.schedule.youtube_id.clone();
            let mut auth = Auth::new(client_of(&self.draft.upload), self.draft.upload.refresh_token.clone());
            self.schedule_job = Some(ScheduleJob::run(ui.ctx(), move || JobDone::Cancelled(youtube_live::cancel(&mut auth, &id).map_err(|e| format!("{e:#}")))));
        }
        if self.schedule_job.is_some() || self.google.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    /// YouTube has answered.
    fn schedule_finished(&mut self) {
        let done = self.schedule_job.as_ref().and_then(|j| j.result.lock_ok().take());
        let Some(done) = done else { return };
        self.schedule_job = None;
        match done {
            JobDone::Scheduled(Ok(s)) => {
                let new = self.draft.schedule.youtube_id != s.id;
                self.draft.schedule.youtube_id = s.id.clone();
                let mut said = if new { "Scheduled on YouTube".to_string() } else { "Updated on YouTube".to_string() };
                if let Some((server, key)) = s.ingest {
                    let dests = &mut self.draft.streaming.destinations;
                    let at = dests.iter().position(|d| d.service == Service::Youtube && d.enabled).or_else(|| dests.iter().position(|d| d.service == Service::Youtube));
                    match at.map(|i| &mut dests[i]) {
                        Some(d) if d.key == key => {}
                        Some(d) => {
                            (d.server, d.key, d.enabled) = (server, key, true);
                            said.push_str(&format!("; {}'s stream key is now the event's", d.name));
                        }
                        None => {
                            dests.push(StreamDestination { name: "YouTube".into(), service: Service::Youtube, server, key, enabled: true, vertical: false });
                            said.push_str("; a YouTube destination was added for it");
                        }
                    }
                }
                if let Some(n) = s.note {
                    said.push_str(&format!(" ({n})"));
                }
                self.message = Some((format!("{said}."), false));
            }
            JobDone::Scheduled(Err(e)) => self.message = Some((format!("Not scheduled: {e}"), true)),
            JobDone::Cancelled(Ok(())) => {
                self.draft.schedule.youtube_id.clear();
                self.message = Some(("Taken off YouTube.".into(), false));
            }
            JobDone::Cancelled(Err(e)) => self.message = Some((format!("Couldn't take it off YouTube: {e}"), true)),
        }
    }
}
