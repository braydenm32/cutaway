//! Clocks, countdowns and stopwatches: the timers behind clock graphics,
//! Stream Deck keys and the rundown's segment clock.

use crate::util::LockExt;
use crate::config::{ClockKind, Graphic};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
struct Timer {
    since: Option<Instant>,
    banked: Duration,
}

impl Timer {
    fn elapsed(&self, now: Instant) -> Duration {
        self.banked + self.since.map(|s| now.saturating_duration_since(s)).unwrap_or_default()
    }
}

/// The rundown's current segment, for its clock.
#[derive(Clone, Debug)]
pub struct SegmentClock {
    #[allow(dead_code)]
    pub name: String,
    /// Zero: open-ended.
    pub planned: Duration,
    pub started: Instant,
}

#[derive(Default)]
struct Inner {
    named: HashMap<String, Timer>,
    segment: Option<SegmentClock>,
}

/// Timers by name (a clock graphic's name).
#[derive(Default)]
pub struct Timers {
    inner: Mutex<Inner>,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl Timers {
    pub fn new() -> Arc<Timers> {
        Arc::new(Timers::default())
    }

    pub fn start(&self, name: &str) {
        let mut i = self.inner.lock_ok();
        let t = i.named.entry(key(name)).or_default();
        if t.since.is_none() {
            t.since = Some(Instant::now());
        }
    }

    pub fn pause(&self, name: &str) {
        let mut i = self.inner.lock_ok();
        if let Some(t) = i.named.get_mut(&key(name)) {
            if let Some(s) = t.since.take() {
                t.banked += s.elapsed();
            }
        }
    }

    /// Start or pause. Returns whether it's now running.
    pub fn toggle(&self, name: &str) -> bool {
        if self.running(name) {
            self.pause(name);
            false
        } else {
            self.start(name);
            true
        }
    }

    /// Back to the start (keeps running if it was).
    pub fn reset(&self, name: &str) {
        let mut i = self.inner.lock_ok();
        let t = i.named.entry(key(name)).or_default();
        t.banked = Duration::ZERO;
        if t.since.is_some() {
            t.since = Some(Instant::now());
        }
    }

    /// Stop and go back to the start.
    pub fn stop(&self, name: &str) {
        self.inner.lock_ok().named.remove(&key(name));
    }

    pub fn running(&self, name: &str) -> bool {
        self.inner.lock_ok().named.get(&key(name)).is_some_and(|t| t.since.is_some())
    }

    pub fn elapsed(&self, name: &str) -> Duration {
        self.inner.lock_ok().named.get(&key(name)).map(|t| t.elapsed(Instant::now())).unwrap_or_default()
    }

    /// The rundown moved on (None: it stopped).
    pub fn set_segment(&self, segment: Option<(String, u32)>) {
        self.inner.lock_ok().segment = segment.map(|(name, secs)| SegmentClock { name, planned: Duration::from_secs(secs as u64), started: Instant::now() });
    }

    /// The segment running now was renamed or given a new length (the
    /// rundown edited during the show): keep its start, take the new plan,
    /// so the Segment timer graphic and the rundown panel agree.
    pub fn replan_segment(&self, name: &str, secs: u32) {
        if let Some(s) = self.inner.lock_ok().segment.as_mut() {
            s.name = name.to_string();
            s.planned = Duration::from_secs(secs as u64);
        }
    }

    pub fn segment(&self) -> Option<SegmentClock> {
        self.inner.lock_ok().segment.clone()
    }

    /// Pretend a timer started `by` earlier (tests).
    #[cfg(test)]
    pub fn inner_for_tests_shift(&self, name: &str, by: Duration) {
        self.inner.lock_ok().named.entry(key(name)).or_default().banked += by;
    }
}

/// A length as "4:59" or "1:04:59".
pub fn fmt_span(d: Duration, round_up: bool) -> String {
    let ms = d.as_millis() as u64;
    let s = if round_up { ms.div_ceil(1000) } else { ms / 1000 };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The time of day as the show shows it everywhere (the top bar, clock
/// graphics, the second screen, scheduled times): `twelve` is the show's
/// one 12/24-hour setting (numbers audit #8).
pub fn time_format(twelve: bool, seconds: bool) -> &'static str {
    match (twelve, seconds) {
        (false, true) => "%H:%M:%S",
        (false, false) => "%H:%M",
        (true, true) => "%-I:%M:%S %p",
        (true, false) => "%-I:%M %p",
    }
}

/// How long a countdown to a time of day stays at 0:00 once that time
/// has come, before it counts to the same time tomorrow.
pub const UNTIL_HOLD: Duration = Duration::from_secs(3600);

/// When a countdown to `until` ("19:30") ends, seen at `now`: today, or
/// after midnight when that time has gone by (a countdown to 00:30 set at
/// 23:50 counts 40 minutes, numbers audit #11). For an hour after the time
/// comes it stays on today's, so the clock rests at 0:00.
pub fn until_target(until: &str, now: chrono::DateTime<chrono::Local>) -> Option<chrono::DateTime<chrono::Local>> {
    let (h, m) = parse_time_of_day(until)?;
    let on = |day: chrono::NaiveDate| day.and_hms_opt(h, m, 0).and_then(|t| t.and_local_timezone(chrono::Local).earliest());
    let today = on(now.date_naive());
    match today {
        Some(t) if t >= now || (now - t).to_std().unwrap_or_default() <= UNTIL_HOLD => Some(t),
        _ => on(now.date_naive().succ_opt()?),
    }
}

/// "19:30", "7:30 pm" or "7:30pm" as (hour, minute).
pub fn parse_time_of_day(s: &str) -> Option<(u32, u32)> {
    let t = s.trim().to_lowercase().replace(' ', "");
    let (t, pm) = if let Some(x) = t.strip_suffix("pm") {
        (x.to_string(), Some(true))
    } else if let Some(x) = t.strip_suffix("am") {
        (x.to_string(), Some(false))
    } else {
        (t, None)
    };
    let (h, m) = match t.split_once([':', '.']) {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (t.parse::<u32>().ok()?, 0),
    };
    if m > 59 {
        return None;
    }
    let h = match pm {
        Some(true) if (1..=12).contains(&h) => h % 12 + 12,
        Some(false) if (1..=12).contains(&h) => h % 12,
        Some(_) => return None,
        None if h < 24 => h,
        None => return None,
    };
    Some((h, m))
}

/// What a clock graphic shows at `now`: (text, over time).
pub fn clock_text(g: &Graphic, timers: &Timers, now: chrono::DateTime<chrono::Local>) -> (String, bool) {
    match g.clock {
        ClockKind::Time => {
            // (`clock_24h` follows the show's 12/24-hour setting: Config::sync_clock_format.)
            (now.format(time_format(!g.clock_24h, g.show_seconds)).to_string(), false)
        }
        ClockKind::Until => {
            let Some(target) = until_target(&g.until, now) else { return ("--:--".into(), false) };
            (fmt_span((target - now).to_std().unwrap_or_default(), true), false)
        }
        ClockKind::Countdown => {
            let left = Duration::from_secs(g.seconds as u64).saturating_sub(timers.elapsed(&g.name));
            (fmt_span(left, true), false)
        }
        ClockKind::Stopwatch => (fmt_span(timers.elapsed(&g.name), false), false),
        ClockKind::Segment => match timers.segment() {
            None => ("--:--".into(), false),
            Some(s) => {
                let gone = s.started.elapsed();
                if s.planned.is_zero() {
                    (fmt_span(gone, false), false)
                } else if gone <= s.planned {
                    (fmt_span(s.planned - gone, true), false)
                } else {
                    (format!("-{}", fmt_span(gone - s.planned, false)), true)
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn spans_and_times_of_day() {
        assert_eq!(fmt_span(Duration::from_millis(299_200), true), "5:00");
        assert_eq!(fmt_span(Duration::from_millis(299_200), false), "4:59");
        assert_eq!(fmt_span(Duration::from_secs(3_725), false), "1:02:05");
        assert_eq!(parse_time_of_day("19:30"), Some((19, 30)));
        assert_eq!(parse_time_of_day("7:30 PM"), Some((19, 30)));
        assert_eq!(parse_time_of_day("12am"), Some((0, 0)));
        assert_eq!(parse_time_of_day("12:15pm"), Some((12, 15)));
        assert_eq!(parse_time_of_day("25:00"), None);
        assert_eq!(parse_time_of_day("soon"), None);
    }

    #[test]
    fn clocks_show_the_right_thing() {
        let timers = Timers::new();
        let now = chrono::Local.with_ymd_and_hms(2026, 10, 18, 18, 55, 1).unwrap();
        let mut g = Graphic::clock("Clock", ClockKind::Time, "");
        assert_eq!(clock_text(&g, &timers, now).0, "6:55 PM");
        g.clock_24h = true;
        g.show_seconds = true;
        assert_eq!(clock_text(&g, &timers, now).0, "18:55:01");

        g.clock = ClockKind::Until;
        g.until = "7pm".into();
        assert_eq!(clock_text(&g, &timers, now).0, "4:59");
        g.until = "18:00".into();
        assert_eq!(clock_text(&g, &timers, now).0, "0:00", "just passed: rests at 0:00");
        g.until = "17:30".into();
        assert_eq!(clock_text(&g, &timers, now).0, "22:34:59", "long gone: tomorrow's");
        // Across midnight (numbers audit #11).
        let late = chrono::Local.with_ymd_and_hms(2026, 10, 18, 23, 50, 0).unwrap();
        g.until = "00:30".into();
        assert_eq!(clock_text(&g, &timers, late).0, "40:00");
        g.until = "12am".into();
        assert_eq!(clock_text(&g, &timers, late).0, "10:00");
        assert_eq!(until_target("00:30", late).unwrap().date_naive(), late.date_naive().succ_opt().unwrap());
        assert_eq!(until_target("23:55", late).unwrap().date_naive(), late.date_naive());

        g.clock = ClockKind::Countdown;
        g.seconds = 90;
        assert_eq!(clock_text(&g, &timers, now).0, "1:30", "not started");
        timers.start("clock");
        std::thread::sleep(Duration::from_millis(30));
        timers.pause("CLOCK");
        let e = timers.elapsed("Clock");
        assert!(e >= Duration::from_millis(30) && e < Duration::from_millis(2000), "{e:?}");
        assert!(!timers.running("Clock"));
        assert!(timers.toggle("Clock"));
        timers.reset("Clock");
        assert!(timers.running("Clock") && timers.elapsed("Clock") < Duration::from_millis(500));
        timers.stop("Clock");
        assert_eq!(timers.elapsed("Clock"), Duration::ZERO);

        g.clock = ClockKind::Segment;
        assert_eq!(clock_text(&g, &timers, now).0, "--:--");
        timers.set_segment(Some(("Intro".into(), 60)));
        assert_eq!(clock_text(&g, &timers, now), ("1:00".into(), false));
        timers.inner.lock_ok().segment.as_mut().unwrap().started -= Duration::from_secs(75);
        assert_eq!(clock_text(&g, &timers, now), ("-0:15".into(), true));
        // The rundown's length changed mid-segment: same start, new plan.
        timers.replan_segment("Intro", 120);
        assert_eq!(clock_text(&g, &timers, now), ("0:45".into(), false));
    }
}
