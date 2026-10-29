//! Settings → Upload: sending takes to YouTube and Google Drive after the
//! show, with your own Google sign-in.

use crate::util::LockExt;
use super::*;
use crate::config::{DriveFiles, UploadConfig, YoutubePrivacy};
use crate::net::google::{self, Client, Endpoints};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

impl SettingsUi {
    /// A Google sign-in that has finished (from Upload or Streaming).
    pub(super) fn google_finished(&mut self) {
        let finished = self.google.as_ref().and_then(|g| g.result.lock_ok().take());
        if let Some(r) = finished {
            self.google = None;
            match r {
                Ok((refresh, account, granted)) => {
                    let u = &mut self.draft.upload;
                    (u.refresh_token, u.account, u.granted) = (refresh, account.clone(), granted);
                    self.message = Some((format!("Signed in as {account}. Apply to keep it."), false));
                }
                Err(e) => self.message = Some((format!("Not signed in: {e}"), true)),
            }
        }
    }

    pub(super) fn upload(&mut self, ui: &mut egui::Ui) {
        title(ui, "Upload after the show", "Send each take to YouTube and Google Drive once it's saved, with your own Google sign-in. Uploads run in the background and resume if the connection drops.");
        self.google_finished();
        egui::CollapsingHeader::new(RichText::new("Setting up your Google sign-in (once)").strong()).default_open(self.draft.upload.client_id.trim().is_empty()).show(ui, |ui| {
            for (i, step) in [
                "In the Google Cloud console (console.cloud.google.com), make a project.",
                "APIs & Services → Library: turn on YouTube Data API v3 and/or Google Drive API.",
                "OAuth consent screen: choose External and add your Google account as a test user. While the app is \"Testing\", Google signs Cutaway out after 7 days; publishing it (no review is needed to use it yourself) stops that, with an \"unverified app\" note when you sign in.",
                "Credentials → Create credentials → OAuth client ID → Desktop app. Copy its client ID and secret below.",
            ]
            .iter()
            .enumerate()
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{}.", i + 1)).strong());
                    ui.label(RichText::new(*step).size(TEXT_STATE));
                });
            }
            hint(ui, "YouTube keeps videos uploaded through a project Google hasn't audited private; make them public yourself in YouTube Studio (or apply for Google's audit). Drive uploads have no such limit. Cutaway only sees the Drive files it makes.");
        });
        ui.add_space(8.0);
        let signing_in = self.google.is_some();
        let u = &mut self.draft.upload;
        let mut sign_in = false;
        let mut cancel = false;
        egui::Grid::new("upload").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Client ID");
            ui.add(egui::TextEdit::singleline(&mut u.client_id).desired_width(420.0).hint_text("….apps.googleusercontent.com"));
            ui.end_row();
            label(ui, "Client secret");
            ui.add(egui::TextEdit::singleline(&mut u.client_secret).password(true).desired_width(260.0));
            ui.end_row();
            label(ui, "Google account");
            ui.horizontal(|ui| {
                if signing_in {
                    ui.spinner();
                    ui.label(RichText::new("Finish signing in in your browser…").color(TEXT_DIM));
                    if ui.small_button("Cancel").clicked() {
                        cancel = true;
                    }
                } else {
                    if u.signed_in() {
                        ui.label(RichText::new(format!("✓ {}", if u.account.is_empty() { "signed in" } else { &u.account })).color(GREEN));
                    }
                    let ready = !u.client_id.trim().is_empty() && !u.client_secret.trim().is_empty() && (u.youtube || u.drive);
                    let text = if u.signed_in() { "Sign in again" } else { "Sign in with Google" };
                    if ui.add_enabled(ready, egui::Button::new(text)).on_disabled_hover_text("Enter the client ID and secret, and choose YouTube and/or Drive below").clicked() {
                        sign_in = true;
                    }
                    if u.signed_in() && ui.small_button("Sign out").clicked() {
                        (u.refresh_token, u.account) = (String::new(), String::new());
                        u.granted.clear();
                    }
                }
            });
            ui.end_row();
        });
        let granted = |g: &str| u.granted.iter().any(|x| x == g);

        ui.add_space(12.0);
        egui::Grid::new("upload_yt").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "YouTube");
            ui.checkbox(&mut u.youtube, "upload the program");
            ui.end_row();
            if u.youtube {
                label(ui, "Visibility");
                ui.horizontal(|ui| {
                    for p in YoutubePrivacy::ALL {
                        ui.radio_value(&mut u.youtube_privacy, p, p.label());
                    }
                });
                ui.end_row();
                label(ui, "Title");
                ui.add(egui::TextEdit::singleline(&mut u.youtube_title).desired_width(380.0).hint_text("the show's name and the date"));
                ui.end_row();
                label(ui, "Description");
                ui.add(egui::TextEdit::multiline(&mut u.youtube_description).desired_width(380.0).desired_rows(3));
                ui.end_row();
                label(ui, "Chapters");
                ui.checkbox(&mut u.youtube_chapters, "the markers, in the description");
                ui.end_row();
            }
        });
        if u.youtube && u.signed_in() && !granted("youtube") {
            ui.label(RichText::new("⚠ Sign in again to allow YouTube.").color(AMBER));
        }

        ui.add_space(12.0);
        egui::Grid::new("upload_drive").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Google Drive");
            ui.checkbox(&mut u.drive, "upload files from the take");
            ui.end_row();
            if u.drive {
                label(ui, "What");
                ui.vertical(|ui| {
                    for d in DriveFiles::ALL {
                        ui.radio_value(&mut u.drive_files, d, d.label());
                    }
                });
                ui.end_row();
                label(ui, "Folder");
                ui.add(egui::TextEdit::singleline(&mut u.drive_folder).desired_width(200.0).hint_text("in My Drive"));
                ui.end_row();
            }
        });
        if u.drive && u.signed_in() && !granted("drive") {
            ui.label(RichText::new("⚠ Sign in again to allow Google Drive.").color(AMBER));
        }

        ui.add_space(12.0);
        egui::Grid::new("upload_when").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "When");
            ui.vertical(|ui| {
                ui.radio_value(&mut u.automatic, false, "when I press Upload (it appears as a take is saved)");
                ui.radio_value(&mut u.automatic, true, "straight away, as each take is saved");
            });
            ui.end_row();
        });

        if cancel {
            if let Some(g) = &self.google {
                g.cancel.store(true, Ordering::Relaxed);
            }
            self.google = None;
        }
        if sign_in {
            match GoogleSignIn::start(ui.ctx(), &self.draft.upload, false) {
                Ok(g) => self.google = Some(g),
                Err(e) => self.message = Some((format!("Can't sign in: {e}"), true)),
            }
        }
        if self.google.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
    }
}

/// Signing in to Google from Settings → Upload.
pub(super) struct GoogleSignIn {
    pub cancel: Arc<AtomicBool>,
    /// (refresh token, account, granted) or why not.
    pub result: Arc<Mutex<Option<Result<(String, String, Vec<String>), String>>>>,
}

impl GoogleSignIn {
    /// Start: opens Google's page in the browser.
    /// `live`: also allow scheduling live streams on YouTube.
    pub fn start(ctx: &egui::Context, u: &UploadConfig, live: bool) -> Result<GoogleSignIn, String> {
        let client = client_of(u);
        // Keep what was allowed before.
        let had = |g: &str| u.granted.iter().any(|x| x == g);
        let s = google::SignIn::begin(&client, &google::scopes(u.youtube, u.drive, live || had("live"))).map_err(|e| format!("{e:#}"))?;
        ctx.open_url(egui::OpenUrl::new_tab(&s.url));
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let (c2, r2, ctx2) = (cancel.clone(), result.clone(), ctx.clone());
        let _ = std::thread::Builder::new().name("google sign-in".into()).spawn(move || {
            let r = s.finish(&client, Duration::from_secs(300), &c2).and_then(|t| {
                let account = google::account(&client.ep, &t.access).unwrap_or_else(|_| "your Google account".into());
                Ok((t.refresh, account, google::granted(&t.scope)))
            });
            *r2.lock_ok() = Some(r.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        Ok(GoogleSignIn { cancel, result })
    }
}

pub(super) fn client_of(u: &UploadConfig) -> Client {
    Client { id: u.client_id.trim().to_string(), secret: u.client_secret.trim().to_string(), ep: Endpoints::default() }
}
