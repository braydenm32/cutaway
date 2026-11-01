//! Graphics › Titles & overlays and its More▾ tabs: logos, watermarks,
//! name titles, clocks and timers, tickers and web pages. Saves as you go.

use super::*;
use crate::config::{ClockKind, Graphic, GraphicKind};
use std::hash::{Hash, Hasher};

fn fingerprint(g: &Graphic, canvas: (u32, u32)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{g:?}{canvas:?}").hash(&mut h);
    h.finish()
}

/// The Graphics area's tabs: Titles & overlays lists every graphic (in
/// the order of their Alt keys); the More▾ tabs show their kind of graphic
/// with their own ＋, or the groups taking turns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GraphicsView {
    /// Every graphic, name titles first in mind (Graphics › Titles & overlays).
    Titles,
    /// Clocks, countdowns and timers.
    Timers,
    /// Graphics taking turns.
    Turns,
    /// Tickers and web pages.
    Web,
}

impl GraphicsView {
    fn shows(self, g: &Graphic) -> bool {
        match self {
            GraphicsView::Titles => true,
            GraphicsView::Timers => g.kind == GraphicKind::Clock,
            GraphicsView::Web => matches!(g.kind, GraphicKind::Ticker | GraphicKind::Web),
            GraphicsView::Turns => false,
        }
    }
}

fn kind_name(g: &Graphic) -> &'static str {
    match g.kind {
        GraphicKind::Image => "logo or picture",
        GraphicKind::Title if g.follow => "name title, follows who's on air",
        GraphicKind::Title => "title",
        GraphicKind::Clock if g.clock.is_timer() => "timer",
        GraphicKind::Clock => "clock",
        GraphicKind::Ticker => "ticker",
        GraphicKind::Web => "web page",
    }
}

/// "Alt+1" (⌥1 on a Mac): the key that shows and hides the i-th graphic.
fn graphic_key(i: usize) -> Option<String> {
    use crate::control::shortcuts::{KeyCombo, DIGITS};
    (i < 9).then(|| KeyCombo { alt: true, ..KeyCombo::plain(DIGITS[i]) }.label())
}

impl SettingsUi {
    pub(super) fn graphics(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let view = self.gview;
        if view == GraphicsView::Turns {
            title(ui, "Taking turns", "Graphics that come on one after another by themselves: sponsors, your social handles, a reminder to subscribe. Each still has its own button too.");
            self.rotations(ui, info);
            return;
        }
        let n = self.draft.graphics.len();
        if self.sel_graphic >= n {
            self.sel_graphic = n.saturating_sub(1);
        }
        match view {
            GraphicsView::Titles if n == 0 => {
                self.graphics_empty(ui);
                return;
            }
            GraphicsView::Titles => self.name_title_line(ui, info),
            GraphicsView::Timers => title(ui, "Clocks & timers", "The time of day, a countdown to a time, a countdown or stopwatch you start, or the time left in the rundown segment."),
            GraphicsView::Web => title(ui, "Tickers & web overlays", "Scrolling headlines, and web pages over the picture: alerts, chat boxes, scoreboards."),
            GraphicsView::Turns => {}
        }
        let shown: Vec<usize> = (0..n).filter(|&i| view.shows(&self.draft.graphics[i])).collect();
        if !shown.contains(&self.sel_graphic) {
            if let Some(&first) = shown.first() {
                self.sel_graphic = first;
            }
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for &i in &shown {
                    let g = &self.draft.graphics[i];
                    let now = info.graphics_now.iter().find(|x| x.name == g.name);
                    let mut sub = kind_name(g).to_string();
                    if let Some(k) = graphic_key(i) {
                        sub = format!("{sub} · {k}");
                    }
                    let on = now.is_some_and(|x| x.on);
                    let mut job = egui::text::LayoutJob::default();
                    if on {
                        job.append("● ", 0.0, egui::TextFormat::simple(egui::FontId::proportional(TEXT_STATE), AMBER));
                    }
                    job.append(&format!("{}\n", g.name), 0.0, egui::TextFormat::simple(egui::FontId::proportional(TEXT_STATE), Color32::WHITE));
                    job.append(&sub, 0.0, egui::TextFormat::simple(egui::FontId::proportional(TEXT_SMALL), TEXT_DIM));
                    let b = egui::Button::selectable(self.sel_graphic == i, job).min_size(vec2(220.0, 40.0));
                    let r = ui.add(b);
                    let r = if on { r.on_hover_text("On screen now") } else { r };
                    if r.clicked() {
                        self.sel_graphic = i;
                        self.graphic_picked = true;
                    }
                }
                ui.add_space(6.0);
                self.graphic_add_buttons(ui, view);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if view == GraphicsView::Titles {
                        if let Some(edit) = list_buttons(ui, n, self.sel_graphic, None) {
                            self.graphic_list_edit(edit);
                        }
                    } else if !shown.is_empty() {
                        // (Moving them is on Titles & overlays, where their keys are in order.)
                        if ui.button("Duplicate").clicked() {
                            self.graphic_list_edit(ListEdit::Duplicate);
                        }
                        if ui.button("Remove").on_hover_text("Undo puts it back").clicked() {
                            self.graphic_list_edit(ListEdit::Remove);
                        }
                    }
                });
            });
            ui.add_space(14.0);
            ui.vertical(|ui| {
                if shown.is_empty() {
                    hint(
                        ui,
                        match view {
                            GraphicsView::Timers => "No clocks or timers yet. Add one on the left: a clock, a countdown to when the show starts, or a timer you start.",
                            _ => "No tickers or web pages yet. Add one on the left.",
                        },
                    );
                } else {
                    self.graphic_detail(ui, info);
                }
            });
        });
    }

    fn graphic_list_edit(&mut self, edit: ListEdit) {
        apply_list_edit(&mut self.draft.graphics, &mut self.sel_graphic, edit, false, |_| None, |g, list| Graphic { name: unique(list.iter().map(|g| g.name.clone()).collect(), &format!("{} copy", g.name)), ..g.clone() });
        self.graphic_picked = true;
    }

    /// A new graphic at the end of the list, picked.
    fn add_graphic(&mut self, g: Graphic) {
        self.draft.graphics.push(g);
        self.sel_graphic = self.draft.graphics.len() - 1;
        self.graphic_picked = true;
    }

    /// The ＋ buttons for a tab's kind of graphic.
    fn graphic_add_buttons(&mut self, ui: &mut egui::Ui, view: GraphicsView) {
        let names: Vec<String> = self.draft.graphics.iter().map(|g| g.name.clone()).collect();
        let wide = |ui: &mut egui::Ui, text: &str| ui.add(egui::Button::new(text).min_size(vec2(220.0, 0.0)));
        match view {
            GraphicsView::Titles => {
                let b = egui::Button::new(RichText::new("＋ Name title").color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(220.0, 0.0));
                if ui.add(b).on_hover_text("A title that names whoever is on air, from the names in Sources › People & names").clicked() {
                    self.pending = Some(SettingsAction::MakeNameTitle);
                }
                if wide(ui, "＋ Fixed title").clicked() {
                    self.add_graphic(Graphic::title(&unique(names.clone(), "Title"), "Your Name", "Role or topic"));
                }
                if wide(ui, "＋ Logo").clicked() {
                    self.add_logo(names.clone());
                }
                ui.horizontal(|ui| {
                    ui.menu_button("＋ More kinds…", |ui| {
                        if ui.button("Clock or countdown").clicked() {
                            self.add_graphic(Graphic::clock(&unique(names.clone(), "Clock"), ClockKind::Time, ""));
                            ui.close();
                        }
                        if ui.button("Scrolling ticker").clicked() {
                            self.add_graphic(Graphic::ticker(&unique(names.clone(), "Ticker"), "Your first headline\nAnd another", ""));
                            ui.close();
                        }
                        if ui.button("Web page").clicked() {
                            self.add_web_page(names.clone());
                            ui.close();
                        }
                    });
                });
            }
            GraphicsView::Timers => {
                for (text, kind, base, label) in [
                    ("＋ Clock", ClockKind::Time, "Clock", ""),
                    ("＋ Countdown to a time", ClockKind::Until, "Starting in", "Starting in"),
                    ("＋ Countdown", ClockKind::Countdown, "Back in", "Back in"),
                    ("＋ Stopwatch", ClockKind::Stopwatch, "Stopwatch", ""),
                ] {
                    if wide(ui, text).clicked() {
                        self.add_graphic(Graphic::clock(&unique(names.clone(), base), kind, label));
                    }
                }
            }
            GraphicsView::Web => {
                if wide(ui, "＋ Scrolling ticker").clicked() {
                    self.add_graphic(Graphic::ticker(&unique(names.clone(), "Ticker"), "Your first headline\nAnd another", ""));
                }
                if wide(ui, "＋ Web page").on_hover_text("A web page or HTML file over the picture, see-through where it's empty: alerts, chat boxes, scoreboards.").clicked() {
                    self.add_web_page(names);
                }
            }
            GraphicsView::Turns => {}
        }
    }

    fn add_logo(&mut self, names: Vec<String>) {
        // Cancelling the picker adds nothing.
        if let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
            self.add_graphic(Graphic { name: unique(names, "Logo"), visible: true, image: f, ..Default::default() });
        }
    }

    fn add_web_page(&mut self, names: Vec<String>) {
        let mut g = Graphic::web(&unique(names, "Web page"), "");
        g.page_size = [self.draft.program.width, self.draft.program.height];
        self.add_graphic(g);
    }

    /// Titles & overlays with no graphics yet (forensics column #4): the
    /// name title first, as on the desk, then the other kinds.
    fn graphics_empty(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        let names: Vec<String> = Vec::new();
        ui.vertical_centered(|ui| {
            egui::Frame::new().fill(RAISED).corner_radius(8.0).inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("No graphics yet").size(TEXT_HEADING).strong().color(Color32::WHITE));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Most shows start with a name title: each person's name and role on screen when they're on air, changing at each cut.").color(TEXT_DIM));
                    ui.add_space(10.0);
                    let b = egui::Button::new(RichText::new("＋ Make a name title").strong().color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(0.0, 32.0));
                    if ui.add(b).on_hover_text("Adds a title that names whoever is on air (Undo takes it away)").clicked() {
                        self.pending = Some(SettingsAction::MakeNameTitle);
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new("Or add").size(TEXT_SMALL).color(TEXT_DIM));
                    ui.horizontal_wrapped(|ui| {
                        if ui.add(egui::Button::new("＋ Fixed title").stroke(egui::Stroke::new(1.0, LINE))).clicked() {
                            self.add_graphic(Graphic::title("Title", "Your Name", "Role or topic"));
                        }
                        if ui.add(egui::Button::new("＋ Logo").stroke(egui::Stroke::new(1.0, LINE))).clicked() {
                            self.add_logo(names.clone());
                        }
                        if ui.add(egui::Button::new("Clocks & timers ›").stroke(egui::Stroke::new(1.0, LINE))).clicked() {
                            self.pending = Some(SettingsAction::Go("graphics/timers/timers"));
                        }
                        if ui.add(egui::Button::new("Tickers & web overlays ›").stroke(egui::Stroke::new(1.0, LINE))).clicked() {
                            self.pending = Some(SettingsAction::Go("graphics/web/web"));
                        }
                    });
                });
            });
        });
    }

    /// Titles & overlays: how the name titles stand, in one line (or the
    /// one button that makes one).
    fn name_title_line(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let total = self.draft.sources.len();
        let named = self.draft.sources.iter().filter(|s| !s.person.trim().is_empty()).count();
        let follow: Vec<(usize, String, u32)> = self.draft.graphics.iter().enumerate().filter(|(_, g)| g.kind == GraphicKind::Title && g.follow).map(|(i, g)| (i, g.name.clone(), g.auto_secs)).collect();
        match follow.first() {
            None => {
                egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("No name title yet: one puts each person's name on screen when they're on air.").color(TEXT_DIM));
                        let b = egui::Button::new(RichText::new("＋ Make a name title").strong().color(Color32::WHITE)).fill(ACCENT.gamma_multiply(0.55)).min_size(vec2(0.0, 28.0));
                        if ui.add(b).on_hover_text("Adds a title that names whoever is on air (Undo takes it away)").clicked() {
                            self.pending = Some(SettingsAction::MakeNameTitle);
                        }
                    });
                });
            }
            Some((i, name, auto)) => {
                ui.horizontal_wrapped(|ui| {
                    let on = info.graphics_now.iter().any(|x| &x.name == name && x.on);
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.5, if on { AMBER } else if named > 0 { GREEN } else { LINE });
                    let what = if *auto > 0 { format!("on by itself for {auto} s for someone new") } else { "on when you press it".to_string() };
                    let r = ui.add(egui::Label::new(RichText::new(format!("Name title \u{201c}{name}\u{201d} follows who's on air · {what}")).color(Color32::WHITE)).sense(egui::Sense::click()));
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show its settings").clicked() {
                        self.sel_graphic = *i;
                        self.graphic_picked = true;
                    }
                    let names = format!("· {named} of {total} pictures have a name");
                    ui.label(RichText::new(names).color(if named == 0 { AMBER } else { TEXT_DIM }));
                    if ui.add(egui::Button::new(RichText::new("Who's in each picture ›").color(ACCENT)).frame(false)).clicked() {
                        self.pending = Some(SettingsAction::Go("sources/people/who"));
                    }
                });
            }
        }
        ui.add_space(8.0);
    }

    fn graphic_detail(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let twelve = self.draft.ui.clock_12h;
        if self.draft.graphics.get(self.sel_graphic).is_none() {
            hint(ui, "No graphics yet. Add a logo (a PNG with a transparent background works best) or a name title.");
            return;
        }
        // What it looks like and what it's doing, first.
        self.graphic_header(ui, info);
        ui.add_space(8.0);
        let Some(g) = self.draft.graphics.get_mut(self.sel_graphic) else { return };
        egui::Grid::new("graphic").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            label(ui, "Name");
            ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut g.name));
            ui.end_row();
            match g.kind {
                GraphicKind::Ticker => {
                    label(ui, "Text");
                    ui.add(egui::TextEdit::multiline(&mut g.title).desired_width(340.0).desired_rows(3).hint_text("one item per line"));
                    ui.end_row();
                    label(ui, "Label");
                    ui.add(egui::TextEdit::singleline(&mut g.subtitle).desired_width(160.0).hint_text("e.g. Coming up (optional)"));
                    ui.end_row();
                    label(ui, "Speed");
                    ui.add(egui::Slider::new(&mut g.speed, 1.0..=20.0).step_by(0.5).suffix(" widths a minute"));
                    ui.end_row();
                    text_style(ui, g);
                }
                GraphicKind::Web => {
                    label(ui, "Address");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut g.url).desired_width(300.0).hint_text("https://… or an HTML file"));
                        if ui.button("Choose file…").clicked() {
                            if let Some(f) = rfd::FileDialog::new().add_filter("Web pages", &["html", "htm"]).pick_file() {
                                g.url = f.display().to_string();
                            }
                        }
                    });
                    ui.end_row();
                    label(ui, "Page size");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut g.page_size[0]).range(16..=3840).suffix(" wide"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut g.page_size[1]).range(16..=2160).suffix(" high"));
                        for (txt, w, h) in [("1080p", 1920, 1080), ("720p", 1280, 720)] {
                            if ui.small_button(txt).clicked() {
                                g.page_size = [w, h];
                            }
                        }
                    });
                    ui.end_row();
                    label(ui, "Drawn by");
                    match crate::engine::web::find_browser_cached() {
                        Some(b) => hint(ui, &format!("{} (hidden; nothing opens on screen)", crate::engine::web::browser_name(&b))),
                        None => {
                            ui.label(RichText::new("⚠ Web pages need Google Chrome or Microsoft Edge installed").color(AMBER));
                        }
                    }
                    ui.end_row();
                }
                GraphicKind::Image => {
                    label(ui, "Image");
                    ui.horizontal(|ui| {
                        let mut text = g.image.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut text).desired_width(280.0)).changed() {
                            g.image = PathBuf::from(text);
                        }
                        if ui.button("Choose…").clicked() {
                            if let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                                g.image = f;
                            }
                        }
                    });
                    ui.end_row();
                }
                GraphicKind::Title => {
                    label(ui, "Names");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut g.follow, false, "the text below");
                        ui.radio_value(&mut g.follow, true, "whoever is on air");
                        if g.follow {
                            // (How many have a name is on the line at the top.)
                            hint(ui, "Their name and role come from Sources › People & names; guests' from the name they type.");
                        }
                    });
                    ui.end_row();
                    if g.follow {
                        label(ui, "By itself");
                        ui.vertical(|ui| {
                            let mut auto = g.auto_secs > 0;
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut auto, "comes on when someone new is on air, for").changed() {
                                    g.auto_secs = if auto { 6 } else { 0 };
                                }
                                ui.add_enabled(auto, egui::DragValue::new(&mut g.auto_secs).range(1..=60).suffix(" s"));
                            });
                            ui.add_enabled_ui(auto, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("not again for the same person for");
                                    ui.add(egui::DragValue::new(&mut g.again_after_min).range(0..=240).suffix(" min"));
                                });
                            });
                        });
                        ui.end_row();
                        label(ui, "Sample");
                    } else {
                        label(ui, "Name line");
                    }
                    ui.add(egui::TextEdit::singleline(&mut g.title).desired_width(300.0));
                    ui.end_row();
                    label(ui, "Second line");
                    ui.add(egui::TextEdit::singleline(&mut g.subtitle).desired_width(300.0));
                    ui.end_row();
                    text_style(ui, g);
                }
                GraphicKind::Clock => {
                    label(ui, "Shows");
                    egui::ComboBox::from_id_salt("clock_kind").selected_text(g.clock.label()).width(240.0).show_ui(ui, |ui| {
                        for k in ClockKind::ALL {
                            ui.selectable_value(&mut g.clock, k, k.label());
                        }
                    });
                    ui.end_row();
                    match g.clock {
                        ClockKind::Time => {
                            label(ui, "Format");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut g.show_seconds, "seconds");
                                hint(ui, if twelve { "12-hour, as the show's clock (Graphics › Clocks & timers, or click the clock in the top bar)" } else { "24-hour, as the show's clock (Graphics › Clocks & timers, or click the clock in the top bar)" });
                            });
                            ui.end_row();
                        }
                        ClockKind::Until => {
                            label(ui, "Counting down to");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut g.until).desired_width(90.0));
                                if crate::engine::timers::parse_time_of_day(&g.until).is_none() {
                                    ui.label(RichText::new("⚠ a time like 19:30 or 7:30 pm").color(AMBER));
                                } else {
                                    hint(ui, "by this computer's clock; once it's gone by, the next one (after midnight)");
                                }
                            });
                            ui.end_row();
                        }
                        ClockKind::Countdown => {
                            label(ui, "From");
                            ui.horizontal(|ui| {
                                let mut m = g.seconds / 60;
                                let mut sec = g.seconds % 60;
                                let a = ui.add(egui::DragValue::new(&mut m).range(0..=599).suffix(" min"));
                                let b = ui.add(egui::DragValue::new(&mut sec).range(0..=59).suffix(" s"));
                                if a.changed() || b.changed() {
                                    g.seconds = m * 60 + sec;
                                }
                            });
                            ui.end_row();
                        }
                        _ => {}
                    }
                    label(ui, "Label");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut g.title).desired_width(200.0).hint_text("none"));
                        for l in ["Starting in", "Back in", "Time left"] {
                            if ui.small_button(l).clicked() {
                                g.title = l.into();
                            }
                        }
                    });
                    ui.end_row();
                    text_style(ui, g);
                }
            }
            label(ui, "Position");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut g.x, 0.0..=1.0).text("across"));
                ui.add(egui::Slider::new(&mut g.y, 0.0..=1.0).text("down"));
            });
            ui.end_row();
            label(ui, "Corner");
            ui.horizontal(|ui| {
                let w = g.width;
                for (txt, x, y) in [("↖", 0.03, 0.04), ("↗", 0.97 - w, 0.04), ("↙", 0.03, 0.80), ("↘", 0.97 - w, 0.80)] {
                    if ui.small_button(txt).clicked() {
                        g.x = x.max(0.0);
                        g.y = y;
                    }
                }
                hint(ui, "quick placement");
            });
            ui.end_row();
            label(ui, "Width");
            ui.add(egui::Slider::new(&mut g.width, 0.02..=1.0).custom_formatter(|v, _| format!("{:.0}% of the picture", v * 100.0)));
            ui.end_row();
            label(ui, "Opacity");
            ui.add(egui::Slider::new(&mut g.opacity, 0.05..=1.0));
            ui.end_row();
            label(ui, "Comes on");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("motion").selected_text(g.motion.label()).show_ui(ui, |ui| {
                    for m in crate::config::Motion::ALL {
                        ui.selectable_value(&mut g.motion, m, m.label());
                    }
                });
                ui.label("taking");
                ui.add(egui::DragValue::new(&mut g.fade_ms).range(0..=3000).speed(10).suffix(" ms"));
            });
            ui.end_row();
            label(ui, "Showing at start");
            ui.checkbox(&mut g.visible, "");
            ui.end_row();
        });
        if g.kind == GraphicKind::Web {
            hint(ui, "The page runs all the time, so it's ready the moment it goes on; ⟳ Load again (here, or beside its button on the desk) loads it again. It's see-through wherever the page has no background. Its sound isn't heard.");
        } else if g.kind == GraphicKind::Ticker {
            hint(ui, "Change the text live from its button on the desk (✎).");
        } else if g.kind == GraphicKind::Clock && g.clock.is_timer() {
            hint(ui, &format!("Start, pause and reset it here, with ▶ and ⟲ beside its button on the desk, a Stream Deck Timer key, or the commands timer:{0}:start, timer:{0}:pause and timer:{0}:reset.", g.name));
        } else if g.kind == GraphicKind::Clock && g.clock == ClockKind::Segment {
            hint(ui, "Counts down the rundown segment that's on, and shows in red how far over it runs.");
        } else {
            hint(ui, "Edit a title's words live from the desk (✎ on the title's button).");
        }
    }

    /// The picked graphic's preview, beside what it's doing now and the
    /// buttons that work it on the show (as its button on the desk does).
    fn graphic_header(&mut self, ui: &mut egui::Ui, info: &SettingsContext) {
        let canvas = (self.draft.program.width, self.draft.program.height);
        let twelve = self.draft.ui.clock_12h;
        let i = self.sel_graphic;
        let Some(mut g) = self.draft.graphics.get(i).cloned() else { return };
        // A title that follows who's on air shows someone named, as on air.
        if g.kind == GraphicKind::Title && g.follow && g.title.trim().is_empty() {
            let (who, role) = self.draft.sources.iter().find(|s| !s.person.trim().is_empty()).map(|s| (s.person.clone(), s.role.clone())).unwrap_or(("Name".into(), "Role".into()));
            g.title = who;
            g.subtitle = role;
        }
        let g = &g;
        let now = info.graphics_now.iter().find(|x| x.name == g.name).cloned();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                // Preview.
                // A clock shows what it shows on air now (the same text as
                // beside it), drawn again as it changes.
                let clock_now = match g.kind {
                    GraphicKind::Clock => Some(match now.as_ref().and_then(|n| n.clock.clone()) {
                        Some(t) => (t.clone(), t.starts_with('-')),
                        None => crate::engine::timers::clock_text(g, &crate::engine::timers::Timers::default(), chrono::Local::now()),
                    }),
                    _ => None,
                };
                let fp = fingerprint(g, canvas) ^ clock_now.as_ref().map(|(t, _)| {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    t.hash(&mut h);
                    h.finish()
                }).unwrap_or(0);
                let key = format!("gfx{}", self.sel_graphic);
                let fresh = self.previews.get(&key).is_none_or(|(f, _)| *f != fp);
                if fresh {
                    let img = match g.kind {
                        GraphicKind::Title => Some(crate::engine::graphics::render_title(g, canvas)),
                        GraphicKind::Image => image::open(&g.image).ok().map(|i| i.to_rgba8()),
                        GraphicKind::Clock => {
                            let (text, over) = clock_now.clone().unwrap_or_default();
                            Some(crate::engine::graphics::render_clock(g, &text, over, canvas))
                        }
                        // (Drawn live by the browser; it shows in the main window.)
                        GraphicKind::Web => None,
                        GraphicKind::Ticker => {
                            // The band with the text where it starts.
                            let mut band = crate::engine::graphics::render_ticker_band(g, canvas);
                            let (strip, _) = crate::engine::graphics::render_ticker_strip(g, canvas);
                            let label_w = crate::engine::graphics::ticker_label_width(g, canvas) as u32;
                            let shown = band.width().saturating_sub(label_w).min(strip.width());
                            let text = image::imageops::crop_imm(&strip, 0, 0, shown, strip.height()).to_image();
                            image::imageops::overlay(&mut band, &text, label_w as i64, 0);
                            Some(band)
                        }
                    };
                    match img {
                        Some(img) => {
                            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                            let tex = ui.ctx().load_texture(&key, ci, egui::TextureOptions::LINEAR);
                            self.previews.insert(key.clone(), (fp, tex));
                        }
                        None => {
                            self.previews.remove(&key);
                        }
                    }
                }
                let pw = if ui.available_width() > 760.0 { 320.0 } else { 260.0 };
                let (rect, _) = ui.allocate_exact_size(vec2(pw, pw * canvas.1 as f32 / canvas.0.max(1) as f32), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(40, 46, 56));
                if let Some((_, tex)) = self.previews.get(&key) {
                    let size = tex.size_vec2();
                    let w = g.width * rect.width();
                    let h = w * size.y / size.x.max(1.0);
                    let r = egui::Rect::from_min_size(rect.min + vec2(g.x * rect.width(), g.y * rect.height()), vec2(w, h));
                    ui.painter().with_clip_rect(rect).image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::from_white_alpha((g.opacity * 255.0) as u8));
                } else if g.kind == GraphicKind::Image {
                    ui.label(RichText::new("⚠ Choose a PNG or JPEG image.").color(AMBER));
                } else if g.kind == GraphicKind::Web {
                    let w = g.width * rect.width();
                    let r = egui::Rect::from_min_size(rect.min + vec2(g.x * rect.width(), g.y * rect.height()), vec2(w, w * g.page_size[1] as f32 / g.page_size[0].max(1) as f32));
                    ui.painter().with_clip_rect(rect).rect_stroke(r, 2.0, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
                    ui.painter().with_clip_rect(rect).text(r.center(), egui::Align2::CENTER_CENTER, "web page", egui::FontId::proportional(TEXT_BODY), ACCENT);
                }
            });
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&g.name).size(TEXT_HEADING).strong().color(Color32::WHITE));
                let mut sub = kind_name(g).to_string();
                if let Some(k) = graphic_key(i) {
                    sub = format!("{sub} · {k} shows and hides it");
                }
                ui.label(RichText::new(sub).size(TEXT_SMALL).color(TEXT_DIM));
                ui.add_space(6.0);
                let Some(now) = now else {
                    hint(ui, "Being saved…");
                    return;
                };
                if let Some(p) = &now.problem {
                    ui.label(RichText::new(format!("⚠ {p}")).color(AMBER));
                }
                if let Some(t) = &now.clock {
                    ui.label(RichText::new(t).size(TEXT_HEADING).monospace().color(Color32::WHITE));
                }
                if g.kind == GraphicKind::Clock && g.clock == ClockKind::Until {
                    if let Some(t) = crate::engine::timers::until_target(&g.until, chrono::Local::now()) {
                        hint(ui, &format!("Counting down to {}", crate::ui::at_time(t, twelve)));
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    let target = crate::control::Target::Name(g.name.clone());
                    let (text, tip) = if now.on { ("Take off", "Take it off the program") } else { ("Show", "Put it on the program now") };
                    let b = egui::Button::new(RichText::new(text).strong().color(if now.on { Color32::BLACK } else { Color32::WHITE })).fill(if now.on { AMBER } else { RAISED }).min_size(vec2(84.0, 28.0));
                    if ui.add(b).on_hover_text(tip).clicked() {
                        self.pending = Some(SettingsAction::Run(crate::control::Action::Graphic(target.clone(), crate::control::Toggle::Toggle)));
                    }
                    if g.kind == GraphicKind::Clock && g.clock.is_timer() {
                        use crate::control::TimerCmd;
                        let (t, cmd) = if now.running { ("⏸ Pause", TimerCmd::Pause) } else { ("▶ Start", TimerCmd::Start) };
                        if ui.add(egui::Button::new(t).min_size(vec2(0.0, 28.0))).clicked() {
                            self.pending = Some(SettingsAction::Run(crate::control::Action::Timer(target.clone(), cmd)));
                        }
                        let back = if g.clock == ClockKind::Countdown { "⟲ Back to the start" } else { "⟲ Back to zero" };
                        if ui.add(egui::Button::new(back).min_size(vec2(0.0, 28.0))).clicked() {
                            self.pending = Some(SettingsAction::Run(crate::control::Action::Timer(target.clone(), TimerCmd::Reset)));
                        }
                    }
                    if g.kind == GraphicKind::Web && ui.add(egui::Button::new("⟳ Load again").min_size(vec2(0.0, 28.0))).on_hover_text("Load the page again (restarts its animations)").clicked() {
                        self.pending = Some(SettingsAction::ReloadGraphic(g.name.clone()));
                    }
                });
            });
        });
    }
}

/// Text colours, box and font (titles and clocks).
fn text_style(ui: &mut egui::Ui, g: &mut Graphic) {
    label(ui, "Colours");
    ui.horizontal(|ui| {
        for (txt, value, fallback) in [("text", &mut g.text_color, [255u8, 255, 255]), ("box", &mut g.box_color, [16, 24, 32])] {
            let mut rgb = crate::config::parse_color(value).map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).unwrap_or(fallback);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            }
            ui.label(txt);
        }
        ui.add(egui::Slider::new(&mut g.box_opacity, 0.0..=1.0).text("box opacity"));
    });
    ui.end_row();
    label(ui, "Font");
    ui.horizontal(|ui| {
        let shown = if g.font.as_os_str().is_empty() { "built in".to_string() } else { g.font.file_name().unwrap_or_default().to_string_lossy().to_string() };
        ui.label(shown);
        if ui.button("Choose…").clicked() {
            if let Some(f) = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf", "ttc"]).pick_file() {
                g.font = f;
            }
        }
        if !g.font.as_os_str().is_empty() && ui.small_button("Built in").clicked() {
            g.font = PathBuf::new();
        }
    });
    ui.end_row();
}
