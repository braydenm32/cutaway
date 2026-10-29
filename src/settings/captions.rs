//! Settings → Captions: live captions from speech recognition, and
//! transcripts with each take.

use super::*;
use crate::config::{CaptionPlace, SpeechModel};

impl SettingsUi {
    pub(super) fn captions(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        title(ui, "Captions", "Live captions from what's said, recognised on this computer, and a transcript with every take.");
        let (lang, model) = (self.draft.captions.language.clone(), self.draft.captions.model);
        if let Some(p) = crate::platform::speech::problem(&lang, model) {
            ui.label(RichText::new(format!("⚠ Live captions can't run yet: {p}.")).color(AMBER));
            ui.add_space(6.0);
        }
        if self.languages.is_none() {
            self.languages = Some(crate::platform::speech::languages());
        }
        let languages = self.languages.clone().unwrap_or_default();
        let inputs: Vec<String> = self.draft.audio.iter().map(|a| a.name.clone()).collect();
        let c = &mut self.draft.captions;
        egui::Grid::new("captions").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Live captions");
            ui.checkbox(&mut c.enabled, "listen and caption, on this computer");
            ui.end_row();
            if c.enabled {
                label(ui, "Status");
                match &info.captions_status {
                    Some(s) if s == "listening" => ui.label(RichText::new("● listening").color(GREEN)),
                    Some(s) => ui.label(RichText::new(s).color(AMBER)),
                    None => ui.label(RichText::new("starts when you press Apply").color(TEXT_DIM)),
                };
                ui.end_row();
            }
            label(ui, "Listen to");
            let shown = match c.source.as_str() {
                "" | "recording" => "the recording mix".to_string(),
                "stream" => "the stream mix".to_string(),
                n => n.to_string(),
            };
            egui::ComboBox::from_id_salt("cap_src").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.source, "recording".to_string(), "the recording mix");
                ui.selectable_value(&mut c.source, "stream".to_string(), "the stream mix");
                ui.separator();
                for n in &inputs {
                    ui.selectable_value(&mut c.source, n.clone(), n);
                }
            });
            ui.end_row();
            label(ui, "Language");
            let default_name = if cfg!(target_os = "macos") { "this computer's" } else { "English" };
            let current = if c.language.is_empty() { default_name.to_string() } else { languages.iter().find(|l| l.0 == c.language).map(|l| l.1.clone()).unwrap_or_else(|| c.language.clone()) };
            egui::ComboBox::from_id_salt("cap_lang").selected_text(current).width(260.0).height(360.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.language, String::new(), default_name);
                for (id, name) in &languages {
                    ui.selectable_value(&mut c.language, id.clone(), name);
                }
            });
            ui.end_row();
            #[cfg(all(not(target_os = "macos"), feature = "whisper"))]
            {
                use crate::platform::whisper;
                label(ui, "Speech model");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        for (m, name) in [(SpeechModel::Fast, "Fast"), (SpeechModel::Accurate, "Accurate")] {
                            let f = whisper::model_file(&c.language, m);
                            ui.selectable_value(&mut c.model, m, format!("{name} ({} MB)", f.megabytes));
                        }
                    });
                    let f = whisper::model_file(&c.language, c.model);
                    match (whisper::installed(&c.language, c.model), whisper::download_state(&c.language, c.model)) {
                        (Some(_), _) => {
                            ui.label(RichText::new(format!("✓ downloaded ({})", f.file)).color(GREEN));
                        }
                        (None, Some(d)) if d.running => {
                            let k = if d.total > 0 { d.got as f32 / d.total as f32 } else { 0.0 };
                            ui.add(egui::ProgressBar::new(k).desired_width(280.0).text(format!("downloading… {} MB", d.got / 1_000_000)));
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                        }
                        (None, d) => {
                            ui.horizontal(|ui| {
                                if ui.button(format!("Download ({} MB)", f.megabytes)).clicked() {
                                    whisper::download(&c.language, c.model);
                                }
                                if let Some(e) = d.and_then(|d| d.error) {
                                    ui.label(RichText::new(e).color(AMBER).size(TEXT_BODY));
                                }
                            });
                        }
                    }
                });
                ui.end_row();
            }
            #[cfg(not(all(not(target_os = "macos"), feature = "whisper")))]
            let _ = SpeechModel::Fast;
            label(ui, "Show on");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut c.on_recording, false, "the stream only");
                ui.selectable_value(&mut c.on_recording, true, "the stream and the recording");
            });
            ui.end_row();
            label(ui, "On screen at start");
            ui.checkbox(&mut c.show, "showing");
            ui.end_row();
            label(ui, "Look");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut c.size, 0.025..=0.08).custom_formatter(|v, _| format!("{:.0}% of the height", v * 100.0)));
                ui.label("lines");
                ui.add(egui::DragValue::new(&mut c.lines).range(1..=3));
                ui.selectable_value(&mut c.place, CaptionPlace::Bottom, "bottom");
                ui.selectable_value(&mut c.place, CaptionPlace::Top, "top");
            });
            ui.end_row();
            label(ui, "Transcript");
            ui.checkbox(&mut c.transcript, "with each take (text, .srt and .vtt)");
            ui.end_row();
        });
        if cfg!(target_os = "macos") {
            hint(ui, "macOS asks for permission the first time.");
        }
    }
}
