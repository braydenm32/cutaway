//! Settings → Switching: transitions, preview/program, automatic switching.

use super::*;
use crate::config::{AutoRule, Direction, TransitionMode};

impl SettingsUi {
    pub(super) fn switching(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Switching", "How scenes change, stingers, fade to black, replays, and switching by itself from the mics.");
        let t = &mut self.draft.transition;
        egui::Grid::new("transitions").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Transition");
            ui.horizontal(|ui| {
                for m in TransitionMode::ALL {
                    ui.selectable_value(&mut t.mode, m, m.label());
                }
            });
            ui.end_row();
            label(ui, "Length");
            ui.add(egui::DragValue::new(&mut t.duration_ms).range(0..=3000).speed(10).suffix(" ms"));
            ui.end_row();
            label(ui, "Wipes and pushes go");
            ui.horizontal(|ui| {
                for d in Direction::ALL {
                    ui.selectable_value(&mut t.direction, d, d.label());
                }
            });
            ui.end_row();
            label(ui, "Dip through");
            ui.horizontal(|ui| {
                let mut rgb = crate::config::parse_color(&t.dip_color).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or([0, 0, 0]);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    t.dip_color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
                if ui.small_button("Black").clicked() {
                    t.dip_color = "#000000".into();
                }
                if ui.small_button("White").clicked() {
                    t.dip_color = "#ffffff".into();
                }
            });
            ui.end_row();
            label(ui, "Preview, then take");
            ui.checkbox(&mut t.preview_mode, "a preview monitor, then Take (Space) or Cut (Enter)");
            ui.end_row();
        });
        ui.add_space(10.0);

        let has_pads = self.draft.audio.iter().any(|a| a.is_pads());
        let st = &mut self.draft.transition.stinger;
        let state = st.file.as_ref().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "none".into());
        part(ui, "stinger", "Stinger", &state, false, |ui| {
            egui::Grid::new("stinger").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Video");
                ui.horizontal(|ui| {
                    let shown = st.file.as_ref().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "none".into());
                    ui.label(shown);
                    if ui.button("Choose…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Video", &["mp4", "mov", "m4v"]).pick_file() {
                            st.file = Some(f);
                        }
                    }
                    if st.file.is_some() && ui.small_button("Clear").clicked() {
                        st.file = None;
                    }
                });
                ui.end_row();
                label(ui, "See-through parts");
                egui::ComboBox::from_id_salt("matte").selected_text(st.matte.label()).width(240.0).show_ui(ui, |ui| {
                    for m in crate::config::Matte::ALL {
                        ui.selectable_value(&mut st.matte, m, m.label());
                    }
                });
                ui.end_row();
                label(ui, "Change scenes at");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut st.cut_ms).range(0..=10_000).speed(10).suffix(" ms"));
                    hint(ui, if st.cut_ms == 0 { "0 = halfway" } else { "from the start, when the video covers the screen" });
                });
                ui.end_row();
                label(ui, "Its sound");
                ui.add(egui::Slider::new(&mut st.volume_db, -40.0..=12.0).suffix(" dB"));
                ui.end_row();
            });
            if let Some(i) = &info.stinger {
                if let Some(p) = &i.problem {
                    ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
                } else if i.loading {
                    hint(ui, "Loading…");
                } else if i.loaded {
                    let matte = i.matte.map(|m| m.label()).unwrap_or("");
                    hint(ui, &format!("{:.1} s, {}×{}, {matte}; scenes change at {:.2} s{}.", i.seconds, i.size.0, i.size.1, i.cut_seconds, if i.has_sound { ", with sound" } else { "" }));
                }
            }
            if !has_pads {
                hint(ui, "Its sound plays through the Sound pads input (add one in Sound pads).");
            }
        });

        let t = &mut self.draft.transition;
        let state = format!("{:.1} s{}", t.ftb_ms as f32 / 1000.0, if t.ftb_audio { ", sound too" } else { "" });
        part(ui, "ftb", "Fade to black", &state, false, |ui| {
            egui::Grid::new("ftb").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Fade length");
                ui.add(egui::DragValue::new(&mut t.ftb_ms).range(0..=10_000).speed(20).suffix(" ms"));
                ui.end_row();
                label(ui, "Sound fades too");
                ui.checkbox(&mut t.ftb_audio, "the recording and the stream");
                ui.end_row();
            });
        });

        let recorded: Vec<String> = self.draft.sources.iter().filter(|s| s.record).map(|s| s.name.clone()).collect();
        let r = &mut self.draft.replay;
        let state = format!("the last {:.0} s", r.seconds);
        part(ui, "replay", "Instant replay", &state, false, |ui| {
            egui::Grid::new("replay").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Replay the last");
                ui.add(egui::Slider::new(&mut r.seconds, 2.0..=30.0).step_by(1.0).suffix(" s"));
                ui.end_row();
                label(ui, "Slow motion");
                ui.add(egui::Slider::new(&mut r.slow_speed, 0.25..=0.75).step_by(0.05).custom_formatter(|v, _| format!("{:.0}% speed", v * 100.0)));
                ui.end_row();
                label(ui, "Shows");
                let shown = match crate::control::ReplayAngle::parse(&r.angle) {
                    crate::control::ReplayAngle::Program => "the program".to_string(),
                    crate::control::ReplayAngle::Quad => "every angle at once".to_string(),
                    crate::control::ReplayAngle::Source(_) => r.angle.clone(),
                };
                egui::ComboBox::from_id_salt("replay_angle").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.angle, String::new(), "the program");
                    for name in &recorded {
                        ui.selectable_value(&mut r.angle, name.clone(), name);
                    }
                    if recorded.len() >= 2 {
                        ui.selectable_value(&mut r.angle, "quad".to_string(), "every angle at once");
                    }
                });
                ui.end_row();
                label(ui, "Label");
                ui.checkbox(&mut r.label, "REPLAY in the corner");
                ui.end_row();
            });
        });

        let scenes: Vec<String> = self.draft.effective_scenes().iter().map(|s| s.name.clone()).collect();
        let inputs: Vec<(String, u32)> = self.draft.audio.iter().map(|a| (a.name.clone(), a.channels)).collect();
        let a = &mut self.draft.auto_switch;
        let state = format!(
            "{} · {}",
            match a.rules.len() {
                0 => "no rules".to_string(),
                1 => "1 rule".to_string(),
                n => format!("{n} rules"),
            },
            if a.enabled { "on at start" } else { "off at start" }
        );
        part(ui, "auto", "Automatic switching", &state, false, |ui| {
            egui::Grid::new("auto_on").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "At start");
                ui.checkbox(&mut a.enabled, "on (AUTO and the A key turn it on and off)");
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.label(RichText::new("When this mic talks, show this scene:").strong());
            let mut remove = None;
            egui::Grid::new("rules").num_columns(5).spacing([10.0, 6.0]).show(ui, |ui| {
                for (i, r) in a.rules.iter_mut().enumerate() {
                    egui::ComboBox::from_id_salt(("rule_in", i)).selected_text(if r.input.is_empty() { "input…" } else { &r.input }).width(150.0).show_ui(ui, |ui| {
                        for (n, _) in &inputs {
                            ui.selectable_value(&mut r.input, n.clone(), n);
                        }
                    });
                    let chans = inputs.iter().find(|x| x.0 == r.input).map(|x| x.1).unwrap_or(2);
                    egui::ComboBox::from_id_salt(("rule_ch", i)).selected_text(if r.channel == 0 { "all channels".to_string() } else { format!("channel {}", r.channel) }).width(110.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.channel, 0, "all channels");
                        for c in 1..=chans {
                            ui.selectable_value(&mut r.channel, c, format!("channel {c}"));
                        }
                    });
                    ui.label("→");
                    egui::ComboBox::from_id_salt(("rule_scene", i)).selected_text(if r.scene.is_empty() { "scene…" } else { &r.scene }).width(170.0).show_ui(ui, |ui| {
                        for s in &scenes {
                            ui.selectable_value(&mut r.scene, s.clone(), s);
                        }
                    });
                    // Live level, to help set the threshold.
                    let level = inputs
                        .iter()
                        .position(|x| x.0 == r.input)
                        .and_then(|ii| info.input_levels.get(ii))
                        .map(|lv| if r.channel == 0 { lv.iter().copied().fold(-120.0, f32::max) } else { lv.get(r.channel as usize - 1).copied().unwrap_or(-120.0) });
                    ui.horizontal(|ui| {
                        if let Some(l) = level {
                            let talking = l >= a.threshold_db;
                            ui.label(RichText::new(format!("{:>4.0} dB", l.max(-99.0))).monospace().color(if talking { GREEN } else { TEXT_DIM }));
                        }
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                a.rules.remove(i);
            }
            if ui.button("＋ Add a rule").clicked() {
                a.rules.push(AutoRule { input: inputs.first().map(|x| x.0.clone()).unwrap_or_default(), channel: 0, scene: scenes.first().cloned().unwrap_or_default() });
            }
            ui.add_space(8.0);
            egui::Grid::new("auto").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                label(ui, "Counts as talking above");
                ui.add(egui::Slider::new(&mut a.threshold_db, -70.0..=-10.0).suffix(" dB"));
                ui.end_row();
                label(ui, "Talking for at least");
                ui.add(egui::DragValue::new(&mut a.hold_ms).range(100..=3000).speed(10).suffix(" ms"));
                ui.end_row();
                label(ui, "Shortest shot");
                ui.add(egui::DragValue::new(&mut a.min_shot_s).range(0.5..=30.0).speed(0.1).suffix(" s"));
                ui.end_row();
                label(ui, "Wide shot");
                egui::ComboBox::from_id_salt("wide").selected_text(if a.wide_scene.is_empty() { "none" } else { &a.wide_scene }).width(200.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.wide_scene, String::new(), "none");
                    for s in &scenes {
                        ui.selectable_value(&mut a.wide_scene, s.clone(), s);
                    }
                });
                ui.end_row();
                label(ui, "When several talk at once");
                ui.checkbox(&mut a.wide_on_crosstalk, "go to the wide shot (otherwise: the loudest)");
                ui.end_row();
                label(ui, "After one person for");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut a.max_shot_s).range(0.0..=600.0).speed(0.5).custom_formatter(|v, _| if v == 0.0 { "never".into() } else { format!("{v:.0} s") }));
                    ui.label("show the wide shot for");
                    ui.add(egui::DragValue::new(&mut a.wide_for_s).range(1.0..=60.0).speed(0.1).suffix(" s"));
                });
                ui.end_row();
                label(ui, "After you switch by hand");
                ui.horizontal(|ui| {
                    ui.label("wait");
                    ui.add(egui::DragValue::new(&mut a.manual_pause_s).range(0.0..=120.0).speed(0.1).suffix(" s"));
                    hint(ui, "before switching automatically again");
                });
                ui.end_row();
            });
            hint(ui, "Give each person their own mic channel; the levels beside the rules are live.");
        });
    }
}
