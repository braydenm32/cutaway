//! Live captions: speech recognition on a mix (or one input), shown as a
//! caption box on the stream (and, if wanted, the recording), and kept as
//! timed cues for each take's transcript and subtitle files.
//!
//! Recognition runs in requests of up to a minute, restarted at pauses; a
//! request's final result gives each word's time, which becomes cues on the
//! program clock (the same clock as every file).

use super::graphics::{GraphicLayer, FULL_UV};
use super::text::{ascent, draw_text, system_fonts, text_width, wrap};
use super::{LogLevel, Status, Waker};
use crate::config::{CaptionPlace, CaptionsConfig};
use crate::media::{AudioBlock, Ns, NS_PER_SEC};
use crate::platform::speech::{Event, Recognizer, Word};
use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use image::RgbaImage;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One caption: host (program clock) times.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: Ns,
    pub end: Ns,
    pub text: String,
}

const MAX_CUE_CHARS: usize = 84;
/// Graphics key for the caption box (well clear of the graphics' own).
const CAPTION_KEY: usize = 1_000_000;

/// Words of a finished request (timed from `base`) as readable cues: at
/// most two lines, split at pauses and sentence ends.
pub fn cues_from_words(base: Ns, words: &[Word]) -> Vec<Cue> {
    let ns = |s: f64| base + (s * NS_PER_SEC as f64) as Ns;
    let mut cues: Vec<Cue> = Vec::new();
    let mut cur: Option<Cue> = None;
    let mut prev_end = f64::MIN;
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let text = w.text.trim();
        let start_new = match &cur {
            None => true,
            Some(c) => {
                let gap = w.start - prev_end;
                let sentence_done = c.text.ends_with(['.', '?', '!']) && c.text.len() >= 25;
                c.text.len() + 1 + text.len() > MAX_CUE_CHARS || gap > 0.8 || sentence_done
            }
        };
        if start_new {
            if let Some(c) = cur.take() {
                cues.push(c);
            }
            cur = Some(Cue { start: ns(w.start), end: ns(w.start + w.duration), text: text.to_string() });
        } else if let Some(c) = cur.as_mut() {
            c.text.push(' ');
            c.text.push_str(text);
            c.end = ns(w.start + w.duration);
        }
        prev_end = w.start + w.duration;
    }
    cues.extend(cur);
    // Keep each up for at least a second (without running into the next).
    for i in 0..cues.len() {
        let next = cues.get(i + 1).map(|c| c.start).unwrap_or(Ns::MAX);
        let c = &mut cues[i];
        c.end = c.end.max((c.start + NS_PER_SEC).min(next));
    }
    cues
}

fn stamp(ns: Ns, sep: char) -> String {
    let ms = (ns.max(0) / 1_000_000) as u64;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

/// Two lines of at most ~42 characters.
fn lines_for_file(text: &str) -> String {
    if text.len() <= 42 {
        return text.to_string();
    }
    let mid = text.len() / 2;
    let split = text.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(mid));
    match split {
        Some(i) => format!("{}\n{}", &text[..i], &text[i + 1..]),
        None => text.to_string(),
    }
}

/// Transcript, SRT and WebVTT for cues of a take starting at `t0`.
pub fn transcript_files(cues: &[Cue], t0: Ns, take_name: &str) -> (String, String, String) {
    let rel: Vec<Cue> = cues.iter().filter(|c| c.end > t0).map(|c| Cue { start: (c.start - t0).max(0), end: c.end - t0, text: c.text.clone() }).collect();
    let mut srt = String::new();
    let mut vtt = String::from("WEBVTT\n\n");
    for (i, c) in rel.iter().enumerate() {
        srt += &format!("{}\n{} --> {}\n{}\n\n", i + 1, stamp(c.start, ','), stamp(c.end, ','), lines_for_file(&c.text));
        vtt += &format!("{} --> {}\n{}\n\n", stamp(c.start, '.'), stamp(c.end, '.'), lines_for_file(&c.text));
    }
    let mut txt = format!("Transcript: {take_name}\n(Times are from the start of the take's files.)\n");
    let mut last_end: Option<Ns> = None;
    for c in &rel {
        if last_end.is_none_or(|e| c.start - e > 2 * NS_PER_SEC) {
            let s = (c.start / NS_PER_SEC) as u64;
            txt += &format!("\n[{:02}:{:02}:{:02}] ", s / 3600, (s / 60) % 60, s % 60);
        } else {
            txt.push(' ');
        }
        txt += &c.text;
        last_end = Some(c.end);
    }
    txt.push('\n');
    (txt, srt, vtt)
}

/// Draw caption lines: white on a dark box.
fn render(lines: &[String], canvas: (u32, u32), size: f32) -> RgbaImage {
    let (_, font) = system_fonts();
    let px = (canvas.1 as f32 * size.clamp(0.02, 0.1)).max(10.0);
    let pad = px * 0.35;
    let line_h = px * 1.25;
    let w = lines.iter().map(|l| text_width(font, px, l)).fold(0.0, f32::max) + pad * 2.0;
    let h = line_h * lines.len() as f32 + pad * 2.0 - (line_h - px);
    let mut img = RgbaImage::from_pixel(w.ceil().max(2.0) as u32, h.ceil().max(2.0) as u32, image::Rgba([0, 0, 0, 190]));
    let a = ascent(font, px);
    for (i, l) in lines.iter().enumerate() {
        let lw = text_width(font, px, l);
        draw_text(&mut img, font, px, (w - lw) / 2.0, pad + line_h * i as f32 + a * 0.95, l, [255, 255, 255]);
    }
    img
}

/// Shared between the captions thread, the compositor and the app.
struct Shared {
    show: AtomicBool,
    style: Mutex<CaptionsConfig>,
    canvas: (u32, u32),
    /// The caption box: (version, picture, rect).
    image: Mutex<Option<(u64, Arc<RgbaImage>, [f32; 4])>>,
    text: Mutex<String>,
    cues: Mutex<VecDeque<Cue>>,
    status: Mutex<String>,
    /// Everything before this program time has its final cues.
    settled_until: AtomicI64,
    /// End the current request once the audio reaches this time.
    restart_at: AtomicI64,
    version: AtomicU64,
    restyle: AtomicBool,
}

pub struct Captions {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Captions {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

type MakeRecognizer = Box<dyn FnOnce() -> Result<Box<dyn Recognizer>> + Send>;

impl Captions {
    /// Start listening to `audio` (48 kHz stereo blocks on the program clock).
    pub fn start(cfg: &CaptionsConfig, canvas: (u32, u32), audio: Receiver<AudioBlock>, make: MakeRecognizer, status: Arc<Mutex<Status>>, waker: Waker) -> Captions {
        let shared = Arc::new(Shared {
            show: AtomicBool::new(cfg.show),
            style: Mutex::new(cfg.clone()),
            canvas,
            image: Mutex::new(None),
            text: Mutex::new(String::new()),
            cues: Mutex::new(VecDeque::new()),
            status: Mutex::new("starting…".into()),
            settled_until: AtomicI64::new(Ns::MIN),
            restart_at: AtomicI64::new(Ns::MAX),
            version: AtomicU64::new(0),
            restyle: AtomicBool::new(false),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, stop2) = (shared.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("captions".into())
            .spawn(move || {
                let rec = match make() {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("{e:#}");
                        *s2.status.lock().unwrap() = format!("not running: {msg}");
                        status.lock().unwrap().push(LogLevel::Warning, format!("Live captions: {msg}"));
                        waker();
                        return;
                    }
                };
                *s2.status.lock().unwrap() = "listening".into();
                waker();
                Runner::new(s2, rec, waker).run(audio, &stop2);
            })
            .expect("thread");
        Captions { shared, stop, thread: Some(thread) }
    }

    pub fn showing(&self) -> bool {
        self.shared.show.load(Ordering::Relaxed)
    }

    pub fn set_showing(&self, on: bool) {
        self.shared.show.store(on, Ordering::Relaxed);
    }

    /// Size, lines and placement changed.
    pub fn set_style(&self, cfg: &CaptionsConfig) {
        *self.shared.style.lock().unwrap() = cfg.clone();
        self.shared.restyle.store(true, Ordering::Relaxed);
    }

    /// "listening", or why not.
    pub fn status(&self) -> String {
        self.shared.status.lock().unwrap().clone()
    }

    /// What's on screen now.
    pub fn text(&self) -> String {
        self.shared.text.lock().unwrap().clone()
    }

    /// The caption box for the compositor, when shown.
    pub fn layer(&self) -> Option<GraphicLayer> {
        if !self.showing() {
            return None;
        }
        let on_recording = self.shared.style.lock().unwrap().on_recording;
        let image = self.shared.image.lock().unwrap();
        let (version, img, rect) = image.as_ref()?;
        Some(GraphicLayer { key: (CAPTION_KEY, *version), image: img.clone(), rect: *rect, alpha: 1.0, stream_only: !on_recording, uv: FULL_UV })
    }

    /// Final cues overlapping [from, to].
    pub fn cues_between(&self, from: Ns, to: Ns) -> Vec<Cue> {
        self.shared.cues.lock().unwrap().iter().filter(|c| c.end > from && c.start < to).cloned().collect()
    }

    /// Finish recognising everything up to `until` (ends the current
    /// request), waiting up to `timeout`.
    pub fn settle(&self, until: Ns, timeout: Duration) {
        if self.shared.status.lock().unwrap().starts_with("not running") {
            return;
        }
        self.shared.restart_at.store(until, Ordering::Relaxed);
        let start = Instant::now();
        while self.shared.settled_until.load(Ordering::Relaxed) < until && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The captions thread's state.
struct Runner {
    shared: Arc<Shared>,
    rec: Box<dyn Recognizer>,
    events: Receiver<Event>,
    waker: Waker,
    request: u64,
    /// Program time of each request's first sample.
    bases: BTreeMap<u64, Ns>,
    /// Requests ended but not yet final.
    pending: Vec<u64>,
    /// Samples given to the current request.
    pushed: u64,
    /// Where the audio pushed so far ends (program time).
    audio_end: Ns,
    quiet_ms: u32,
    committed: String,
    partial: String,
    last_change: Instant,
    shown: String,
}

impl Runner {
    fn new(shared: Arc<Shared>, rec: Box<dyn Recognizer>, waker: Waker) -> Self {
        let events = rec.events();
        Runner {
            shared,
            rec,
            events,
            waker,
            request: 1,
            bases: BTreeMap::new(),
            pending: Vec::new(),
            pushed: 0,
            audio_end: Ns::MIN,
            quiet_ms: 0,
            committed: String::new(),
            partial: String::new(),
            last_change: Instant::now(),
            shown: String::new(),
        }
    }

    fn run(mut self, audio: Receiver<AudioBlock>, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match audio.recv_timeout(Duration::from_millis(50)) {
                Ok(b) => self.push(&b),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let age = self.pushed as f64 / 48_000.0;
            let restart_at = self.shared.restart_at.load(Ordering::Relaxed);
            let asked = restart_at != Ns::MAX && self.audio_end >= restart_at;
            if asked {
                self.shared.restart_at.store(Ns::MAX, Ordering::Relaxed);
            }
            if asked || (age > 45.0 && self.quiet_ms >= 300) || age > 58.0 {
                self.restart();
            }
            while let Ok(e) = self.events.try_recv() {
                self.handle(e);
            }
            self.update_settled();
            // A pause: clear the screen and start afresh.
            if !self.shown.is_empty() && self.last_change.elapsed() > Duration::from_secs(4) {
                self.committed.clear();
                self.partial.clear();
                if self.pushed > 0 {
                    self.restart();
                }
                self.redraw();
            }
            if self.shared.restyle.swap(false, Ordering::Relaxed) {
                self.shown.clear();
                self.redraw();
            }
        }
    }

    fn push(&mut self, b: &AudioBlock) {
        let ch = b.channels.max(1) as usize;
        let mono: Vec<f32> = b.samples.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        if mono.is_empty() {
            return;
        }
        self.bases.entry(self.request).or_insert(b.pts);
        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
        let ms = (mono.len() * 1000 / 48_000) as u32;
        self.quiet_ms = if rms < 0.003 { self.quiet_ms.saturating_add(ms) } else { 0 };
        self.rec.push(&mono);
        self.pushed += mono.len() as u64;
        self.audio_end = b.pts + mono.len() as i64 * NS_PER_SEC / 48_000;
    }

    fn restart(&mut self) {
        if !self.partial.is_empty() {
            self.committed = format!("{} {}", self.committed, self.partial).trim().to_string();
            self.partial.clear();
        }
        if self.pushed > 0 {
            self.pending.push(self.request);
        } else {
            self.bases.remove(&self.request);
        }
        self.request = self.rec.restart();
        self.pushed = 0;
        self.update_settled();
    }

    fn handle(&mut self, e: Event) {
        match e {
            Event::Partial { request, text } if request == self.request => {
                if text != self.partial {
                    self.partial = text;
                    self.last_change = Instant::now();
                    self.redraw();
                }
            }
            Event::Partial { .. } => {}
            Event::Final { request, text, words } => {
                if let Some(&base) = self.bases.get(&request) {
                    let cues = cues_from_words(base, &words);
                    let mut all = self.shared.cues.lock().unwrap();
                    all.extend(cues);
                    // Keep about a day of captions.
                    while all.len() > 50_000 {
                        all.pop_front();
                    }
                }
                self.pending.retain(|r| *r != request);
                if request == self.request {
                    // The recogniser finished on its own: carry on with a new one.
                    self.partial.clear();
                    if !text.is_empty() {
                        self.committed = format!("{} {text}", self.committed).trim().to_string();
                        self.last_change = Instant::now();
                    }
                    self.request = self.rec.restart();
                    self.pushed = 0;
                    self.redraw();
                }
                // Times are only needed until a request's final result.
                let (pending, current) = (self.pending.clone(), self.request);
                self.bases.retain(|r, _| *r == current || pending.contains(r));
            }
            Event::Error { request, message } => {
                *self.shared.status.lock().unwrap() = format!("problem: {message}");
                self.pending.retain(|r| *r != request);
                if request == self.request {
                    self.request = self.rec.restart();
                    self.pushed = 0;
                }
                (self.waker)();
            }
        }
    }

    fn update_settled(&self) {
        let settled = match self.pending.iter().filter_map(|r| self.bases.get(r)).min() {
            Some(&b) => b,
            None => self.bases.get(&self.request).copied().unwrap_or(self.audio_end),
        };
        self.shared.settled_until.store(settled, Ordering::Relaxed);
    }

    /// Redraw the box if the visible lines changed.
    fn redraw(&mut self) {
        let style = self.shared.style.lock().unwrap().clone();
        let canvas = self.shared.canvas;
        let full = format!("{} {}", self.committed, self.partial).trim().to_string();
        // Don't let the remembered text grow without end.
        if self.committed.len() > 600 {
            let cut = self.committed.len() - 400;
            let at = self.committed[cut..].find(' ').map(|i| cut + i + 1).unwrap_or(cut);
            self.committed = self.committed[at..].to_string();
        }
        let (_, font) = system_fonts();
        let px = (canvas.1 as f32 * style.size.clamp(0.02, 0.1)).max(10.0);
        let max_w = canvas.0 as f32 * 0.8;
        let all = wrap(font, px, &full, max_w);
        let n = style.lines.clamp(1, 3) as usize;
        let lines: Vec<String> = all[all.len().saturating_sub(n)..].to_vec();
        let shown = lines.join("\n");
        if shown == self.shown {
            return;
        }
        self.shown = shown.clone();
        *self.shared.text.lock().unwrap() = shown;
        let mut image = self.shared.image.lock().unwrap();
        if lines.is_empty() {
            *image = None;
        } else {
            let img = render(&lines, canvas, style.size);
            let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
            let (w, h) = (img.width() as f32 / cw, img.height() as f32 / ch);
            let y = match style.place {
                CaptionPlace::Bottom => 0.93 - h,
                CaptionPlace::Top => 0.07,
            };
            let version = self.shared.version.fetch_add(1, Ordering::Relaxed) + 1;
            *image = Some((version, Arc::new(img), [(1.0 - w) / 2.0, y, w, h]));
        }
        drop(image);
        (self.waker)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::speech::Scripted;

    fn w(text: &str, start: f64) -> Word {
        Word { text: text.into(), start, duration: 0.25 }
    }

    #[test]
    fn words_become_readable_cues() {
        let words = vec![w("Hello", 0.0), w("there.", 0.3), w("This", 2.0), w("is", 2.3), w("the", 2.6), w("show.", 2.9)];
        let cues = cues_from_words(10 * NS_PER_SEC, &words);
        assert_eq!(cues.len(), 2, "{cues:?}");
        assert_eq!(cues[0].text, "Hello there.");
        assert_eq!(cues[0].start, 10 * NS_PER_SEC);
        // At least a second on screen.
        assert_eq!(cues[0].end, 11 * NS_PER_SEC);
        assert_eq!(cues[1].text, "This is the show.");
        // Long runs are split.
        let long: Vec<Word> = (0..40).map(|i| w("word", i as f64 * 0.3)).collect();
        assert!(cues_from_words(0, &long).iter().all(|c| c.text.len() <= MAX_CUE_CHARS));
        let (txt, srt, vtt) = transcript_files(&cues, 9 * NS_PER_SEC, "Take");
        assert!(srt.starts_with("1\n00:00:01,000 --> 00:00:02,000\nHello there.\n\n2\n00:00:03,000"), "{srt}");
        assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n"), "{vtt}");
        assert!(txt.contains("\n[00:00:01] Hello there. This is the show.\n"), "{txt}");
        // A long pause starts a new paragraph.
        let later = vec![cues[0].clone(), Cue { start: 20 * NS_PER_SEC, end: 21 * NS_PER_SEC, text: "Next part.".into() }];
        assert!(transcript_files(&later, 9 * NS_PER_SEC, "Take").0.contains("\n[00:00:11] Next part."));
        assert_eq!(lines_for_file("one two three four five six seven eight nine ten eleven"), "one two three four five six\nseven eight nine ten eleven");
    }

    #[test]
    fn listens_shows_and_settles() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let status = Arc::new(Mutex::new(Status::default()));
        let cfg = CaptionsConfig { enabled: true, ..Default::default() };
        let c = Captions::start(&cfg, (1280, 720), rx, Box::new(|| Ok(Box::new(Scripted::new()) as Box<dyn Recognizer>)), status, Arc::new(|| {}));
        // Two seconds of "speech" (loud tone), on the program clock from 100 s.
        let t0 = 100 * NS_PER_SEC;
        for k in 0..200 {
            let samples: Vec<f32> = (0..960).map(|i| ((i / 2) as f32 * 0.1).sin() * 0.3).collect();
            tx.send(AudioBlock { pts: t0 + k * 10_000_000, rate: 48_000, channels: 2, samples }).unwrap();
        }
        let start = Instant::now();
        while !c.text().contains("back") && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let layer = c.layer().expect("captions on screen");
        assert!(layer.stream_only, "stream only unless asked");
        assert!(layer.rect[1] > 0.7, "near the bottom: {:?}", layer.rect);
        assert!(c.text().starts_with("Welcome back"), "{}", c.text());
        c.set_showing(false);
        assert!(c.layer().is_none());
        // Stopping a take: everything up to the end gets its final cues.
        c.settle(t0 + 2 * NS_PER_SEC, Duration::from_secs(3));
        let cues = c.cues_between(t0, t0 + 2 * NS_PER_SEC);
        assert!(!cues.is_empty());
        assert_eq!(cues[0].start, t0);
        assert!(cues[0].text.starts_with("Welcome back to the show."), "{cues:?}");
    }
}
