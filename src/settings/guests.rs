//! Settings → Remote guests: people joining from a browser, by link.

use super::*;
use crate::config::GuestAccess;
use crate::net::tunnel::TunnelState;

impl SettingsUi {
    /// Phones as cameras: each has a QR code to scan.
    fn phone_cameras(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        hint(ui, "Scan a phone's code with its camera app and allow the camera: it sends up to 1080p, picture only, over your Wi-Fi. Safari on iPhone, Chrome on Android.");
        let phones: Vec<usize> = self.draft.sources.iter().enumerate().filter(|(_, s)| s.phone_id().is_some()).map(|(i, _)| i).collect();
        let mut remove = None;
        for &i in &phones {
            let s = &mut self.draft.sources[i];
            let id = s.phone_id().unwrap_or("").to_string();
            ui.horizontal(|ui| {
                ui.add_sized([150.0, 22.0], egui::TextEdit::singleline(&mut s.name));
                let (text, color) = match info.guest_status.iter().find(|x| x.id == id) {
                    Some(st) if st.connected => ("● connected".to_string(), GREEN),
                    Some(st) if st.connecting => ("connecting…".to_string(), AMBER),
                    Some(_) => ("waiting for a phone".to_string(), TEXT_DIM),
                    None => ("apply to open it".to_string(), TEXT_DIM),
                };
                ui.label(RichText::new(text).color(color));
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
            let applied = self.original.sources.iter().any(|o| o.phone_id() == Some(id.as_str()));
            match info.guest_base.as_ref().filter(|_| applied) {
                Some(base) => {
                    let link = format!("{base}/g/{id}");
                    ui.horizontal(|ui| {
                        qr_code(ui, &link, 132.0);
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(RichText::new(&link).monospace().size(13.0)).truncate());
                            if ui.small_button("Copy link").clicked() {
                                ui.ctx().copy_text(link.clone());
                                self.message = Some((format!("Copied {}'s link", s.name), false));
                            }
                            hint(ui, "The code is also on the main screen (Guests → QR).");
                        });
                    });
                }
                None => hint(ui, "Its QR code appears here once Guests are on and this is applied."),
            }
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            self.draft.sources.remove(i);
        }
        if ui.button("＋ Phone camera").clicked() {
            let taken: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
            let name = unique(taken, "Phone");
            let name = if name == "Phone" { "Phone 1".to_string() } else { name };
            self.draft.sources.push(phone_source(&name));
            self.draft.guests.enabled = true;
        }
    }

    pub(super) fn guests(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Remote guests", "Guests join from a browser with a link (no app, no account), arriving as a source and an audio input. A phone can be a camera too.");
        let g = &mut self.draft.guests;
        egui::Grid::new("guests").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Guests");
            ui.checkbox(&mut g.enabled, "let guests join by link");
            ui.end_row();
            label(ui, "Reached through");
            ui.vertical(|ui| {
                ui.radio_value(&mut g.access, GuestAccess::Tunnel, "a free Cloudflare tunnel (nothing to set up)");
                ui.radio_value(&mut g.access, GuestAccess::Tailscale, "Tailscale Funnel (free, and the links stay the same)");
                if g.access == GuestAccess::Tailscale && crate::net::tunnel::tailscale_exe().is_none() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Tailscale isn't on this computer yet:").color(AMBER).size(12.0));
                        ui.hyperlink_to(RichText::new("get it from tailscale.com").size(12.0), "https://tailscale.com/download");
                        ui.label(RichText::new("and sign in (a free account).").color(TEXT_DIM).size(12.0));
                    });
                }
                ui.radio_value(&mut g.access, GuestAccess::Custom, "my own https:// address");
                if g.access == GuestAccess::Custom {
                    ui.add(egui::TextEdit::singleline(&mut g.public_url).hint_text("https://guests.example.com").desired_width(320.0));
                }
            });
            ui.end_row();
            label(ui, "Port");
            ui.add(egui::DragValue::new(&mut g.port).range(1024..=65535));
            ui.end_row();
            label(ui, "Guests see");
            ui.checkbox(&mut g.return_video, "the program");
            ui.end_row();
            label(ui, "Backup recordings");
            ui.checkbox(&mut g.backup, "their browsers record them too");
            ui.end_row();
        });
        if g.enabled {
            ui.add_space(4.0);
            match &info.guest_reach {
                TunnelState::Up(a) => ui.label(RichText::new(format!("✓ Guests can reach {a}")).color(GREEN)),
                TunnelState::Starting(s) => ui.label(RichText::new(s).color(TEXT_DIM)),
                TunnelState::Failed(e) => ui.label(RichText::new(format!("⚠ {e}")).color(AMBER)),
                TunnelState::Off => ui.label(RichText::new("Apply to start the guest page.").color(TEXT_DIM)),
                TunnelState::NeedsYou(what, url) => {
                    ui.label(RichText::new(format!("⚠ {what}")).color(AMBER));
                    ui.hyperlink_to("Allow it on Tailscale's site", url)
                }
            };
            match g.access {
                GuestAccess::Tunnel => hint(ui, "The address is new each time Cutaway starts: send links on the day. For links that stay the same, use Tailscale Funnel."),
                GuestAccess::Tailscale => hint(ui, "The address stays the same, so you can send links days ahead. It's named after this computer: rename it in Tailscale's admin console to change it (old links stop working)."),
                GuestAccess::Custom => {}
            }
        }

        ui.add_space(14.0);
        subtitle(ui, "Guest spots");
        hint(ui, "Each spot has its own link, source and audio input. Whoever opens the link appears there.");
        let guests: Vec<usize> = self.draft.sources.iter().enumerate().filter(|(_, s)| s.guest_id().is_some()).map(|(i, _)| i).collect();
        if guests.is_empty() {
            hint(ui, "No guest spots yet.");
        }
        let mut remove = None;
        let mut rename: Option<(String, String)> = None;
        // (link id, guest's name, share their screen)
        let mut screen: Option<(String, String, bool)> = None;
        let screens: Vec<String> = self.draft.sources.iter().filter_map(|s| s.guest_screen_id().map(str::to_string)).collect();
        egui::Grid::new("guest_spots").num_columns(5).spacing([12.0, 8.0]).striped(true).show(ui, |ui| {
            for &i in &guests {
                let s = &mut self.draft.sources[i];
                let id = s.guest_id().unwrap_or("").to_string();
                let before = s.name.clone();
                ui.add_sized([150.0, 22.0], egui::TextEdit::singleline(&mut s.name));
                if s.name != before {
                    rename = Some((before, s.name.clone()));
                }
                let status = info.guest_status.iter().find(|x| x.id == id);
                let (text, color) = match status {
                    Some(st) if st.connected => (format!("● {}", if st.guest_name.is_empty() { "on".to_string() } else { st.guest_name.clone() }), GREEN),
                    Some(st) if st.connecting => ("joining…".to_string(), AMBER),
                    Some(_) => ("waiting".to_string(), TEXT_DIM),
                    None => ("apply to open it".to_string(), TEXT_DIM),
                };
                ui.label(RichText::new(text).color(color));
                match info.guest_base.as_ref() {
                    Some(base) if self.original.sources.iter().any(|o| o.guest_id() == Some(id.as_str())) => {
                        let link = format!("{base}/g/{id}");
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&link).monospace().size(13.0)).truncate());
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(link.clone());
                                self.message = Some((format!("Copied {}'s link", s.name), false));
                            }
                        });
                    }
                    _ => {
                        ui.label(RichText::new("the link appears once guests are on and this is applied").color(TEXT_DIM).size(13.0));
                    }
                }
                let mut on = screens.contains(&id);
                if ui.checkbox(&mut on, "Screen share").on_hover_text("Adds a source for their screen: they can share it (or one window) from their page, as well as their camera.").changed() {
                    screen = Some((id.clone(), s.name.clone(), on));
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some((id, name, on)) = screen {
            if on {
                let taken: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
                self.draft.sources.push(screen_source(&unique(taken, &screen_name(&name)), &id));
            } else {
                self.draft.sources.retain(|s| s.guest_screen_id() != Some(id.as_str()));
            }
        }
        // The audio input follows its source's name (and so does their screen's).
        if let Some((old, new)) = &rename {
            for s in self.draft.sources.iter_mut() {
                if s.guest_screen_id().is_some() && s.name == screen_name(old) {
                    s.name = screen_name(new);
                }
            }
        }
        if let Some((old, new)) = rename {
            for a in self.draft.audio.iter_mut() {
                if a.device.trim().strip_prefix("source:").is_some_and(|n| n.trim().eq_ignore_ascii_case(&old)) {
                    a.device = format!("source:{new}");
                    if a.name.eq_ignore_ascii_case(&old) {
                        a.name = new.clone();
                    }
                }
            }
        }
        if let Some(i) = remove {
            let name = self.draft.sources[i].name.clone();
            let id = self.draft.sources[i].guest_id().map(str::to_string);
            self.draft.sources.remove(i);
            // Their screen goes with them.
            self.draft.sources.retain(|s| s.guest_screen_id().is_none() || s.guest_screen_id() != id.as_deref());
            self.draft.audio.retain(|a| !a.device.trim().strip_prefix("source:").is_some_and(|n| n.trim().eq_ignore_ascii_case(&name)));
        }
        ui.add_space(6.0);
        if ui.button("＋ Guest spot").clicked() {
            let mut taken: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
            taken.extend(self.draft.audio.iter().map(|a| a.name.clone()));
            let name = unique(taken, "Guest");
            let name = if name == "Guest" { "Guest 1".to_string() } else { name };
            self.draft.sources.push(SourceConfig {
                name: name.clone(),
                device: format!("guest:{}", crate::engine::guests::new_id()),
                width: 1280,
                height: 720,
                fps: 30,
                network_latency_ms: 250,
                ..Default::default()
            });
            self.draft.audio.push(AudioConfig { name: name.clone(), device: format!("source:{name}"), channels: 2, program_left: 1, program_right: 2, ..Default::default() });
            self.draft.guests.enabled = true;
        }
        ui.add_space(12.0);
        let phones = self.draft.sources.iter().filter(|s| s.phone_id().is_some()).count();
        let state = match phones {
            0 => "none".to_string(),
            1 => "1 phone".to_string(),
            n => format!("{n} phones"),
        };
        part(ui, "phones", "Phone cameras", &state, phones > 0, |ui| self.phone_cameras(ui, info));
        let state = if self.draft.guests.turn_url.trim().is_empty() { "not needed by most".to_string() } else { "set".to_string() };
        part(ui, "turn", "Strict networks (TURN)", &state, false, |ui| {
            let g = &mut self.draft.guests;
            egui::Grid::new("turn").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                label(ui, "TURN server");
                ui.add(egui::TextEdit::singleline(&mut g.turn_url).hint_text("turn:turn.example.com:3478").desired_width(320.0));
                ui.end_row();
                label(ui, "Username");
                ui.add(egui::TextEdit::singleline(&mut g.turn_username).desired_width(200.0));
                ui.end_row();
                label(ui, "Password");
                ui.add(egui::TextEdit::singleline(&mut g.turn_password).password(true).desired_width(200.0));
                ui.end_row();
            });
        });
        ui.add_space(4.0);
        hint(ui, "Guests need Chrome, Edge, Safari or Firefox; headphones help.");
    }
}

/// What a guest's screen source is called.
fn screen_name(guest: &str) -> String {
    format!("{guest}'s screen")
}

/// A source for a guest's shared screen (`id` is their link).
pub(super) fn screen_source(name: &str, id: &str) -> SourceConfig {
    SourceConfig { name: name.to_string(), device: format!("guest-screen:{id}"), width: 1920, height: 1080, fps: 30, network_latency_ms: 250, ..Default::default() }
}

/// A new phone camera source.
pub(super) fn phone_source(name: &str) -> SourceConfig {
    SourceConfig { name: name.to_string(), device: format!("phone:{}", crate::engine::guests::new_id()), width: 1920, height: 1080, fps: 30, network_latency_ms: 150, ..Default::default() }
}
