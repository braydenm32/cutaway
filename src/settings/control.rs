//! More › Control (the phone remote, Stream Deck, OSC and MIDI), and
//! More › App › Shows.

use super::*;
use crate::config::MidiMapping;
use crate::control::{midi, profiles};

/// The remote's address with the PIN in it (`?pin=`, which the page reads), for the QR code.
fn remote_link(url: &str, pin: &str) -> String {
    let pin = pin.trim();
    if pin.is_empty() {
        return format!("{url}/");
    }
    let mut enc = String::new();
    for b in pin.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{url}/?pin={enc}")
}

#[cfg(test)]
mod tests {
    use super::remote_link;

    #[test]
    fn the_qr_link_carries_the_pin() {
        assert_eq!(remote_link("http://10.0.0.5:8080", " 4821 "), "http://10.0.0.5:8080/?pin=4821");
        assert_eq!(remote_link("http://10.0.0.5:8080", "a b&c"), "http://10.0.0.5:8080/?pin=a%20b%26c");
        assert_eq!(remote_link("http://10.0.0.5:8080", ""), "http://10.0.0.5:8080/");
    }
}

/// The Stream Deck plugin's actions, as its manifest names and explains
/// them (so the page lists exactly what the plugin has).
fn stream_deck_actions() -> Vec<(String, String)> {
    let manifest: serde_json::Value = serde_json::from_str(include_str!("../../streamdeck/app.cutaway.streamdeck.sdPlugin/manifest.json")).unwrap_or_default();
    let actions = manifest["Actions"].as_array().cloned().unwrap_or_default();
    actions.iter().map(|a| (a["Name"].as_str().unwrap_or_default().to_string(), a["Tooltip"].as_str().unwrap_or_default().to_string())).collect()
}

/// A part of More › Control: folded under its heading when the whole page
/// shows, or (`only`, the section's own page) with its state on one line.
fn control_part(ui: &mut egui::Ui, id: &str, only: Option<&str>, name: &str, state: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    let shown = match only {
        None => true,
        Some("osc") => id == "osc" || id == "midi",
        Some(o) => o == id,
    };
    if !shown {
        return;
    }
    // (OSC and MIDI share a section: each keeps its heading there.)
    if only.is_none() || only == Some("osc") {
        part(ui, id, name, state, open || only.is_some(), body);
        return;
    }
    if !state.is_empty() {
        // (On or Off: the port and PIN are in the form just below.)
        let on = state.starts_with("on");
        let words = match state {
            s if s.starts_with("on") => "On",
            "off" => "Off",
            s => s,
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.5, if on { GREEN } else { TEXT_DIM });
            ui.label(RichText::new(words).color(if on { Color32::WHITE } else { TEXT_DIM }));
        });
        ui.add_space(4.0);
    }
    body(ui);
}

impl SettingsUi {
    /// More › Control: the phone remote, the Stream Deck, OSC and MIDI
    /// (`part_only` picks the section's part; None: all of them).
    pub(super) fn control(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let only = self.part_only;
        let intro = match only {
            Some("remote") => "Run the show from a phone or tablet on the same Wi-Fi, and use phones as tally lights. Scripts and show-control apps use the same address.",
            Some("streamdeck") => "Stream Deck keys with live pictures of scenes and sources, red and green tally, and every command.",
            Some("osc") => "Show-control software (QLab, Companion, TouchOSC, lighting desks) over OSC, and MIDI controllers' pads, keys and faders.",
            _ => "Run the show from a phone or tablet, a Stream Deck, Companion, show-control software or a MIDI controller.",
        };
        title(ui, "Control", intro);
        let sources: Vec<String> = self.draft.sources.iter().map(|s| s.name.clone()).collect();
        let r = &mut self.draft.remote;
        let state = if r.enabled { format!("on, port {}, PIN {}", r.port, r.pin.trim()) } else { "off".to_string() };
        control_part(ui, "remote", only, "Phone and tablet remote", &state, true, |ui| {
            egui::Grid::new("remote").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Remote control");
                ui.checkbox(&mut r.enabled, "devices on this network can run the show");
                ui.end_row();
                label(ui, "Port");
                ui.add(egui::DragValue::new(&mut r.port).range(1024..=65535));
                ui.end_row();
                label(ui, "PIN");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut r.pin).desired_width(120.0).hint_text("made for you"));
                    if ui.small_button("New PIN").clicked() {
                        r.pin = crate::config::new_pin();
                    }
                    if r.enabled && r.pin.trim().is_empty() {
                        ui.label(RichText::new("a PIN is made as it turns on").color(TEXT_DIM).size(TEXT_BODY));
                    } else if r.pin.trim().chars().count() < 4 {
                        ui.label(RichText::new("short PINs are easy to guess: use 4 or more").color(AMBER).size(TEXT_BODY));
                    }
                });
                ui.end_row();
            });
            if !r.enabled {
                hint(ui, "Turn it on to get the address and a QR code for your phone, tally lights, and the links for scripts.");
                return;
            }
            let ip = crate::engine::netsrc::local_ip().unwrap_or_else(|| "this-computer".into());
            let url = format!("http://{ip}:{}", r.port);
            ui.horizontal(|ui| {
                ui.label("Open");
                ui.label(RichText::new(&url).monospace().color(ACCENT));
                ui.label("on a phone or tablet on the same Wi-Fi.");
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
            });
            ui.horizontal(|ui| {
                qr_code(ui, &remote_link(&url, &r.pin), 132.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Scan with your phone's camera").size(TEXT_BODY).color(TEXT_DIM));
                    hint(ui, "It opens the remote already signed in with the PIN.");
                });
            });
            if info.remote_port.is_none() && self.original.remote.enabled {
                ui.label(RichText::new("⚠ The remote isn't running (see the log: the port may be in use).").color(AMBER));
            }
            let pin_q = if r.pin.trim().is_empty() { String::new() } else { format!("&pin={}", r.pin.trim()) };
            ui.add_space(6.0);
            ui.label(RichText::new("Tally lights").strong());
            hint(ui, "Open one of these on a phone mounted on each camera: red when it's on air, green in preview. (The phone remote's More tab lists them too.)");
            ui.horizontal_wrapped(|ui| {
                for (i, name) in sources.iter().enumerate() {
                    let link = format!("{url}/?tally={}{pin_q}", i + 1);
                    if ui.small_button(format!("{name}  ⧉")).on_hover_text(format!("Copy {link}")).clicked() {
                        ui.ctx().copy_text(link);
                    }
                }
                if sources.is_empty() {
                    hint(ui, "(Add sources first.)");
                }
            });
            ui.add_space(6.0);
            egui::CollapsingHeader::new(RichText::new("For scripts, Companion and other apps").strong()).id_salt("remote_api").show(ui, |ui| {
                hint(ui, "Any button that can open a web address can run a command:");
                let pin = format!("&pin={}", if r.pin.trim().is_empty() { "PIN" } else { r.pin.trim() });
                for (cmd, what) in [("scene:1", "take scene 1"), ("preview:2", "preview scene 2"), ("take", "take preview to program"), ("record", "record on/off"), ("marker", "add a marker"), ("graphic:1", "graphic 1 on/off")] {
                    ui.label(RichText::new(format!("{url}/api/action?do={cmd}{pin}    ({what})")).monospace().size(TEXT_BODY).color(TEXT_DIM));
                }
                ui.add_space(4.0);
                for (path, what) in [
                    ("/api/state", "everything on the remote as JSON: scenes, tally, recording, levels"),
                    ("/api/ws", "the same, live, over a WebSocket (ws://…)"),
                    ("/api/program.jpg", "a still of the program"),
                    ("/api/tally/1", "source 1's tally (program, preview or off), for lights and scripts"),
                    ("/prompter", "the teleprompter page (More › Show tools › Teleprompter)"),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("{url}{path}")).monospace().size(TEXT_BODY).color(TEXT_DIM));
                        ui.label(RichText::new(what).size(TEXT_BODY).color(TEXT_DIM));
                    });
                }
                hint(ui, "Each needs the PIN (pin=…). The full command list is in the README.");
            });
        });
        let remote_on = self.draft.remote.enabled;
        let port = self.draft.remote.port;
        let mut note = self.plugin_note.clone();
        control_part(ui, "streamdeck", only, "Stream Deck", if remote_on { "" } else { "needs the remote" }, false, |ui| {
            hint(ui, "The Cutaway plugin's keys show live pictures of scenes and sources (drawn by Cutaway, /api/key), light up red and green, and run anything.");
            ui.horizontal(|ui| {
                let plugin = crate::util::bundled("Cutaway.streamDeckPlugin", "streamdeck");
                if ui.add_enabled(plugin.is_some(), egui::Button::new("Install the Stream Deck plugin…")).clicked() {
                    if let Some(p) = &plugin {
                        note = Some(match crate::util::open_with_system(p) {
                            Err(e) => format!("Couldn't open {}: {e}", p.display()),
                            Ok(_) => "The Stream Deck app will ask to install it.".into(),
                        });
                    }
                }
                match (&plugin, &note) {
                    (_, Some(n)) => hint(ui, n),
                    (None, None) => hint(ui, "Cutaway.streamDeckPlugin is in the streamdeck folder of the download: double-click it."),
                    _ => {}
                }
            });
            if !remote_on {
                ui.label(RichText::new("The plugin talks to Cutaway through the phone remote: turn it on in More › Control › Phone remote.").color(AMBER));
            } else {
                let ip = crate::engine::netsrc::local_ip().unwrap_or_else(|| "this-computer".into());
                hint(ui, &format!("In a key's settings, use address 127.0.0.1:{port} on this computer ({ip}:{port} from another one), and the PIN."));
            }
            ui.add_space(6.0);
            let actions = stream_deck_actions();
            ui.label(RichText::new(format!("The plugin's {} keys", actions.len())).strong());
            egui::Grid::new("sd_actions").num_columns(2).spacing([14.0, 4.0]).striped(true).show(ui, |ui| {
                for (name, what) in &actions {
                    ui.label(RichText::new(name).size(TEXT_BODY).color(Color32::WHITE));
                    ui.add(egui::Label::new(RichText::new(what).size(TEXT_BODY).color(TEXT_DIM)).wrap());
                    ui.end_row();
                }
            });
        });
        self.plugin_note = note;
        let o = &mut self.draft.osc;
        let state = if o.enabled { format!("on, UDP port {}", o.port) } else { "off".to_string() };
        control_part(ui, "osc", only, "OSC (show-control software)", &state, false, |ui| {
            egui::Grid::new("osc").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "OSC");
                ui.checkbox(&mut o.enabled, "accept OSC commands");
                ui.end_row();
                label(ui, "Listen on UDP port");
                ui.add(egui::DragValue::new(&mut o.port).range(1024..=65535));
                ui.end_row();
                label(ui, "Send state to");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut o.feedback_host).desired_width(160.0).hint_text("address (optional)"));
                    ui.label("port");
                    ui.add(egui::DragValue::new(&mut o.feedback_port).range(1..=65535));
                });
                ui.end_row();
            });
            if o.enabled {
                if info.osc_port.is_none() && self.original.osc.enabled {
                    ui.label(RichText::new("⚠ OSC isn't running (see the log: the port may be in use).").color(AMBER));
                }
                for (addr, what) in [
                    ("/cutaway/scene/2", "take scene 2 (or /cutaway/scene \"Host\")"),
                    ("/cutaway/preview/2  ·  /cutaway/take  ·  /cutaway/cut", "preview, then take or cut"),
                    ("/cutaway/record 1  ·  /cutaway/record 0", "record on / off (no value toggles); also live, auto, captions"),
                    ("/cutaway/graphic/1 1  ·  /cutaway/audio/2 0", "graphics and audio inputs on / off"),
                    ("/cutaway/fader/Mic 0.75  ·  /cutaway/gain/Mic -6", "input level from a fader (0–1) or in dB"),
                    ("/cutaway/marker \"Guest arrives\"  ·  /cutaway/source/3", "a marker; a source full screen"),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(addr).monospace().size(TEXT_BODY).color(TEXT_DIM));
                        ui.label(RichText::new(what).size(TEXT_BODY).color(TEXT_DIM));
                    });
                }
                hint(ui, "State goes back to the address above whenever it changes: /cutaway/scene/N and /cutaway/preview/N (1 or 0), /cutaway/record, /cutaway/live, /cutaway/graphic/N, /cutaway/audio/N, /cutaway/level/N (0–1), /cutaway/tally/N and more (see the README).");
            }
        });
        if !midi::available() {
            control_part(ui, "midi", only, "MIDI controller", "needs macOS or Windows", false, |ui| hint(ui, "MIDI needs macOS or Windows."));
            return;
        }
        let m = &self.draft.midi;
        let state = if m.enabled {
            match m.mappings.len() {
                1 => "on, 1 mapping".to_string(),
                n => format!("on, {n} mappings"),
            }
        } else {
            "off".to_string()
        };
        control_part(ui, "midi", only, "MIDI controller", &state, self.draft.midi.enabled, |ui| {
            let m = &mut self.draft.midi;
            egui::Grid::new("midi").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "MIDI control");
                ui.checkbox(&mut m.enabled, "use MIDI controllers");
                ui.end_row();
                label(ui, "Controller");
                let ports = midi::port_names();
                egui::ComboBox::from_id_salt("midi_dev").selected_text(if m.device.is_empty() { "all connected" } else { &m.device }).width(260.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.device, String::new(), "all connected");
                    for p in ports {
                        ui.selectable_value(&mut m.device, p.clone(), p);
                    }
                });
                ui.end_row();
                label(ui, "Light up pads");
                ui.checkbox(&mut m.feedback, "show what's on air on the controller (pads that support it)");
                ui.end_row();
            });
            if !info.midi_connected.is_empty() {
                hint(ui, &format!("Connected: {}", info.midi_connected.join(", ")));
            }
            ui.add_space(6.0);
            let commands = crate::control::suggestions(&self.draft);
            let m = &mut self.draft.midi;
            let mut remove = None;
            // Learn: the next message received fills in the mapping.
            if let (Some(i), Some(msg)) = (self.learning, &info.midi_last) {
                if let Some(map) = m.mappings.get_mut(i) {
                    map.message = msg.clone();
                    self.learning = None;
                }
            }
            egui::Grid::new("mappings").num_columns(4).spacing(TABLE_GAP).show(ui, |ui| {
                for (i, map) in m.mappings.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut map.message).desired_width(110.0).hint_text("note 36"));
                    let learning = self.learning == Some(i);
                    if ui.add(egui::Button::selectable(learning, if learning { "press a pad…" } else { "Learn" })).clicked() {
                        self.learning = if learning { None } else { Some(i) };
                    }
                    let shown = commands.iter().find(|c| c.0 == map.action).map(|c| c.1.clone()).unwrap_or_else(|| if map.action.is_empty() { "choose…".into() } else { map.action.clone() });
                    egui::ComboBox::from_id_salt(("map_action", i)).selected_text(shown).width(240.0).show_ui(ui, |ui| {
                        for (code, text) in &commands {
                            ui.selectable_value(&mut map.action, code.clone(), text);
                        }
                        ui.separator();
                        for a in &self.draft.audio {
                            ui.selectable_value(&mut map.action, format!("fader:{}", a.name), format!("Fader: {} level", a.name));
                        }
                    });
                    if row_buttons(ui, i, 0, false) == Some(RowEdit::Remove) {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                m.mappings.remove(i);
            }
            if ui.button("＋ Add a mapping").clicked() {
                m.mappings.push(MidiMapping::default());
                self.learning = Some(m.mappings.len() - 1);
            }
            hint(ui, "Press Learn, then the pad, key or fader. Faders (CC messages) can set an input's level.");
            if self.learning.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
    }

    /// Saved shows (More › App › Shows).
    pub(super) fn shows(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let Some(dir) = info.profiles_dir.clone() else {
            hint(ui, "Demo mode: shows can't be saved.");
            return;
        };
        hint(ui, "A show is a whole setup: sources, scenes, audio, graphics, streaming. Save one per production; switch here or from the show's name under the logo.");
        let current = self.draft.ui.profile.clone();
        let list = profiles::list(&dir);
        ui.add_space(4.0);
        egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(8)).corner_radius(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width().min(560.0));
            if list.is_empty() {
                hint(ui, "No saved shows yet.");
            }
            for name in &list {
                ui.horizontal(|ui| {
                    let is_current = *name == current;
                    // Renaming this one: its name becomes a box.
                    if let Some((old, to)) = self.renaming.as_mut().filter(|(old, _)| old == name) {
                        let r = ui.add(egui::TextEdit::singleline(to).desired_width(200.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let to = to.trim().to_string();
                        let old = old.clone();
                        if ui.add_enabled(!to.is_empty() && to != old, egui::Button::new("Rename")).clicked() || (enter && !to.is_empty() && to != old) {
                            self.message = Some(match profiles::rename(&dir, &old, &to) {
                                Ok(()) => {
                                    if is_current {
                                        self.draft.ui.profile = to.clone();
                                    }
                                    (format!("Renamed to \"{to}\""), false)
                                }
                                Err(e) => (format!("{e:#}"), true),
                            });
                            self.renaming = None;
                        } else if ui.button("Cancel").clicked() {
                            self.renaming = None;
                        }
                        return;
                    }
                    ui.label(RichText::new(name).strong().color(if is_current { GREEN } else { Color32::WHITE }));
                    if is_current {
                        ui.label(RichText::new("(these settings)").size(TEXT_BODY).color(TEXT_DIM));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.deleting.as_deref() == Some(name.as_str()) {
                            if ui.small_button("Keep it").clicked() {
                                self.deleting = None;
                            }
                            if ui.add(egui::Button::new(RichText::new("Delete for good").color(Color32::WHITE)).fill(ERROR).small()).clicked() {
                                self.message = Some(match profiles::delete(&dir, name) {
                                    Ok(()) => (format!("Deleted \"{name}\""), false),
                                    Err(e) => (format!("{e:#}"), true),
                                });
                                self.deleting = None;
                            }
                            ui.label(RichText::new("Delete this show?").color(AMBER));
                            return;
                        }
                        if ui.small_button("Delete").clicked() {
                            self.deleting = Some(name.clone());
                        }
                        if ui.small_button("Rename").clicked() {
                            self.renaming = Some((name.clone(), name.clone()));
                        }
                        if !is_current && ui.small_button("Switch to it").on_hover_text("Asks first; this show's settings are kept as they are").clicked() {
                            self.pending = Some(SettingsAction::SwitchShow(name.clone()));
                        }
                    });
                });
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.profile_name).hint_text("show name").desired_width(200.0));
            let name = self.profile_name.trim().to_string();
            // Saving over a show that's already there asks first.
            let exists = list.iter().any(|n| n.eq_ignore_ascii_case(&name)) && name != current;
            let label = if exists { "Replace that show with these settings" } else { "Save these settings as a show" };
            let b = ui.add_enabled(!name.is_empty(), egui::Button::new(label));
            let b = if exists { b.on_hover_text(format!("There's already a show called \"{name}\": this replaces it")) } else { b };
            if b.clicked() {
                // What's in effect (not changes still waiting for Apply).
                let mut cfg = self.live_part();
                cfg.ui.profile = name.clone();
                self.draft.ui.profile = name.clone();
                self.message = Some(match profiles::save(&dir, &name, &cfg) {
                    Ok(()) => (format!("Saved \"{name}\""), false),
                    Err(e) => (format!("{e:#}"), true),
                });
                self.profile_name.clear();
            }
        });
    }
}
