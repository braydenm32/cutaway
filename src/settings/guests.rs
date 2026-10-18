//! Settings → Remote guests: people joining from a browser, by link.

use super::*;
use crate::config::GuestAccess;
use crate::net::tunnel::TunnelState;

impl SettingsUi {
    pub(super) fn guests(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Remote guests", "Guests join from their browser — computer, tablet or phone — with a link: no app, no account. They become a source and an audio input, hear the show without their own voice, and can see the program.");
        let g = &mut self.draft.guests;
        egui::Grid::new("guests").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Guests");
            ui.checkbox(&mut g.enabled, "let guests join by link");
            ui.end_row();
            label(ui, "Reached through");
            ui.vertical(|ui| {
                ui.radio_value(&mut g.access, GuestAccess::Tunnel, "a free Cloudflare tunnel (an https://….trycloudflare.com address; nothing to set up)");
                ui.radio_value(&mut g.access, GuestAccess::Custom, "my own https:// address, forwarded to this computer");
                if g.access == GuestAccess::Custom {
                    ui.add(egui::TextEdit::singleline(&mut g.public_url).hint_text("https://guests.example.com").desired_width(320.0));
                }
            });
            ui.end_row();
            label(ui, "Port");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut g.port).range(1024..=65535));
                hint(ui, if g.access == GuestAccess::Tunnel { "only this computer can reach it; the tunnel brings guests in" } else { "open to the network, for your forwarding" });
            });
            ui.end_row();
            label(ui, "Guests see");
            ui.checkbox(&mut g.return_video, "the program (small, so it's light on their connection)");
            ui.end_row();
        });
        if g.enabled {
            ui.add_space(4.0);
            match &info.guest_reach {
                TunnelState::Up(a) => ui.label(RichText::new(format!("✓ Guests can reach {a}")).color(GREEN)),
                TunnelState::Starting(s) => ui.label(RichText::new(s).color(TEXT_DIM)),
                TunnelState::Failed(e) => ui.label(RichText::new(format!("⚠ {e}")).color(AMBER)),
                TunnelState::Off => ui.label(RichText::new("Apply to start the guest page.").color(TEXT_DIM)),
            };
            if g.access == GuestAccess::Tunnel {
                hint(ui, "The tunnel's address is new each time Cutaway starts, so send links on the day. Cutaway downloads Cloudflare's small tunnel program the first time (or uses cloudflared if it's installed).");
            }
        }

        ui.add_space(14.0);
        subtitle(ui, "Guest spots");
        hint(ui, "Each spot has its own link, video source and audio input (named alike). Send each guest their spot's link; whoever opens it appears there.");
        let guests: Vec<usize> = self.draft.sources.iter().enumerate().filter(|(_, s)| s.guest_id().is_some()).map(|(i, _)| i).collect();
        let mut remove = None;
        let mut rename: Option<(String, String)> = None;
        egui::Grid::new("guest_spots").num_columns(4).spacing([12.0, 8.0]).striped(true).show(ui, |ui| {
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
                            ui.add(egui::Label::new(RichText::new(&link).monospace().size(12.0)).truncate());
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(link.clone());
                                self.message = Some((format!("Copied {}'s link", s.name), false));
                            }
                        });
                    }
                    _ => {
                        ui.label(RichText::new("the link appears once guests are on and this is applied").color(TEXT_DIM).size(12.0));
                    }
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        // The audio input follows its source's name.
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
            self.draft.sources.remove(i);
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
        hint(ui, "Adding or removing a spot restarts the video engine (stop recording first). Guests already on stay connected.");

        ui.add_space(14.0);
        egui::CollapsingHeader::new(RichText::new("Strict networks (TURN)").strong()).default_open(!self.draft.guests.turn_url.is_empty()).show(ui, |ui| {
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
            hint(ui, "Most guests connect directly. Some office, school and hotel networks block that; a TURN server (e.g. Cloudflare Calls, Twilio, or your own coturn) relays for them.");
        });
        ui.add_space(8.0);
        hint(ui, "Guests need Chrome, Edge, Safari or Firefox, and headphones help. Their sound arrives as the audio input of the same name; the outputs and mix-minus set in Mixing & headphones work for them too.");
    }
}
