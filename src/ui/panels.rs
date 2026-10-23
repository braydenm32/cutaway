//! Main-window panels for the show's extras: the rundown, sound pads,
//! Syrinscape and talkback, and the presenters' clocks on the second screen.

use crate::util::LockExt;
use super::theme::*;
use super::App;
use crate::config::{parse_color, ClockKind, GraphicKind, SyrinKind};
use crate::control::rundown::RundownCmd;
use crate::control::{Action, TimerCmd};
use crate::settings::{Section, SettingsUi};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind};
use std::time::Duration;

/// "4:59", or "-0:12" when over.
pub(super) fn left_text(elapsed: f64, planned: f64) -> (String, bool) {
    use crate::engine::timers::fmt_span;
    if planned <= 0.0 {
        (fmt_span(Duration::from_secs_f64(elapsed), false), false)
    } else if elapsed <= planned {
        (fmt_span(Duration::from_secs_f64(planned - elapsed), true), false)
    } else {
        (format!("-{}", fmt_span(Duration::from_secs_f64(elapsed - planned), false)), true)
    }
}

fn color_of(s: &str, fallback: u32) -> Color32 {
    let c = parse_color(s).unwrap_or(fallback);
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl App {
    pub(super) fn section_heading(&mut self, ui: &mut egui::Ui, text: &str, section: Section) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(text).color(TEXT_DIM).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Set up").clicked() {
                    self.settings = Some(SettingsUi::new(self.cfg.clone()).open_at(section));
                }
            });
        });
    }

    pub(super) fn act(&mut self, a: Action) {
        if let Err(e) = self.run_action(a) {
            self.toast(e, true);
        }
    }

    /// Where the show is in its running order, and buttons to move on.
    pub(super) fn rundown_panel(&mut self, ui: &mut egui::Ui) {
        if self.cfg.rundown.segments.is_empty() {
            return;
        }
        ui.add_space(4.0);
        self.section_heading(ui, "RUNDOWN", Section::Rundown);
        let segs = self.cfg.rundown.segments.clone();
        let st = self.rundown.status(&segs);
        let mut cmd = None;
        egui::Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match st.index {
                Some(i) => {
                    let (left, over) = left_text(st.elapsed, st.planned);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}/{}", i + 1, st.count)).color(TEXT_DIM).monospace());
                        ui.add(egui::Label::new(RichText::new(&st.name).strong().size(16.0).color(Color32::WHITE)).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(left).monospace().size(22.0).strong().color(if over { RED } else { Color32::WHITE }));
                        });
                    });
                    if st.planned > 0.0 {
                        let f = (st.elapsed / st.planned).min(1.0) as f32;
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, LINE);
                        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * f, r.height())), 2.0, if f >= 1.0 { RED } else if f > 0.85 { AMBER } else { GREEN });
                    }
                    ui.horizontal(|ui| {
                        let next = st.next.clone().map(|n| format!("Next: {n}")).unwrap_or_else(|| "Last segment".into());
                        ui.add(egui::Label::new(RichText::new(next).color(TEXT_DIM).size(12.0)).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let d = st.drift.round() as i64;
                            let (text, color) = if d.abs() < 5 {
                                ("on time".to_string(), GREEN)
                            } else if d > 0 {
                                (format!("{} over", crate::engine::timers::fmt_span(Duration::from_secs(d as u64), false)), AMBER)
                            } else {
                                (format!("{} under", crate::engine::timers::fmt_span(Duration::from_secs((-d) as u64), false)), TEXT_DIM)
                            };
                            ui.label(RichText::new(text).color(color).size(12.0)).on_hover_text(format!("Show running {}", crate::engine::timers::fmt_span(Duration::from_secs_f64(st.show), false)));
                        });
                    });
                    if let Some(notes) = segs.get(i).map(|s| s.notes.trim().to_string()).filter(|n| !n.is_empty()) {
                        ui.add(egui::Label::new(RichText::new(notes).color(Color32::from_gray(200)).size(12.0)).wrap());
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} segments, {}", st.count, crate::engine::timers::fmt_span(Duration::from_secs(crate::control::rundown::planned_total(&segs)), false))).color(TEXT_DIM));
                    });
                    ui.label(RichText::new(format!("First: {}", segs[0].name)).color(Color32::WHITE));
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let running = st.index.is_some();
                if ui.add_enabled(running && st.index != Some(0), egui::Button::new("◀")).on_hover_text("Previous segment (Shift+N)").clicked() {
                    cmd = Some(RundownCmd::Previous);
                }
                let next_text = if running { "Next ▶" } else { "Start ▶" };
                let can_next = !running || st.next.is_some();
                if ui.add_enabled(can_next, egui::Button::new(RichText::new(next_text).strong()).fill(if can_next { ACCENT } else { RAISED }).min_size(vec2(84.0, 0.0))).on_hover_text(format!("Next segment{}", self.key_hint(crate::control::shortcuts::Builtin::NextSegment))).clicked() {
                    cmd = Some(RundownCmd::Next);
                }
                if ui.add_enabled(running, egui::Button::new("■")).on_hover_text("Stop the rundown").clicked() {
                    cmd = Some(RundownCmd::Stop);
                }
                ui.menu_button("Go to ▾", |ui| {
                    for (i, s) in segs.iter().enumerate() {
                        let text = format!("{}. {}", i + 1, s.name);
                        if ui.selectable_label(st.index == Some(i), text).clicked() {
                            cmd = Some(RundownCmd::Go(crate::control::Target::Index(i)));
                            ui.close();
                        }
                    }
                });
            });
        });
        if let Some(c) = cmd {
            self.act(Action::Rundown(c));
        }
        ui.add_space(4.0);
        ui.separator();
    }

    /// Remote guests: who's on, and their links.
    pub(super) fn guests_panel(&mut self, ui: &mut egui::Ui) {
        let hub = crate::engine::guests::hub();
        let guests = hub.guests();
        if guests.is_empty() || !self.cfg.guests.enabled {
            return;
        }
        ui.add_space(4.0);
        let phones = guests.iter().filter(|g| g.camera).count();
        let heading = if phones == 0 { "GUESTS" } else if phones == guests.len() { "PHONE CAMERAS" } else { "GUESTS & PHONES" };
        self.section_heading(ui, heading, Section::Guests);
        let reach = hub.reach();
        match &reach {
            crate::net::tunnel::TunnelState::Up(_) => {}
            crate::net::tunnel::TunnelState::Starting(s) => {
                ui.label(RichText::new(s).color(TEXT_DIM).size(12.0));
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
            crate::net::tunnel::TunnelState::Failed(e) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(AMBER).size(12.0));
            }
            crate::net::tunnel::TunnelState::Off => {}
        }
        let mut kick = None;
        let mut copied = None;
        let mut qr = None;
        for g in &guests {
            ui.horizontal(|ui| {
                let (dot, color) = if g.connected { ("●", GREEN) } else if g.connecting { ("●", AMBER) } else { ("○", TEXT_DIM) };
                ui.label(RichText::new(dot).color(color));
                ui.label(RichText::new(&g.source).strong());
                let mut detail = if g.screen {
                    if g.connected { "sharing" } else if g.connecting { "starting…" } else { "not sharing" }.to_string()
                } else if g.connected && g.camera {
                    "phone on".to_string()
                } else if g.connected {
                    let t = g.since.map(|s| crate::engine::timers::fmt_span(s.elapsed(), false)).unwrap_or_default();
                    format!("{} {t}", if g.guest_name.is_empty() { "on".to_string() } else { g.guest_name.clone() })
                } else if g.connecting {
                    if g.camera { "connecting…" } else { "joining…" }.into()
                } else if g.camera {
                    "no phone yet".into()
                } else {
                    "waiting".into()
                };
                if let Some(b) = &g.backup {
                    let mb = format!("{:.0} MB", b.bytes as f64 / 1e6);
                    detail.push_str(&if b.done { format!(" · backup ✓ {mb}") } else if b.recording { format!(" · backup {mb}") } else { format!(" · backup coming {mb}") });
                }
                if let Some((q, note)) = &g.link {
                    let (rect, r) = ui.allocate_exact_size(vec2(16.0, 12.0), Sense::hover());
                    super::waiting::link_bars(ui.painter(), rect, *q);
                    let word = match q {
                        crate::engine::guests::LinkQuality::Good => "Connection good".to_string(),
                        crate::engine::guests::LinkQuality::Weak => format!("Weak connection: {note}"),
                        crate::engine::guests::LinkQuality::Poor => format!("Poor connection: {note}. Their picture may freeze on air."),
                    };
                    r.on_hover_text(word);
                }
                let r = ui.add(egui::Label::new(RichText::new(detail).color(TEXT_DIM).size(12.0)).truncate());
                if let Some(b) = &g.backup {
                    r.on_hover_text(format!("Their backup recording: {}", b.file.display()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if g.connected || g.connecting {
                        let tip = if g.screen { "Stop their screen share" } else if g.camera { "Disconnect the phone" } else { "Disconnect them" };
                        if ui.small_button("✕").on_hover_text(tip).clicked() {
                            kick = Some(g.id.clone());
                        }
                    }
                    // (A screen is shared from the guest's own link.)
                    if let Some(link) = hub.link(&g.id).filter(|_| !g.screen) {
                        if g.camera {
                            if ui.small_button("QR").on_hover_text("Show the code to scan with the phone").clicked() {
                                qr = Some(g.id.clone());
                            }
                        } else if ui.small_button("Copy link").on_hover_text(&link).clicked() {
                            ui.ctx().copy_text(link);
                            copied = Some(g.source.clone());
                        }
                    }
                });
            });
        }
        if let Some(id) = qr {
            self.qr_for = if self.qr_for.as_deref() == Some(id.as_str()) { None } else { Some(id) };
        }
        if let Some(id) = self.qr_for.clone() {
            let name = guests.iter().find(|g| g.id == id).map(|g| g.source.clone()).unwrap_or_default();
            let mut open = true;
            match hub.link(&id) {
                Some(link) => {
                    egui::Window::new(format!("{name}: scan with the phone")).id(egui::Id::unique("phone_qr")).open(&mut open).collapsible(false).resizable(false).show(ui.ctx(), |ui| {
                        crate::settings::qr_code(ui, &link, 260.0);
                        ui.label(RichText::new(&link).monospace().size(11.0).color(TEXT_DIM));
                        ui.label(RichText::new("Open it in the phone's camera app, allow the camera and hold the phone sideways.").size(12.0).color(TEXT_DIM));
                    });
                }
                None => open = false,
            }
            if !open || guests.iter().any(|g| g.id == id && g.connected) {
                self.qr_for = None;
            }
        }
        if guests.iter().any(|g| g.connected) {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        if let Some(id) = kick {
            hub.kick(&id);
        }
        if let Some(name) = copied {
            self.toast(format!("Copied {name}'s link: send it to them"), false);
        }
        ui.add_space(4.0);
        ui.separator();
    }

    /// Sound pads, Syrinscape buttons and the talkback button.
    pub(super) fn sound_panel(&mut self, ui: &mut egui::Ui) {
        let Some(engine) = &self.engine else { return };
        let pads = engine.pads.state();
        let talk_ok = engine.mix.talkback_input().is_some();
        let talking = engine.talkback();
        let syrin_on = self.cfg.syrinscape.enabled && !self.cfg.syrinscape.buttons.is_empty();
        if pads.is_empty() && !syrin_on && !talk_ok {
            return;
        }
        ui.add_space(4.0);
        self.section_heading(ui, "SOUND", Section::Pads);
        let mut press: Option<usize> = None;
        let mut release: Option<usize> = None;
        let mut stop_all = false;
        if !pads.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, p) in pads.iter().enumerate() {
                    let size = vec2(92.0, 42.0);
                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
                    let painter = ui.painter_at(rect);
                    let base = color_of(&p.color, 0x3b6fd8);
                    let dim = base.gamma_multiply(0.35);
                    painter.rect_filled(rect, 6.0, if p.playing { dim } else { RAISED });
                    if p.playing {
                        let w = rect.width() * p.progress.clamp(0.0, 1.0);
                        painter.rect_filled(Rect::from_min_size(rect.min, vec2(w, rect.height())), 6.0, base);
                    }
                    painter.rect_stroke(rect, 6.0, Stroke::new(if p.playing { 2.0 } else { 1.0 }, base), StrokeKind::Inside);
                    let name: String = p.name.chars().take(14).collect();
                    let text_color = if p.problem.is_some() { AMBER } else { Color32::WHITE };
                    painter.text(rect.left_top() + vec2(7.0, 6.0), Align2::LEFT_TOP, name, FontId::proportional(13.0), text_color);
                    let corner = if p.playing {
                        format!("{:.0}s", p.remaining.ceil())
                    } else if p.loading {
                        "…".into()
                    } else if i < 12 {
                        format!("F{}", i + 1)
                    } else {
                        String::new()
                    };
                    painter.text(rect.right_bottom() + vec2(-7.0, -5.0), Align2::RIGHT_BOTTOM, corner, FontId::monospace(11.0), Color32::from_white_alpha(180));
                    // Sound starts the moment the button goes down.
                    let down = resp.is_pointer_button_down_on();
                    if down && self.pad_held != Some(i) {
                        self.pad_held = Some(i);
                        press = Some(i);
                    } else if !down && self.pad_held == Some(i) {
                        self.pad_held = None;
                        release = Some(i);
                    }
                    let tip = match &p.problem {
                        Some(problem) => format!("⚠ {problem}"),
                        None => format!("{}{}", p.name, if i < 12 { format!(" (F{})", i + 1) } else { String::new() }),
                    };
                    resp.on_hover_text(tip);
                }
                if pads.iter().any(|p| p.playing) && ui.add(egui::Button::new("■ Stop").min_size(vec2(0.0, 42.0))).on_hover_text("Stop every pad (with their fades)").clicked() {
                    stop_all = true;
                }
            });
            if pads.iter().any(|p| p.playing) {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(i) = press {
            self.act(Action::Pad(crate::control::Target::Index(i), crate::control::PadCmd::Press));
        }
        if let Some(i) = release {
            self.act(Action::Pad(crate::control::Target::Index(i), crate::control::PadCmd::Release));
        }
        if stop_all {
            self.act(Action::PadsStop);
        }

        // Syrinscape.
        if syrin_on {
            ui.add_space(4.0);
            let st = self.syrinscape.state.lock_ok().clone();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Syrinscape").color(TEXT_DIM).size(11.0));
                if st.busy {
                    ui.spinner();
                }
                if let Some(p) = &st.problem {
                    ui.label(RichText::new("⚠").color(AMBER)).on_hover_text(p);
                }
            });
            let buttons = self.cfg.syrinscape.buttons.clone();
            let mut pressed = None;
            ui.horizontal_wrapped(|ui| {
                for (i, b) in buttons.iter().enumerate() {
                    let playing = self.syrinscape.is_playing(b);
                    let text = RichText::new(&b.name).size(12.5).color(if playing { Color32::BLACK } else { Color32::WHITE });
                    let fill = match (b.kind, playing) {
                        (SyrinKind::StopAll, _) => RED_DIM,
                        (_, true) => Color32::from_rgb(150, 110, 220),
                        _ => RAISED,
                    };
                    let what = match b.kind {
                        SyrinKind::Mood => "mood",
                        SyrinKind::Element => "sound (press again to stop)",
                        SyrinKind::StopAll => "stops everything",
                    };
                    if ui.add(egui::Button::new(text).fill(fill).min_size(vec2(0.0, 28.0))).on_hover_text(format!("{} — {what}", b.name)).clicked() {
                        pressed = Some(i);
                    }
                }
            });
            if let Some(i) = pressed {
                self.act(Action::Syrinscape(crate::control::syrinscape::SyrinCmd::Button(crate::control::Target::Index(i))));
            }
        }

        // Talkback: hold to talk.
        if talk_ok {
            ui.add_space(6.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(220.0), 34.0), Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 17.0, if talking { AMBER } else { RAISED });
            painter.rect_stroke(rect, 17.0, Stroke::new(1.0, if talking { AMBER } else { LINE }), StrokeKind::Inside);
            painter.text(rect.center(), Align2::CENTER_CENTER, if talking { "TALKING" } else { "TALK  (hold, or B)" }, FontId::proportional(13.5), if talking { Color32::BLACK } else { Color32::WHITE });
            let down = resp.is_pointer_button_down_on();
            if down != self.talk_held {
                self.talk_held = down;
                self.act(Action::Talkback(if down { crate::control::Toggle::On } else { crate::control::Toggle::Off }));
            }
            resp.on_hover_text("Your mic in the presenters' (and guests') ears while held — never in the recording or stream");
        }
        ui.add_space(4.0);
        ui.separator();
    }

    /// Keys for the extras: F1–F12 pads, B talkback, N rundown, K clip.
    /// Right-click menu on a clock graphic's button: its timer.
    pub(super) fn clock_menu(&mut self, resp: &egui::Response, i: usize) {
        let Some(g) = self.cfg.graphics.get(i).cloned() else { return };
        if g.kind != GraphicKind::Clock || !g.clock.is_timer() {
            return;
        }
        let running = self.engine.as_ref().is_some_and(|e| e.graphics.timers.running(&g.name));
        let mut cmd = None;
        resp.context_menu(|ui| {
            ui.label(RichText::new(format!("{} ({})", g.name, g.clock.label().to_lowercase())).strong());
            if ui.button(if running { "Pause" } else { "Start" }).clicked() {
                cmd = Some(if running { TimerCmd::Pause } else { TimerCmd::Start });
                ui.close();
            }
            if ui.button(if g.clock == ClockKind::Countdown { "Back to the start" } else { "Back to zero" }).clicked() {
                cmd = Some(TimerCmd::Reset);
                ui.close();
            }
            if ui.button("Stop and reset").clicked() {
                cmd = Some(TimerCmd::Stop);
                ui.close();
            }
        });
        if let Some(c) = cmd {
            self.act(Action::Timer(crate::control::Target::Index(i), c));
        }
    }

    /// The presenters' clocks over the second screen's picture.
    pub(super) fn draw_display_clocks(painter: &egui::Painter, area: Rect, segment: Option<(String, f64, f64)>, recording: Option<Duration>) {
        let now = chrono::Local::now();
        let scale = (area.height() / 1080.0).clamp(0.7, 2.0);
        let big = FontId::monospace(54.0 * scale);
        let small = FontId::proportional(22.0 * scale);
        let pad = 18.0 * scale;
        let boxed = |anchor: egui::Pos2, align: Align2, lines: Vec<(String, FontId, Color32)>| {
            let galleys: Vec<_> = lines.into_iter().map(|(t, f, c)| painter.layout_no_wrap(t, f, c)).collect();
            let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
            let h: f32 = galleys.iter().map(|g| g.size().y).sum();
            let rect = align.anchor_size(anchor, vec2(w + pad * 2.0, h + pad * 1.4));
            painter.rect_filled(rect, 10.0 * scale, Color32::from_black_alpha(235));
            let mut y = rect.top() + pad * 0.7;
            for g in galleys {
                let x = match align.x() {
                    egui::Align::Min => rect.left() + pad,
                    egui::Align::Max => rect.right() - pad - g.size().x,
                    egui::Align::Center => rect.center().x - g.size().x / 2.0,
                };
                let gh = g.size().y;
                painter.galley(pos2(x, y), g, Color32::WHITE);
                y += gh;
            }
        };
        let mut left = vec![(now.format("%-I:%M:%S %p").to_string(), big.clone(), Color32::WHITE)];
        if let Some(r) = recording {
            left.push((format!("● REC {}", crate::engine::timers::fmt_span(r, false)), small.clone(), RED));
        }
        boxed(area.left_top() + vec2(pad, pad), Align2::LEFT_TOP, left);
        if let Some((name, elapsed, planned)) = segment {
            let (t, over) = left_text(elapsed, planned);
            // Left of the full-screen button's corner.
            boxed(area.right_top() + vec2(-76.0, pad), Align2::RIGHT_TOP, vec![(name, small, Color32::from_gray(220)), (t, big, if over { RED } else { Color32::WHITE })]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_text() {
        assert_eq!(left_text(10.0, 70.0), ("1:00".into(), false));
        assert_eq!(left_text(75.5, 60.0), ("-0:15".into(), true));
        assert_eq!(left_text(75.5, 0.0), ("1:15".into(), false));
    }
}
