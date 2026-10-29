//! Settings → Mixing & headphones, and the voice processing strip.

use super::*;
use crate::config::VoiceProcessing;

/// What the channel strip is doing, in a few words.
pub(super) fn processing_summary(p: &VoiceProcessing) -> String {
    if !p.enabled {
        return "off".into();
    }
    let on: Vec<&str> = [
        (p.high_pass_hz >= 20.0, "low cut"),
        (p.noise_reduction, "noise reduction"),
        (p.gate, "gate"),
        (p.eq, "EQ"),
        (p.compressor, "compressor"),
        (p.de_esser, "de-esser"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| n)
    .collect();
    if on.is_empty() { "on, nothing set".into() } else { on.join(", ") }
}

/// The channel strip for one input.
pub(super) fn processing_editor(ui: &mut egui::Ui, p: &mut VoiceProcessing) {
    ui.horizontal(|ui| {
        check(ui, &mut p.enabled, RichText::new("Process this input").strong());
        ui.separator();
        ui.label("Start from:");
        if ui.button("Spoken voice").on_hover_text("High-pass, gentle compression, de-esser").clicked() {
            *p = VoiceProcessing { enabled: true, high_pass_hz: 80.0, compressor: true, comp_threshold_db: -20.0, comp_ratio: 3.0, comp_makeup_db: 4.0, de_esser: true, ..Default::default() };
        }
        if ui.button("Noisy room").on_hover_text("Adds noise reduction").clicked() {
            *p = VoiceProcessing {
                enabled: true,
                high_pass_hz: 100.0,
                noise_reduction: true,
                noise_amount: 1.0,
                compressor: true,
                comp_threshold_db: -20.0,
                comp_ratio: 3.0,
                comp_makeup_db: 4.0,
                de_esser: true,
                ..Default::default()
            };
        }
        if ui.button("Off").clicked() {
            p.enabled = false;
        }
    });
    ui.add_enabled_ui(p.enabled, |ui| {
        egui::Grid::new("fx").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Low cut (rumble)");
            ui.horizontal(|ui| {
                let mut on = p.high_pass_hz >= 20.0;
                if ui.checkbox(&mut on, "").changed() {
                    p.high_pass_hz = if on { 80.0 } else { 0.0 };
                }
                ui.add_enabled(on, egui::Slider::new(&mut p.high_pass_hz, 20.0..=300.0).suffix(" Hz").logarithmic(true));
            });
            ui.end_row();
            label(ui, "Noise reduction");
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.noise_reduction, "");
                let mut pct = p.noise_amount * 100.0;
                if ui.add_enabled(p.noise_reduction, egui::Slider::new(&mut pct, 10.0..=100.0).suffix(" %").text("fans, hum, traffic")).changed() {
                    p.noise_amount = pct / 100.0;
                }
            });
            ui.end_row();
            label(ui, "Noise gate");
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.gate, "");
                ui.add_enabled(p.gate, egui::Slider::new(&mut p.gate_threshold_db, -80.0..=-20.0).suffix(" dB").text("opens above"));
            });
            ui.end_row();
            label(ui, "EQ");
            ui.vertical(|ui| {
                ui.checkbox(&mut p.eq, "");
                ui.add_enabled_ui(p.eq, |ui| {
                    ui.add(egui::Slider::new(&mut p.eq_low_db, -12.0..=12.0).suffix(" dB").text("low (120 Hz)"));
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut p.eq_mid_db, -12.0..=12.0).suffix(" dB").text("mid at"));
                        ui.add(egui::DragValue::new(&mut p.eq_mid_hz).range(200.0..=8000.0).speed(10.0).suffix(" Hz"));
                    });
                    ui.add(egui::Slider::new(&mut p.eq_high_db, -12.0..=12.0).suffix(" dB").text("high (8 kHz)"));
                });
            });
            ui.end_row();
            label(ui, "Compressor");
            ui.vertical(|ui| {
                ui.checkbox(&mut p.compressor, "even out loud and quiet speech");
                ui.add_enabled_ui(p.compressor, |ui| {
                    ui.add(egui::Slider::new(&mut p.comp_threshold_db, -40.0..=0.0).suffix(" dB").text("threshold"));
                    ui.add(egui::Slider::new(&mut p.comp_ratio, 1.0..=10.0).suffix(":1").text("ratio"));
                    ui.add(egui::Slider::new(&mut p.comp_makeup_db, 0.0..=18.0).suffix(" dB").text("make-up gain"));
                });
            });
            ui.end_row();
            label(ui, "De-esser");
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.de_esser, "");
                ui.add_enabled(p.de_esser, egui::Slider::new(&mut p.de_esser_threshold_db, -50.0..=-10.0).suffix(" dB").text("threshold"));
            });
            ui.end_row();
        });
    });
}

impl SettingsUi {
    pub(super) fn mixing(&mut self, ui: &mut egui::Ui) {
        title(ui, "Mixing & headphones", "Protecting the mixes, stream loudness, and what you and the presenters hear.");
        let m = &mut self.draft.mix;
        egui::Grid::new("master").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Limiter");
            ui.horizontal(|ui| {
                ui.checkbox(&mut m.limiter, "stop the mixes clipping, at");
                ui.add_enabled(m.limiter, egui::DragValue::new(&mut m.limiter_ceiling_db).range(-12.0..=0.0).speed(0.1).suffix(" dBFS"));
            });
            ui.end_row();
            label(ui, "Stream loudness");
            ui.horizontal(|ui| {
                ui.checkbox(&mut m.stream_levelling, "keep the stream steady at");
                ui.add_enabled(m.stream_levelling, egui::DragValue::new(&mut m.stream_target_lufs).range(-30.0..=-8.0).speed(0.1).suffix(" LUFS"));
            });
            ui.end_row();
        });
        ui.add_space(10.0);

        let outputs = self.outputs.clone();
        let inputs: Vec<String> = self.draft.audio.iter().map(|a| a.name.clone()).collect();
        let m = &mut self.draft.mix;
        let state = if m.monitor_device.is_empty() { "off".to_string() } else { format!("{} · {}", if m.monitor_device == "default" { "system output" } else { &m.monitor_device }, match m.monitor_source.as_str() {
            "stream" => "the stream mix".to_string(),
            "program" | "" => "the recording mix".to_string(),
            other => format!("{other} solo"),
        }) };
        part(ui, "mix_phones", "Headphones", &state, true, |ui| {
            egui::Grid::new("monitor").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Output");
                let shown = if m.monitor_device.is_empty() { "Off".to_string() } else { m.monitor_device.clone() };
                egui::ComboBox::from_id_salt("monitor_device").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.monitor_device, String::new(), "Off");
                    ui.selectable_value(&mut m.monitor_device, "default".to_string(), "System output");
                    for d in &outputs {
                        ui.selectable_value(&mut m.monitor_device, d.name.clone(), &d.name).on_hover_text(&d.summary);
                    }
                });
                ui.end_row();
                label(ui, "Listen to");
                let label_of = |s: &str| match s {
                    "stream" => "The stream mix".to_string(),
                    "program" | "" => "The recording mix".to_string(),
                    other => format!("{other} (solo)"),
                };
                egui::ComboBox::from_id_salt("monitor_source").selected_text(label_of(&m.monitor_source)).width(300.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.monitor_source, "program".to_string(), "The recording mix");
                    ui.selectable_value(&mut m.monitor_source, "stream".to_string(), "The stream mix");
                    for a in &inputs {
                        ui.selectable_value(&mut m.monitor_source, a.clone(), format!("{a} (solo)"));
                    }
                });
                ui.end_row();
                label(ui, "Volume");
                ui.add(egui::Slider::new(&mut m.monitor_volume_db, -40.0..=12.0).suffix(" dB"));
                ui.end_row();
            });
            if !cfg!(any(target_os = "macos", windows)) {
                hint(ui, "Headphone output needs macOS or Windows.");
            }
        });

        let mixed: Vec<String> = self.draft.audio.iter().filter(|a| a.automix).map(|a| a.name.clone()).collect();
        let state = if !self.draft.mix.automix { "off".to_string() } else if mixed.len() < 2 { "on, tick at least two mics".to_string() } else { mixed.join(", ") };
        part(ui, "mix_automix", "Automatic mic mixing", &state, self.draft.mix.automix, |ui| {
            hint(ui, "For panels and round tables: whoever is talking is up and the other mics go down, so their room noise, echo and breathing don't pile up. Two people talking at once share it. Nothing is cut off: a quiet mic is only ever turned down, never off.");
            egui::Grid::new("automix").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Automatic mixing");
                ui.checkbox(&mut self.draft.mix.automix, "share the level between the mics below");
                ui.end_row();
                label(ui, "Mics");
                ui.horizontal_wrapped(|ui| {
                    for a in self.draft.audio.iter_mut().filter(|a| !a.is_pads()) {
                        ui.checkbox(&mut a.automix, &a.name);
                    }
                    if self.draft.audio.is_empty() {
                        hint(ui, "No audio inputs yet.");
                    }
                });
                ui.end_row();
            });
            if self.draft.mix.automix && mixed.len() == 1 {
                hint(ui, "Tick at least two mics: with one there's nothing to share.");
            }
            hint(ui, "Only the mixes change: each mic's own WAV file is recorded as it was, so you can still mix it yourself afterwards.");
        });

        let ducked: Vec<String> = self.draft.audio.iter().filter(|a| a.duck).map(|a| a.name.clone()).collect();
        let state = if !self.draft.mix.ducking { "off".to_string() } else if ducked.is_empty() { "on, nothing ticked".to_string() } else { format!("{} under voices", ducked.join(", ")) };
        part(ui, "mix_duck", "Ducking", &state, false, |ui| {
            let ducking = self.draft.mix.ducking;
            egui::Grid::new("ducking").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Ducking");
                ui.checkbox(&mut self.draft.mix.ducking, "turn music, pads and ambience down while people talk");
                ui.end_row();
                if ducking {
                    let m = &mut self.draft.mix;
                    label(ui, "Turn down by");
                    ui.add(egui::Slider::new(&mut m.duck_db, -40.0..=-3.0).suffix(" dB"));
                    ui.end_row();
                    label(ui, "When a voice is above");
                    ui.add(egui::Slider::new(&mut m.duck_threshold_db, -60.0..=-10.0).suffix(" dBFS"));
                    ui.end_row();
                    label(ui, "Back up after");
                    ui.add(egui::DragValue::new(&mut m.duck_release_ms).range(100..=5000).speed(10).suffix(" ms of quiet"));
                    ui.end_row();
                    label(ui, "These go down");
                    ui.horizontal_wrapped(|ui| {
                        for a in self.draft.audio.iter_mut() {
                            ui.checkbox(&mut a.duck, &a.name);
                        }
                        if self.draft.audio.is_empty() {
                            hint(ui, "No audio inputs yet.");
                        }
                    });
                    ui.end_row();
                }
            });
        });

        let m = &mut self.draft.mix;
        let state = if m.talkback_input.is_empty() { "no mic chosen".to_string() } else { format!("on {}", m.talkback_input) };
        part(ui, "mix_talk", "Talkback", &state, false, |ui| {
            egui::Grid::new("talkback").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                label(ui, "Your mic");
                let shown = if m.talkback_input.is_empty() { "none".to_string() } else { m.talkback_input.clone() };
                egui::ComboBox::from_id_salt("tb_input").selected_text(shown).width(240.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.talkback_input, String::new(), "none");
                    for n in &inputs {
                        ui.selectable_value(&mut m.talkback_input, n.clone(), n);
                    }
                });
                ui.end_row();
                label(ui, "While talking back");
                ui.add(egui::Slider::new(&mut m.talkback_dim_db, -40.0..=0.0).suffix(" dB").text("dim the rest"));
                ui.end_row();
                label(ui, "Guests hear it");
                ui.checkbox(&mut m.talkback_to_guests, "remote guests too");
                ui.end_row();
            });
        });

        let n_out = self.draft.mix.outputs.len();
        let state = match n_out {
            0 => "none".to_string(),
            _ => self.draft.mix.outputs.iter().map(|o| o.name.clone()).collect::<Vec<_>>().join(", "),
        };
        part(ui, "mix_outs", "More outputs", &state, n_out > 0, |ui| {
            hint(ui, "Presenters' headphones, or a return feed for a caller, with their own voice left out (a mix-minus).");
            ui.add_space(4.0);
            let mut remove = None;
            for i in 0..n_out {
                let o = &mut self.draft.mix.outputs[i];
                egui::Frame::new().fill(RAISED).corner_radius(4.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    egui::Grid::new(("out", i)).num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                        label(ui, "Name");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut o.name).desired_width(200.0));
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.end_row();
                        label(ui, "Device");
                        let shown = if o.device.is_empty() || o.device == "default" { "System output".to_string() } else { o.device.clone() };
                        let devs = &outputs;
                        egui::ComboBox::from_id_salt(("out_dev", i)).selected_text(shown).width(300.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut o.device, "default".to_string(), "System output");
                            for d in devs {
                                ui.selectable_value(&mut o.device, d.name.clone(), &d.name).on_hover_text(&d.summary);
                            }
                        });
                        ui.end_row();
                        label(ui, "Channels");
                        ui.horizontal(|ui| {
                            let max = devs.iter().find(|d| d.name == o.device).map(|d| d.max_channels.max(2)).unwrap_or(2).max(o.right).max(2);
                            let mut pair = (o.left.max(1) - 1) / 2;
                            egui::ComboBox::from_id_salt(("out_ch", i)).selected_text(format!("{} and {}", o.left, o.right)).show_ui(ui, |ui| {
                                for p in 0..max.div_ceil(2) {
                                    ui.selectable_value(&mut pair, p, format!("{} and {}", p * 2 + 1, p * 2 + 2));
                                }
                            });
                            o.left = pair * 2 + 1;
                            o.right = pair * 2 + 2;
                            ui.add(egui::Slider::new(&mut o.volume_db, -40.0..=12.0).suffix(" dB").text("volume"));
                        });
                        ui.end_row();
                        label(ui, "Mix");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut o.mix, "recording".to_string(), "the recording mix");
                            ui.selectable_value(&mut o.mix, "stream".to_string(), "the stream mix");
                        });
                        ui.end_row();
                        label(ui, "Leave out");
                        ui.horizontal_wrapped(|ui| {
                            for n in &inputs {
                                let mut out = o.minus.iter().any(|m| m.eq_ignore_ascii_case(n));
                                if ui.checkbox(&mut out, n).changed() {
                                    o.minus.retain(|m| !m.eq_ignore_ascii_case(n));
                                    if out {
                                        o.minus.push(n.clone());
                                    }
                                }
                            }
                        });
                        ui.end_row();
                        label(ui, "Talkback");
                        ui.checkbox(&mut o.talkback, "heard here");
                        ui.end_row();
                    });
                });
                ui.add_space(6.0);
            }
            if let Some(i) = remove {
                self.draft.mix.outputs.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button("＋ Headphones").clicked() {
                    let name = unique(self.draft.mix.outputs.iter().map(|o| o.name.clone()).collect(), "Presenter headphones");
                    self.draft.mix.outputs.push(crate::config::AudioOutput { name, ..Default::default() });
                }
                if ui.button("＋ Mix-minus for a caller").clicked() {
                    let name = unique(self.draft.mix.outputs.iter().map(|o| o.name.clone()).collect(), "Caller return");
                    self.draft.mix.outputs.push(crate::config::AudioOutput { name, talkback: false, ..Default::default() });
                }
            });
            if !cfg!(any(target_os = "macos", windows)) {
                hint(ui, "Outputs need macOS or Windows.");
            }
        });
    }
}
