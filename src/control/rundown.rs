//! The rundown: the show's running order, one segment at a time. Going to
//! a segment takes its scene and runs its cues; the producer sees how long
//! it has left and whether the show is running over or under.

use crate::config::Segment;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum RundownCmd {
    /// The first segment (starts the show clock).
    Start,
    Next,
    Previous,
    /// A segment by number (from 0) or name.
    Go(super::Target),
    Stop,
}

impl RundownCmd {
    pub fn parse(s: &str) -> RundownCmd {
        match s.trim().to_lowercase().as_str() {
            "" | "next" => RundownCmd::Next,
            "start" | "first" | "top" => RundownCmd::Start,
            "previous" | "prev" | "back" => RundownCmd::Previous,
            "stop" | "end" | "off" => RundownCmd::Stop,
            _ => RundownCmd::Go(super::Target::parse(s)),
        }
    }
}

/// Where the show is, for the window, the remote and the Stream Deck.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RundownStatus {
    pub index: Option<usize>,
    pub name: String,
    pub next: Option<String>,
    /// Seconds into this segment.
    pub elapsed: f64,
    /// Seconds planned for it (0: open-ended).
    pub planned: f64,
    /// Seconds over (+) or under (-) the plan for the show so far.
    pub drift: f64,
    /// Seconds since the show (the first segment) started.
    pub show: f64,
    pub count: usize,
}

#[derive(Default)]
pub struct Rundown {
    current: Option<usize>,
    started: Option<Instant>,
    show_started: Option<Instant>,
    /// Finished segments and how long they really ran.
    done: Vec<(usize, Duration)>,
}

impl Rundown {
    /// Where a command goes: Some(Some(i)) to segment i, Some(None) to
    /// stop, None for nowhere (the end, or no such segment).
    pub fn target(&self, cmd: &RundownCmd, segments: &[Segment]) -> Option<Option<usize>> {
        let n = segments.len();
        if n == 0 {
            return None;
        }
        match cmd {
            RundownCmd::Start => Some(Some(0)),
            RundownCmd::Next => match self.current {
                None => Some(Some(0)),
                Some(i) if i + 1 < n => Some(Some(i + 1)),
                Some(_) => None,
            },
            RundownCmd::Previous => match self.current {
                Some(i) if i > 0 => Some(Some(i - 1)),
                _ => None,
            },
            RundownCmd::Go(t) => {
                let names: Vec<String> = segments.iter().map(|s| s.name.clone()).collect();
                t.find(&names).map(Some)
            }
            RundownCmd::Stop => self.current.map(|_| None),
        }
    }

    /// Move to segment `to` (None: stop) now.
    pub fn go(&mut self, to: Option<usize>) {
        let now = Instant::now();
        if let (Some(i), Some(s)) = (self.current, self.started) {
            self.done.push((i, now - s));
        }
        match to {
            Some(i) => {
                if self.current.is_none() {
                    // Not running: a fresh show.
                    self.show_started = Some(now);
                    self.done.clear();
                }
                self.current = Some(i);
                self.started = Some(now);
            }
            None => {
                self.current = None;
                self.started = None;
            }
        }
    }

    pub fn status(&self, segments: &[Segment]) -> RundownStatus {
        let now = Instant::now();
        let mut st = RundownStatus { count: segments.len(), ..Default::default() };
        // Over or under: each finished segment's real length against its
        // plan (open-ended ones count as planned), and this one once it's over.
        let mut drift = 0.0;
        for (i, d) in &self.done {
            let planned = segments.get(*i).map(|s| s.seconds).unwrap_or(0);
            if planned > 0 {
                drift += d.as_secs_f64() - planned as f64;
            }
        }
        if let (Some(i), Some(s)) = (self.current, self.started) {
            let seg = segments.get(i);
            st.index = Some(i);
            st.name = seg.map(|s| s.name.clone()).unwrap_or_default();
            st.next = segments.get(i + 1).map(|s| s.name.clone());
            st.elapsed = (now - s).as_secs_f64();
            st.planned = seg.map(|s| s.seconds as f64).unwrap_or(0.0);
            if st.planned > 0.0 && st.elapsed > st.planned {
                drift += st.elapsed - st.planned;
            }
        } else {
            st.next = segments.first().map(|s| s.name.clone());
        }
        st.drift = drift;
        st.show = self.show_started.filter(|_| self.current.is_some()).map(|s| (now - s).as_secs_f64()).unwrap_or(0.0);
        st
    }
}

/// Planned length of the whole show, in seconds.
pub fn planned_total(segments: &[Segment]) -> u64 {
    segments.iter().map(|s| s.seconds as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Target;

    fn segs() -> Vec<Segment> {
        ["Intro", "Interview", "Outro"].iter().zip([60, 600, 0]).map(|(n, s)| Segment { name: n.to_string(), seconds: s, ..Default::default() }).collect()
    }

    #[test]
    fn steps_through_the_show() {
        let s = segs();
        let mut r = Rundown::default();
        assert_eq!(RundownCmd::parse("next"), RundownCmd::Next);
        assert_eq!(RundownCmd::parse("2"), RundownCmd::Go(Target::Index(1)));
        assert_eq!(RundownCmd::parse("Outro"), RundownCmd::Go(Target::Name("Outro".into())));
        assert_eq!(r.target(&RundownCmd::Previous, &s), None);
        assert_eq!(r.target(&RundownCmd::Stop, &s), None, "not running");
        assert_eq!(r.target(&RundownCmd::Next, &s), Some(Some(0)), "next starts the show");
        assert_eq!(r.status(&s).next.as_deref(), Some("Intro"));
        r.go(Some(0));
        let st = r.status(&s);
        assert_eq!((st.index, st.name.as_str(), st.next.as_deref(), st.planned), (Some(0), "Intro", Some("Interview"), 60.0));
        assert_eq!(r.target(&RundownCmd::Next, &s), Some(Some(1)));
        assert_eq!(r.target(&RundownCmd::Go(Target::Name("outro".into())), &s), Some(Some(2)));
        assert_eq!(r.target(&RundownCmd::Go(Target::Index(7)), &s), None);
        // The intro ran 15 s over.
        r.started = Some(Instant::now() - Duration::from_secs(75));
        r.show_started = r.started;
        assert!((r.status(&s).drift - 15.0).abs() < 0.5);
        r.go(Some(1));
        let st = r.status(&s);
        assert!((st.drift - 15.0).abs() < 0.5, "carried over: {}", st.drift);
        assert!(st.show >= 75.0 && st.elapsed < 1.0);
        r.go(Some(2));
        assert_eq!(r.target(&RundownCmd::Next, &s), None, "the end");
        assert_eq!(r.target(&RundownCmd::Stop, &s), Some(None));
        r.go(None);
        assert_eq!(r.status(&s).index, None);
        assert_eq!(r.status(&s).show, 0.0);
        assert_eq!(planned_total(&s), 660);
        assert_eq!(Rundown::default().target(&RundownCmd::Next, &[]), None);
    }
}
