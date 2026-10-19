//! Settings → Viewer comments.

use super::*;

impl SettingsUi {
    pub(super) fn comments(&mut self, ui: &mut egui::Ui) {
        title(ui, "Viewer comments", "Read your stream's live chat in the main window and put a viewer's comment on screen with a click (or comment:next from the Stream Deck, MIDI or the remote).");
        let c = &mut self.draft.comments;
        egui::Grid::new("comments").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Twitch channel");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.twitch_channel).desired_width(200.0).hint_text("yourchannel"));
                hint(ui, "no sign-in needed");
            });
            ui.end_row();
            label(ui, "YouTube stream");
            ui.add(egui::TextEdit::singleline(&mut c.youtube).desired_width(360.0).hint_text("https://www.youtube.com/watch?v=… or /live/…"));
            ui.end_row();
            label(ui, "YouTube API key");
            ui.vertical(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.youtube_api_key).password(true).desired_width(360.0));
                hint(ui, "Free: in Google Cloud Console, create a project, enable \"YouTube Data API v3\", then Credentials → Create credentials → API key.");
            });
            ui.end_row();
        });
        if !c.youtube.trim().is_empty() && crate::net::chat::youtube_video_id(&c.youtube).is_none() {
            ui.label(RichText::new("⚠ That doesn't look like a YouTube video or stream address.").color(AMBER));
        }
        ui.add_space(12.0);
        subtitle(ui, "On screen");
        egui::Grid::new("comment_style").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            label(ui, "Position");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut c.x, 0.0..=1.0).text("across"));
                ui.add(egui::Slider::new(&mut c.y, 0.0..=1.0).text("down"));
            });
            ui.end_row();
            label(ui, "Width");
            ui.add(egui::Slider::new(&mut c.width, 0.15..=0.9).custom_formatter(|v, _| format!("{:.0}% of the picture", v * 100.0)));
            ui.end_row();
            label(ui, "Colours");
            ui.horizontal(|ui| {
                for (txt, value, fallback) in [("text", &mut c.text_color, [255u8, 255, 255]), ("box", &mut c.box_color, [27, 31, 42])] {
                    let mut rgb = crate::config::parse_color(value).map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8]).unwrap_or(fallback);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    }
                    ui.label(txt);
                }
                ui.add(egui::Slider::new(&mut c.box_opacity, 0.0..=1.0).text("box opacity"));
            });
            ui.end_row();
            label(ui, "Fade");
            ui.add(egui::DragValue::new(&mut c.fade_ms).range(0..=3000).speed(10).suffix(" ms"));
            ui.end_row();
            label(ui, "Hide after");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.hide_after_s).range(0..=600).suffix(" s"));
                hint(ui, if c.hide_after_s == 0 { "0 = stays until hidden" } else { "then it hides by itself" });
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        hint(ui, "The commenter's name is shown in their chat colour (Twitch) or the platform's colour. Comments are only read, never posted.");
    }
}
