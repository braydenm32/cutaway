//! Settings → Remote & MIDI, and show profiles (in General).

use super::*;
use crate::config::MidiMapping;
use crate::control::{midi, profiles};

impl SettingsUi {
    pub(super) fn control(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Remote & MIDI", "Run the show from a phone or tablet, a Stream Deck, Companion, show-control software or a MIDI controller.");
        let r = &mut self.draft.remote;
        let state = if r.enabled { format!("on, port {}{}", r.port, if r.pin.trim().is_empty() { ", no PIN" } else { "" }) } else { "off".to_string() };
        part(ui, "remote", "Phone and tablet remote", &state, true, |ui| {
            egui::Grid::new("remote").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Remote control");
                ui.checkbox(&mut r.enabled, "devices on this network can run the show");
                ui.end_row();
                label(ui, "Port");
                ui.add(egui::DragValue::new(&mut r.port).range(1024..=65535));
                ui.end_row();
                label(ui, "PIN");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut r.pin).desired_width(90.0).hint_text("none"));
                    if ui.small_button("New PIN").clicked() {
                        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(4321);
                        r.pin = format!("{:04}", n % 10_000);
                    }
                    if r.pin.trim().is_empty() {
                        ui.label(RichText::new("anyone on the network could control the show").color(AMBER).size(12.5));
                    }
                });
                ui.end_row();
            });
            if r.enabled {
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
                if info.remote_port.is_none() && self.original.remote.enabled {
                    ui.label(RichText::new("⚠ The remote isn't running (see the log: the port may be in use).").color(AMBER));
                }
                hint(ui, &format!("Tally lights: open {url}/?tally=1 (2, 3…) on a phone mounted on each camera — red when on air, green in preview."));
                ui.add_space(6.0);
                ui.label(RichText::new("Stream Deck, Bitfocus Companion and scripts").strong());
                hint(ui, "Any button that can open a web address can run a command:");
                let pin = if r.pin.trim().is_empty() { String::new() } else { format!("&pin={}", r.pin.trim()) };
                for (cmd, what) in [("scene:1", "take scene 1"), ("preview:2", "preview scene 2"), ("take", "take preview to program"), ("record", "record on/off"), ("marker", "add a marker"), ("graphic:1", "graphic 1 on/off")] {
                    ui.label(RichText::new(format!("{url}/api/action?do={cmd}{pin}    ({what})")).monospace().size(12.5).color(TEXT_DIM));
                }
                hint(ui, "The full command list is in the README. Scripts can also connect to ws://…/api/ws for live state.");
            }
        });
        part(ui, "streamdeck", "Stream Deck", if self.draft.remote.enabled { "" } else { "needs the remote" }, false, |ui| {
            hint(ui, "The Cutaway plugin's keys show live pictures of scenes and sources, light up red and green, and run anything: take and cut, record, go live, markers, graphics, audio (with Stream Deck+ dials), auto-switching, media players and any command.");
            ui.horizontal(|ui| {
                let plugin = crate::util::bundled("Cutaway.streamDeckPlugin", "streamdeck");
                if ui.add_enabled(plugin.is_some(), egui::Button::new("Install the Stream Deck plugin…")).clicked() {
                    if let Some(p) = &plugin {
                        if let Err(e) = crate::util::open_with_system(p) {
                            self.plugin_note = Some(format!("Couldn't open {}: {e}", p.display()));
                        } else {
                            self.plugin_note = Some("The Stream Deck app will ask to install it.".into());
                        }
                    }
                }
                match (&plugin, &self.plugin_note) {
                    (_, Some(n)) => hint(ui, n),
                    (None, None) => hint(ui, "Cutaway.streamDeckPlugin is in the streamdeck folder of the download: double-click it."),
                    _ => {}
                }
            });
            if !self.draft.remote.enabled {
                ui.label(RichText::new("The plugin talks to Cutaway through remote control: turn it on above.").color(AMBER));
            } else {
                let ip = crate::engine::netsrc::local_ip().unwrap_or_else(|| "this-computer".into());
                hint(ui, &format!("In a key's settings, use address 127.0.0.1:{} on this computer ({ip}:{} from another one), and the PIN.", self.draft.remote.port, self.draft.remote.port));
            }
        });
        let o = &mut self.draft.osc;
        let state = if o.enabled { format!("on, UDP port {}", o.port) } else { "off".to_string() };
        part(ui, "osc", "OSC (show-control software)", &state, false, |ui| {
            egui::Grid::new("osc").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
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
                        ui.label(RichText::new(addr).monospace().size(12.5).color(TEXT_DIM));
                        ui.label(RichText::new(what).size(12.5).color(TEXT_DIM));
                    });
                }
                hint(ui, "State goes back to the address above whenever it changes: /cutaway/scene/N and /cutaway/preview/N (1 or 0), /cutaway/record, /cutaway/live, /cutaway/graphic/N, /cutaway/audio/N, /cutaway/level/N (0–1), /cutaway/tally/N and more (see the README).");
            }
        });
        if !midi::available() {
            part(ui, "midi", "MIDI controller", "needs macOS or Windows", false, |ui| hint(ui, "MIDI needs macOS or Windows."));
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
        part(ui, "midi", "MIDI controller", &state, self.draft.midi.enabled, |ui| {
            let m = &mut self.draft.midi;
            egui::Grid::new("midi").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
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
            egui::Grid::new("mappings").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
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
                    if ui.small_button("✕").clicked() {
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

    /// Show profiles, inside General.
    pub(super) fn shows(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        subtitle(ui, "Shows");
        let Some(dir) = info.profiles_dir.clone() else {
            hint(ui, "Demo mode: shows can't be saved.");
            return;
        };
        hint(ui, "A show is a whole setup: sources, scenes, audio, graphics, streaming. Save one per production and switch from the header.");
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
                    ui.label(RichText::new(name).strong().color(if is_current { GREEN } else { Color32::WHITE }));
                    if is_current {
                        ui.label(RichText::new("(these settings)").size(12.5).color(TEXT_DIM));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Delete").clicked() {
                            self.message = Some(match profiles::delete(&dir, name) {
                                Ok(()) => (format!("Deleted \"{name}\""), false),
                                Err(e) => (format!("{e:#}"), true),
                            });
                        }
                        if ui.small_button("Rename").on_hover_text("to the name typed below").clicked() && !self.rename_to.trim().is_empty() {
                            self.message = Some(match profiles::rename(&dir, name, self.rename_to.trim()) {
                                Ok(()) => {
                                    if is_current {
                                        self.draft.ui.profile = self.rename_to.trim().to_string();
                                    }
                                    (format!("Renamed to \"{}\"", self.rename_to.trim()), false)
                                }
                                Err(e) => (format!("{e:#}"), true),
                            });
                        }
                        if ui.small_button("Load").on_hover_text("Loads into this window; Apply to switch").clicked() {
                            self.message = Some(match profiles::load(&dir, name) {
                                Ok(cfg) => {
                                    self.draft = cfg;
                                    (format!("Loaded \"{name}\" — press Apply to switch to it"), false)
                                }
                                Err(e) => (format!("{e:#}"), true),
                            });
                        }
                    });
                });
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.profile_name).hint_text("show name").desired_width(200.0));
            if ui.add_enabled(!self.profile_name.trim().is_empty(), egui::Button::new("Save these settings as a show")).clicked() {
                let name = self.profile_name.trim().to_string();
                self.draft.ui.profile = name.clone();
                self.message = Some(match profiles::save(&dir, &name, &self.draft) {
                    Ok(()) => (format!("Saved \"{name}\""), false),
                    Err(e) => (format!("{e:#}"), true),
                });
                self.profile_name.clear();
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.rename_to).hint_text("new name (for Rename)").desired_width(200.0));
        });
    }
}
