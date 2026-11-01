//! Press-and-hold buttons (REC's "hold to stop").
//!
//! A hold is timed on egui's clock (`input.time`), and the widget must sense
//! drags as well as clicks: egui stops reporting a click-only widget as
//! "down" once the press is longer than a click can be (0.8 s) or has moved
//! a few points, so a one-second hold on one never finished.

use eframe::egui::{self, Rect, Response, Sense, Vec2};

/// How a hold-to-act widget senses the pointer. Click and drag: a click
/// alone stops counting as "down" after 0.8 s or 6 points of movement.
pub(super) fn hold_sense() -> Sense {
    Sense::click_and_drag()
}

/// Where a press-and-hold is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Hold {
    /// egui time when the press began.
    since: Option<f64>,
    /// This press already did its thing (so holding on doesn't repeat it).
    spent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum HoldStep {
    /// Not pressed.
    Up,
    /// Let go before the time was up (nothing happens; show the hint).
    LetGo,
    /// Held this far (0 to 1).
    Holding(f32),
    /// Held long enough: do it (once per press).
    Done,
    /// Still held after it was done.
    Spent,
}

impl Hold {
    /// One frame: whether it's held down now, and egui's time.
    pub(super) fn step(&mut self, down: bool, now: f64, secs: f32) -> HoldStep {
        if !down {
            let early = self.since.is_some();
            *self = Hold::default();
            return if early { HoldStep::LetGo } else { HoldStep::Up };
        }
        if self.spent {
            return HoldStep::Spent;
        }
        let since = *self.since.get_or_insert(now);
        let f = (((now - since) as f32) / secs.max(0.001)).clamp(0.0, 1.0);
        if f >= 1.0 {
            self.since = None;
            self.spent = true;
            return HoldStep::Done;
        }
        HoldStep::Holding(f)
    }

    /// Being held (and not done yet).
    pub(super) fn active(&self) -> bool {
        self.since.is_some()
    }

    pub(super) fn reset(&mut self) {
        *self = Hold::default();
    }

    /// Never let it get further than `secs` (the self-test's part-way hold).
    pub(super) fn hold_back(&mut self, now: f64, secs: f64) {
        if let Some(s) = &mut self.since {
            *s = s.max(now - secs);
        }
    }
}

/// Whether a hold widget's press is down on it now: the button is down on
/// it and the pointer is still over it (sliding off lets go, like a button).
pub(super) fn held_on(r: &Response, rect: Rect) -> bool {
    r.is_pointer_button_down_on() && r.interact_pointer_pos().is_some_and(|p| rect.expand(12.0).contains(p))
}

/// A hold area of `size`: allocates it and steps `hold`. (The REC pill
/// paints itself on top.)
pub(super) fn hold_area(ui: &mut egui::Ui, size: Vec2, hold: &mut Hold, secs: f32, also_down: bool) -> (Rect, Response, HoldStep) {
    let (rect, r) = ui.allocate_exact_size(size, hold_sense());
    let down = held_on(&r, rect) || also_down;
    let now = ui.input(|i| i.time);
    let step = hold.step(down, now, secs);
    (rect, r, step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2, Event, PointerButton, RawInput};

    /// Drives a real egui context headlessly: the pointer goes down on the
    /// area at t=0, wobbles a few points while it's held for `hold` seconds
    /// (60 frames a second), then lets go. Returns every step.
    fn press_for(hold_secs: f64, sense: Sense) -> Vec<HoldStep> {
        let ctx = egui::Context::default();
        let mut hold = Hold::default();
        let mut steps = Vec::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let at = pos2(60.0, 30.0);
        let frame_dt = 1.0 / 60.0;
        let total = (hold_secs / frame_dt).round() as usize;
        for i in 0..=total + 3 {
            let t = i as f64 * frame_dt;
            let mut input = RawInput { screen_rect: Some(screen), time: Some(t), predicted_dt: frame_dt as f32, ..Default::default() };
            // A hand on a trackpad: a few points of drift while it's held.
            let wobble = vec2(((i as f32) * 0.37).sin() * 4.0, (i as f32 * 0.11).min(5.0));
            match i {
                0 => input.events.push(Event::PointerMoved(at)),
                1 => input.events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }),
                _ if i == total + 1 => input.events.push(Event::PointerButton { pos: at + wobble, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }),
                _ if i <= total => input.events.push(Event::PointerMoved(at + wobble)),
                _ => {}
            }
            let mut out = ctx.run_ui(input, |ui| {
                let (rect, r) = ui.allocate_exact_size(vec2(150.0, 40.0), sense);
                let down = held_on(&r, rect);
                let now = ui.input(|i| i.time);
                steps.push(hold.step(down, now, 1.0));
            });
            out.textures_delta.clear();
        }
        steps
    }

    /// The REC pill: a 1.3 s hold (with a little movement) stops the take,
    /// once.
    #[test]
    fn a_long_hold_completes() {
        let steps = press_for(1.3, hold_sense());
        assert_eq!(steps.iter().filter(|s| **s == HoldStep::Done).count(), 1, "{steps:?}");
        assert!(steps.contains(&HoldStep::Spent));
        assert!(!steps.contains(&HoldStep::LetGo));
        assert_eq!(steps.last(), Some(&HoldStep::Up));
    }

    /// Half a second does nothing but light the hint.
    #[test]
    fn a_short_press_does_not_stop() {
        let steps = press_for(0.5, hold_sense());
        assert!(!steps.contains(&HoldStep::Done), "{steps:?}");
        assert!(steps.contains(&HoldStep::LetGo));
    }

    /// The bug this fixes: the same hold on a click-only widget never
    /// finishes (egui gives up on the click after 0.8 s).
    #[test]
    fn a_click_only_widget_cannot_be_held() {
        let steps = press_for(1.3, Sense::click());
        assert!(!steps.contains(&HoldStep::Done), "{steps:?}");
    }

    /// The steps on their own, and the self-test's part-way hold.
    #[test]
    fn steps_and_hold_back() {
        let mut h = Hold::default();
        assert_eq!(h.step(true, 10.0, 1.0), HoldStep::Holding(0.0));
        assert!(h.active());
        h.hold_back(10.9, 0.6);
        assert!(matches!(h.step(true, 10.9, 1.0), HoldStep::Holding(f) if (f - 0.6).abs() < 1e-3));
        assert_eq!(h.step(true, 11.95, 1.0), HoldStep::Done);
        assert_eq!(h.step(true, 12.5, 1.0), HoldStep::Spent);
        assert_eq!(h.step(false, 12.6, 1.0), HoldStep::Up);
        h.step(true, 13.0, 1.0);
        assert_eq!(h.step(false, 13.2, 1.0), HoldStep::LetGo);
    }

    /// Sliding off the pill lets go of it.
    #[test]
    fn sliding_off_lets_go() {
        let ctx = egui::Context::default();
        let mut hold = Hold::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let mut last = HoldStep::Up;
        for i in 0..80 {
            let t = i as f64 / 60.0;
            let mut input = RawInput { screen_rect: Some(screen), time: Some(t), ..Default::default() };
            let at = if i < 20 { pos2(60.0, 30.0) } else { pos2(300.0, 150.0) };
            input.events.push(Event::PointerMoved(at));
            if i == 1 {
                input.events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
            }
            let mut out = ctx.run_ui(input, |ui| {
                let (rect, r) = ui.allocate_exact_size(vec2(150.0, 40.0), hold_sense());
                last = hold.step(held_on(&r, rect), ui.input(|i| i.time), 1.0);
                assert_ne!(last, HoldStep::Done);
            });
            out.textures_delta.clear();
        }
        assert_eq!(last, HoldStep::Up);
    }
}
