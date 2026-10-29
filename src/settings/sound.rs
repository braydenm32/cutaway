//! Settings → Sound pads and Settings → Syrinscape.

use crate::util::LockExt;
use super::*;
use crate::config::{Pad, PadMode, SyrinButton, SyrinKind};
use crate::control::syrinscape::{parse_link, Syrinscape};

/// The input strip pads (and stinger sounds) play through.
fn pads_input() -> AudioConfig {
    AudioConfig { name: "Sound pads".into(), device: "pads".into(), channels: 2, program_left: 1, program_right: 2, record: true, ..Default::default() }
}

pub(super) fn color_button(ui: &mut egui::Ui, value: &mut String, fallback: [u8; 3]) {
    let mut rgb = crate::config::parse_color(value).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or(fallback);
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
    }
}

const PAD_COLORS: [&str; 6] = ["#3b6fd8", "#d8873b", "#3bb36f", "#b33bb3", "#d83b4f", "#3bb3b3"];

impl SettingsUi {
    pub(super) fn pads(&mut self, ui: &mut egui::Ui) {
        title(ui, "Sound pads", "Jingles, stings, applause and music beds on buttons, F1–F12, the Stream Deck, MIDI pads and the remote.");
        if !self.draft.audio.iter().any(|a| a.is_pads()) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚠ Pads play through a \"Sound pads\" audio input, which this show doesn't have yet.").color(AMBER));
                if ui.button("Add it").clicked() {
                    let mut a = pads_input();
                    a.name = unique(self.draft.audio.iter().map(|a| a.name.clone()).collect(), "Sound pads");
                    self.draft.audio.push(a);
                }
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            if ui.button("＋ Add sounds…").clicked() {
                if let Some(files) = rfd::FileDialog::new().add_filter("Sounds", crate::media::audiofile::EXTENSIONS).pick_files() {
                    for f in files {
                        let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
                        let name = unique(self.draft.pads.iter().map(|p| p.name.clone()).collect(), stem.trim());
                        let color = PAD_COLORS[self.draft.pads.len() % PAD_COLORS.len()].to_string();
                        self.draft.pads.push(Pad { name, file: f, color, ..Default::default() });
                    }
                    if !self.draft.audio.iter().any(|a| a.is_pads()) {
                        self.draft.audio.push(pads_input());
                    }
                }
            }
            hint(ui, "WAV, AIFF, MP3, AAC/M4A, FLAC, Ogg or CAF, loaded into memory so they start instantly.");
        });
        ui.add_space(8.0);
        let n = self.draft.pads.len();
        let mut remove = None;
        let mut swap = None;
        egui::Grid::new("pads").num_columns(8).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
            for h in ["Key", "Name", "Sound", "Plays", "Level", "Fade out", "Colour", ""] {
                column_head(ui, h);
            }
            ui.end_row();
            for (i, p) in self.draft.pads.iter_mut().enumerate() {
                ui.label(RichText::new(if i < 12 { format!("F{}", i + 1) } else { "—".into() }).monospace());
                ui.add_sized([130.0, 22.0], egui::TextEdit::singleline(&mut p.name));
                ui.horizontal(|ui| {
                    let shown = p.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "choose a file".into());
                    let missing = !p.file.as_os_str().is_empty() && !p.file.exists();
                    ui.add(egui::Label::new(RichText::new(shown).color(if missing { AMBER } else { Color32::WHITE })).truncate()).on_hover_text(if missing { format!("⚠ {} is missing", p.file.display()) } else { p.file.display().to_string() });
                    if ui.small_button("…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Sounds", crate::media::audiofile::EXTENSIONS).pick_file() {
                            p.file = f;
                        }
                    }
                });
                egui::ComboBox::from_id_salt(("pad_mode", i)).selected_text(p.mode.label()).width(100.0).show_ui(ui, |ui| {
                    for m in PadMode::ALL {
                        ui.selectable_value(&mut p.mode, m, m.label());
                    }
                });
                ui.add(egui::DragValue::new(&mut p.gain_db).range(-40.0..=12.0).speed(0.2).suffix(" dB"));
                ui.add(egui::DragValue::new(&mut p.fade_ms).range(0..=5000).speed(10).suffix(" ms"));
                color_button(ui, &mut p.color, [59, 111, 216]);
                match row_buttons(ui, i, n, true) {
                    Some(RowEdit::Up) => swap = Some((i, i - 1)),
                    Some(RowEdit::Down) => swap = Some((i, i + 1)),
                    Some(RowEdit::Remove) => remove = Some(i),
                    None => {}
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.draft.pads.remove(i);
        }
        if let Some((a, b)) = swap {
            self.draft.pads.swap(a, b);
        }
        if n == 0 {
            hint(ui, "No pads yet.");
        }
        ui.add_space(10.0);
        hint(ui, "Press a pad to play it, again to stop. The Sound pads input has its own fader, is recorded like any input, and can duck under voices. Commands: pad:1, pad:Applause, pad:1:stop, pads:stop.");
    }

    pub(super) fn syrinscape(&mut self, ui: &mut egui::Ui) {
        title(ui, "Syrinscape", "Start Syrinscape moods and sounds from Cutaway, the Stream Deck, scenes and the rundown.");
        let s = &mut self.draft.syrinscape;
        egui::Grid::new("syrin").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Control Syrinscape");
            ui.checkbox(&mut s.enabled, "on");
            ui.end_row();
            label(ui, "Auth token");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.token).password(true).desired_width(300.0).hint_text("paste your token"));
                ui.hyperlink_to("Get yours", "https://syrinscape.com/account/auth-token/");
            });
            ui.end_row();
        });
        hint(ui, "Needs a Syrinscape Online subscription, with the Online Player open somewhere.");
        if !s.enabled {
            return;
        }
        // A private connection for testing and browsing with the token typed here.
        let token = s.token.trim().to_string();
        if self.syrin.is_none() {
            let ctx = ui.ctx().clone();
            self.syrin = Some(Syrinscape::new(&self.draft.syrinscape, Arc::new(move || ctx.request_repaint())));
        }
        let syrin = self.syrin.as_ref().unwrap();
        syrin.set_token(&token);
        let st = syrin.state.lock_ok().clone();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!token.is_empty() && !st.busy, egui::Button::new("Test")).clicked() {
                self.syrin_tested = true;
                syrin.check();
            }
            if st.busy {
                ui.spinner();
            } else if let Some(p) = &st.problem {
                ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
            } else if self.syrin_tested {
                ui.label(RichText::new("✓ Connected").color(GREEN));
            }
        });

        ui.add_space(14.0);
        subtitle(ui, "Buttons");
        hint(ui, "These appear in the main window's sound panel and as Syrinscape keys on the Stream Deck, numbered in this order.");
        let n = self.draft.syrinscape.buttons.len();
        let mut remove = None;
        let mut swap = None;
        egui::Grid::new("syrin_buttons").num_columns(4).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
            for (i, b) in self.draft.syrinscape.buttons.iter_mut().enumerate() {
                ui.label(RichText::new(format!("{}", i + 1)).monospace());
                ui.add_sized([170.0, 22.0], egui::TextEdit::singleline(&mut b.name));
                let what = match b.kind {
                    SyrinKind::Mood => format!("mood {}", b.id),
                    SyrinKind::Element => format!("sound {}", b.id),
                    SyrinKind::StopAll => "stop everything".into(),
                };
                ui.label(RichText::new(if b.soundset.is_empty() { what } else { format!("{what} · {}", b.soundset) }).color(TEXT_DIM));
                match row_buttons(ui, i, n, true) {
                    Some(RowEdit::Up) => swap = Some((i, i - 1)),
                    Some(RowEdit::Down) => swap = Some((i, i + 1)),
                    Some(RowEdit::Remove) => remove = Some(i),
                    None => {}
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.draft.syrinscape.buttons.remove(i);
        }
        if let Some((a, b)) = swap {
            self.draft.syrinscape.buttons.swap(a, b);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Add from a link:");
            ui.add(egui::TextEdit::singleline(&mut self.syrin_link).desired_width(300.0).hint_text("a mood or sound's remote-control link"));
            let parsed = parse_link(&self.syrin_link);
            if ui.add_enabled(parsed.is_some(), egui::Button::new("Add")).clicked() {
                let (kind, id) = parsed.unwrap();
                let name = match kind {
                    SyrinKind::Mood => format!("Mood {id}"),
                    SyrinKind::Element => format!("Sound {id}"),
                    SyrinKind::StopAll => "Stop".into(),
                };
                self.draft.syrinscape.buttons.push(SyrinButton { name, kind, id, soundset: String::new() });
                self.syrin_link.clear();
            }
            if ui.button("＋ Stop everything").clicked() {
                self.draft.syrinscape.buttons.push(SyrinButton { name: "Stop".into(), kind: SyrinKind::StopAll, id: 0, soundset: String::new() });
            }
        });
        hint(ui, "In the Online Player, a mood or sound's menu has \"Copy remote control links\"; paste one here.");

        ui.add_space(14.0);
        subtitle(ui, "Browse your soundsets");
        let syrin = self.syrin.as_ref().unwrap();
        ui.horizontal(|ui| {
            if ui.add_enabled(!token.is_empty() && !st.busy, egui::Button::new(if st.soundsets.is_some() { "Reload" } else { "Load my soundsets" })).clicked() {
                syrin.load_soundsets();
                self.syrin_soundset = None;
            }
        });
        match &st.soundsets {
            None => {}
            Some(Err(e)) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(AMBER));
            }
            Some(Ok(sets)) => {
                ui.horizontal(|ui| {
                    let current = self.syrin_soundset.clone();
                    let text = current.as_ref().and_then(|u| sets.iter().find(|s| &s.uuid == u)).map(|s| s.name.clone()).unwrap_or_else(|| format!("choose one of {} soundsets", sets.len()));
                    egui::ComboBox::from_id_salt("soundset").selected_text(text).width(340.0).height(400.0).show_ui(ui, |ui| {
                        for set in sets {
                            if ui.selectable_label(current.as_deref() == Some(set.uuid.as_str()), &set.name).clicked() {
                                self.syrin_soundset = Some(set.uuid.clone());
                                syrin.load_items(&set.uuid);
                            }
                        }
                    });
                });
            }
        }
        if let (Some(uuid), Some((for_set, items))) = (&self.syrin_soundset, &st.items) {
            if for_set == uuid {
                let set_name = match &st.soundsets {
                    Some(Ok(sets)) => sets.iter().find(|s| &s.uuid == uuid).map(|s| s.name.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                match items {
                    Err(e) => {
                        ui.label(RichText::new(format!("⚠ {e}")).color(AMBER));
                    }
                    Ok(items) => {
                        let have: Vec<(SyrinKind, u64)> = self.draft.syrinscape.buttons.iter().map(|b| (b.kind, b.id)).collect();
                        egui::ScrollArea::vertical().id_salt("syrin_items").max_height(260.0).show(ui, |ui| {
                            egui::Grid::new("items").num_columns(3).spacing(TABLE_GAP).striped(true).show(ui, |ui| {
                                for it in items {
                                    let kind = if it.kind == SyrinKind::Mood { "mood".to_string() } else if it.what.is_empty() { "sound".to_string() } else { it.what.clone() };
                                    ui.label(RichText::new(kind).color(TEXT_DIM).size(TEXT_BODY));
                                    ui.label(&it.name);
                                    let added = have.contains(&(it.kind, it.id));
                                    if ui.add_enabled(!added, egui::Button::new(if added { "added" } else { "＋ Add" }).small()).clicked() {
                                        self.draft.syrinscape.buttons.push(SyrinButton { name: it.name.clone(), kind: it.kind, id: it.id, soundset: set_name.clone() });
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                    }
                }
            }
        }
        ui.add_space(14.0);
        subtitle(ui, "Hearing it");
        hint(ui, "Syrinscape plays on the computer running its player. To get it into Cutaway, either add an audio input set to \"Sound from an app\" and pick the player (your browser or the Syrinscape app) — Mac and Windows — or route the player to a virtual cable (BlackHole, VB-CABLE) and add that as an input. Tick \"Duck under voices\" on that input to keep it below the talking.");
        hint(ui, "Commands: syrinscape:2, syrinscape:Tavern, syrinscape:stop, syrinscape:volume:0.8. Scenes and rundown segments can start them too.");
    }
}
