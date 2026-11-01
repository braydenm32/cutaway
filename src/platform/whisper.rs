//! Live captions on Windows (and Linux) with Whisper, through whisper.cpp:
//! speech is recognised on the computer; nothing is sent anywhere.
//!
//! Whisper isn't a streaming recogniser, so the recent audio is
//! transcribed again every second or so: words before a pause, or more
//! than about ten seconds back, are settled; the rest may still change
//! (like live captions anywhere). The speech model is downloaded once,
//! from Graphics › Captions.

use crate::util::LockExt;
use super::speech::{Event, Recognizer, Word};
use crate::config::SpeechModel;
use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Whisper works on 16 kHz mono.
const RATE: usize = 16_000;

// ---- the models ---------------------------------------------------------------------------

/// A Whisper model file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelFile {
    pub file: &'static str,
    pub megabytes: u32,
}

fn english(language: &str) -> bool {
    let l = language.trim().to_lowercase();
    l.is_empty() || l.starts_with("en")
}

/// The model for a language and choice (English-only models are better at
/// English).
pub fn model_file(language: &str, model: SpeechModel) -> ModelFile {
    match (model, english(language)) {
        (SpeechModel::Fast, true) => ModelFile { file: "ggml-base.en-q5_1.bin", megabytes: 57 },
        (SpeechModel::Fast, false) => ModelFile { file: "ggml-base-q5_1.bin", megabytes: 57 },
        (SpeechModel::Accurate, true) => ModelFile { file: "ggml-small.en-q5_1.bin", megabytes: 181 },
        (SpeechModel::Accurate, false) => ModelFile { file: "ggml-small-q5_1.bin", megabytes: 181 },
    }
}

/// Where downloaded models are kept.
pub fn models_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| crate::config::home_dir().join("AppData").join("Local")).join("Cutaway")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::config::home_dir().join(".local").join("share")).join("cutaway")
    };
    base.join("Models")
}

/// The model's file, if it's been downloaded.
pub fn installed(language: &str, model: SpeechModel) -> Option<PathBuf> {
    let p = models_dir().join(model_file(language, model).file);
    std::fs::metadata(&p).ok().filter(|m| m.len() > 1_000_000).map(|_| p)
}

pub fn problem(language: &str, model: SpeechModel) -> Option<String> {
    if installed(language, model).is_some() {
        return None;
    }
    let m = model_file(language, model);
    Some(format!("the speech model ({} MB) isn't downloaded yet (Graphics › Captions › Download)", m.megabytes))
}

// ---- downloading ---------------------------------------------------------------------------

/// How a model download is going.
#[derive(Clone, Debug, Default)]
pub struct Download {
    pub got: u64,
    pub total: u64,
    pub running: bool,
    pub error: Option<String>,
}

fn downloads() -> &'static Mutex<HashMap<&'static str, Download>> {
    static D: OnceLock<Mutex<HashMap<&'static str, Download>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

pub fn download_state(language: &str, model: SpeechModel) -> Option<Download> {
    downloads().lock_ok().get(model_file(language, model).file).cloned()
}

/// Fetch the model in the background (once at a time).
pub fn download(language: &str, model: SpeechModel) {
    let m = model_file(language, model);
    {
        let mut d = downloads().lock_ok();
        if d.get(m.file).is_some_and(|x| x.running) {
            return;
        }
        d.insert(m.file, Download { running: true, ..Default::default() });
    }
    std::thread::spawn(move || {
        let r = fetch(m.file);
        let mut d = downloads().lock_ok();
        let s = d.entry(m.file).or_default();
        s.running = false;
        s.error = r.err().map(|e| format!("{e:#}"));
    });
}

fn fetch(file: &'static str) -> Result<()> {
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}");
    // A whisper.cpp model starts with "lmgg". The shared downloader checks
    // that, the answer and the length, and gives up on a stalled connection.
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::net::fetch::fetch(&url, &models_dir().join(file), b"lmgg", "the model server", "a speech model", &never, &mut |got, total| {
        if let Some(s) = downloads().lock_ok().get_mut(file) {
            s.got = got;
            s.total = total.unwrap_or(0);
        }
    })?;
    Ok(())
}

// ---- turning audio into text ---------------------------------------------------------------

/// A stretch of recognised text, timed in seconds from the start of the
/// audio given.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Something that transcribes audio (whisper.cpp, or a stand-in in tests).
pub trait Transcriber: Send {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>>;
}

struct WhisperCpp {
    state: whisper_rs::WhisperState,
    language: String,
    threads: i32,
}

impl Transcriber for WhisperCpp {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>> {
        let mut p = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some(&self.language));
        p.set_n_threads(self.threads);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_suppress_nst(true);
        p.set_no_speech_thold(0.6);
        p.set_temperature(0.0);
        self.state.full(p, audio).map_err(|e| anyhow!("speech recognition: {e}"))?;
        let mut out = Vec::new();
        for i in 0..self.state.full_n_segments() {
            let Some(seg) = self.state.get_segment(i) else { continue };
            if seg.no_speech_probability() > 0.6 {
                continue;
            }
            let text = seg.to_str_lossy().map(|t| t.to_string()).unwrap_or_default();
            // whisper.cpp times segments in hundredths of a second.
            out.push(Segment { start: seg.start_timestamp() as f64 / 100.0, end: seg.end_timestamp() as f64 / 100.0, text });
        }
        Ok(out)
    }
}

/// Text fit for captions: no "[Music]" or "(applause)" annotations, no
/// stock phrases Whisper invents for silence.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let (mut depth, mut starred) = (0i32, false);
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            '*' => starred = !starred,
            _ if depth == 0 && !starred => out.push(c),
            _ => {}
        }
    }
    let t = out.split_whitespace().collect::<Vec<_>>().join(" ");
    const INVENTED: [&str; 4] = ["thank you.", "thanks for watching!", "you", "."];
    if INVENTED.contains(&t.to_lowercase().as_str()) {
        String::new()
    } else {
        t
    }
}

/// Words of a segment, each given a share of its time by length.
fn words_of(seg: &Segment, offset: f64) -> Vec<Word> {
    let text = clean(&seg.text);
    let parts: Vec<&str> = text.split_whitespace().collect();
    let chars: usize = parts.iter().map(|w| w.chars().count() + 1).sum();
    let span = (seg.end - seg.start).max(0.05);
    let mut at = seg.start;
    parts
        .iter()
        .map(|w| {
            let d = span * (w.chars().count() + 1) as f64 / chars.max(1) as f64;
            let word = Word { text: w.to_string(), start: offset + at, duration: d };
            at += d;
            word
        })
        .collect()
}

/// Is there speech-like sound in this audio (not just room noise)?
fn has_sound(audio: &[f32]) -> bool {
    let frames = audio.chunks(480);
    let n = frames.len().max(1);
    let loud = audio.chunks(480).filter(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len().max(1) as f32).sqrt() > 0.01).count();
    loud * 20 >= n
}

enum Msg {
    Audio(Vec<f32>),
    Restart(u64),
}

/// The recogniser's work, separate from threads for testing: audio comes
/// in, partial and final results go out.
struct Stream<T: Transcriber> {
    t: T,
    events: Sender<Event>,
    request: u64,
    /// The current request's audio (16 kHz).
    audio: Vec<f32>,
    /// Settled words, and where the unsettled audio starts.
    settled: Vec<Word>,
    window: usize,
    /// Audio length at the last transcription, and how long it took.
    last_len: usize,
    last_cost: Duration,
}

impl<T: Transcriber> Stream<T> {
    fn new(t: T, events: Sender<Event>) -> Self {
        Stream { t, events, request: 1, audio: Vec::new(), settled: Vec::new(), window: 0, last_len: 0, last_cost: Duration::ZERO }
    }

    fn text(words: &[Word]) -> String {
        words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Transcribe the unsettled audio; settle what's safely past.
    fn update(&mut self, final_pass: bool) {
        let window = &self.audio[self.window.min(self.audio.len())..];
        let offset = self.window as f64 / RATE as f64;
        let mut current = Vec::new();
        if window.len() >= RATE / 4 && has_sound(window) {
            let started = Instant::now();
            match self.t.transcribe(window) {
                Ok(segs) => {
                    self.last_cost = started.elapsed();
                    let secs = window.len() as f64 / RATE as f64;
                    // Settle every segment but the last one, once there's a
                    // pause after it or the window is long; everything at
                    // the end.
                    let mut keep = if final_pass { segs.len() } else { segs.len().saturating_sub(1) };
                    if !final_pass && segs.last().is_some_and(|s| secs - s.end > 1.0 || secs > 20.0) {
                        keep = segs.len();
                    }
                    if !final_pass && secs < 8.0 && keep < segs.len() {
                        keep = 0; // short window: let it all still change
                    }
                    for s in &segs[..keep] {
                        self.settled.extend(words_of(s, offset));
                    }
                    if keep > 0 {
                        let end = segs[keep - 1].end;
                        self.window = (self.window + (end * RATE as f64) as usize).min(self.audio.len());
                    }
                    for s in &segs[keep..] {
                        current.extend(words_of(s, offset));
                    }
                }
                Err(e) => {
                    let _ = self.events.send(Event::Error { request: self.request, message: format!("{e:#}") });
                }
            }
        } else if window.len() > 3 * RATE {
            // Quiet for a while: nothing to keep from it.
            self.window = self.audio.len() - RATE;
        }
        self.last_len = self.audio.len();
        if !final_pass {
            let mut all = self.settled.clone();
            all.extend(current);
            let _ = self.events.send(Event::Partial { request: self.request, text: Self::text(&all) });
        }
    }

    fn push(&mut self, samples: &[f32]) {
        self.audio.extend_from_slice(samples);
        // Again once there's a second more (or longer, if it's slow).
        let wait = (RATE as f64 * (self.last_cost.as_secs_f64() * 1.2).max(1.0)) as usize;
        if self.audio.len() - self.last_len >= wait {
            self.update(false);
        }
    }

    fn restart(&mut self, next: u64) {
        self.update(true);
        let words = std::mem::take(&mut self.settled);
        let _ = self.events.send(Event::Final { request: self.request, text: Self::text(&words), words });
        self.request = next;
        self.audio.clear();
        self.window = 0;
        self.last_len = 0;
    }
}

/// 48 kHz → 16 kHz (averaging each three samples, enough for speech).
#[derive(Default)]
struct Downsample {
    carry: Vec<f32>,
}

impl Downsample {
    fn run(&mut self, samples: &[f32]) -> Vec<f32> {
        self.carry.extend_from_slice(samples);
        let n = self.carry.len() / 3;
        let out = self.carry.chunks_exact(3).map(|c| (c[0] + c[1] + c[2]) / 3.0).collect();
        self.carry.drain(..n * 3);
        out
    }
}

pub struct WhisperRecognizer {
    tx: Sender<Msg>,
    events: Receiver<Event>,
    request: u64,
    down: Downsample,
}

impl Recognizer for WhisperRecognizer {
    fn push(&mut self, samples: &[f32]) {
        let s = self.down.run(samples);
        if !s.is_empty() {
            let _ = self.tx.send(Msg::Audio(s));
        }
    }

    fn restart(&mut self) -> u64 {
        self.request += 1;
        let _ = self.tx.send(Msg::Restart(self.request));
        self.request
    }

    fn events(&self) -> Receiver<Event> {
        self.events.clone()
    }
}

/// Start recognising with the downloaded model.
pub fn start(language: &str, model: SpeechModel) -> Result<Box<dyn Recognizer>> {
    let path = installed(language, model).ok_or_else(|| anyhow!("{}", problem(language, model).unwrap_or_default()))?;
    let lang = {
        let l = language.trim().to_lowercase();
        if l.is_empty() { "en".to_string() } else { l.split(['-', '_']).next().unwrap_or("en").to_string() }
    };
    let ctx = whisper_rs::WhisperContext::new_with_params(&path, whisper_rs::WhisperContextParameters::default()).map_err(|e| anyhow!("loading the speech model: {e}"))?;
    let state = ctx.create_state().map_err(|e| anyhow!("speech recognition: {e}"))?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8) as i32;
    let t = WhisperCpp { state, language: lang, threads };
    Ok(Box::new(spawn(t, ctx)))
}

/// Run a transcriber on its own thread (`keep` lives as long as it does).
fn spawn<T: Transcriber + 'static, K: Send + 'static>(t: T, keep: K) -> WhisperRecognizer {
    let (tx, rx) = unbounded::<Msg>();
    let (etx, erx) = unbounded();
    std::thread::Builder::new()
        .name("whisper".into())
        .spawn(move || {
            let _keep = keep;
            let mut s = Stream::new(t, etx);
            loop {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(Msg::Audio(a)) => {
                        // Take everything waiting, then work once.
                        let mut all = a;
                        let mut restart = None;
                        while let Ok(m) = rx.try_recv() {
                            match m {
                                Msg::Audio(b) => all.extend(b),
                                Msg::Restart(n) => {
                                    restart = Some(n);
                                    break;
                                }
                            }
                        }
                        s.push(&all);
                        if let Some(n) = restart {
                            s.restart(n);
                        }
                    }
                    Ok(Msg::Restart(n)) => s.restart(n),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("thread");
    WhisperRecognizer { tx, events: erx, request: 1, down: Downsample::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pretends to hear one word per half second of loud audio, in
    /// segments that end at pauses.
    struct Fake;

    impl Transcriber for Fake {
        fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>> {
            let mut segs = Vec::new();
            let mut cur: Option<Segment> = None;
            for (i, chunk) in audio.chunks(RATE / 2).enumerate() {
                let t = i as f64 * 0.5;
                let loud = chunk.iter().any(|s| s.abs() > 0.1);
                match (&mut cur, loud) {
                    (Some(s), true) => {
                        s.text.push_str(&format!(" w{}", i));
                        s.end = t + 0.5;
                    }
                    (None, true) => cur = Some(Segment { start: t, end: t + 0.5, text: format!("w{i}") }),
                    (Some(_), false) => segs.push(cur.take().unwrap()),
                    (None, false) => {}
                }
            }
            segs.extend(cur);
            Ok(segs)
        }
    }

    fn speech(secs: f64, loud: bool) -> Vec<f32> {
        (0..(secs * RATE as f64) as usize).map(|i| if loud { (i as f32 * 0.05).sin() * 0.5 } else { 0.0 }).collect()
    }

    #[test]
    fn partials_then_settled_words_with_times() {
        let (tx, rx) = unbounded();
        let mut s = Stream::new(Fake, tx);
        for part in [speech(3.0, true), speech(1.5, false), speech(2.0, true)] {
            for c in part.chunks(RATE / 10) {
                s.push(c);
            }
        }
        s.restart(2);
        let events: Vec<Event> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, Event::Partial { request: 1, text } if text.starts_with("w0"))), "{events:?}");
        let Some(Event::Final { request, words, .. }) = events.last() else { panic!("{events:?}") };
        assert_eq!(*request, 1);
        assert_eq!(words.len(), 10, "{words:?}");
        // The second phrase starts 4.5 s in.
        let second = words.iter().find(|w| w.text == "w9").unwrap();
        assert!((second.start - 4.5).abs() < 0.3, "{second:?}");
    }

    #[test]
    fn long_speech_is_settled_as_it_goes() {
        let (tx, rx) = unbounded();
        let mut s = Stream::new(Fake, tx);
        for _ in 0..6 {
            for c in speech(4.0, true).chunks(RATE / 10) {
                s.push(c);
            }
            for c in speech(1.5, false).chunks(RATE / 10) {
                s.push(c);
            }
        }
        // Settled words never go back past 20 s of audio.
        assert!(s.audio.len() - s.window < 25 * RATE, "window {}", (s.audio.len() - s.window) / RATE);
        s.restart(2);
        let last = rx.try_iter().last().unwrap();
        let Event::Final { words, .. } = last else { panic!() };
        assert!(words.len() >= 40, "{}", words.len());
        assert!(words.windows(2).all(|w| w[1].start >= w[0].start), "in order");
    }

    #[test]
    fn cleans_annotations_and_silence_phrases() {
        assert_eq!(clean(" [Music] Hello (laughs) there "), "Hello there");
        assert_eq!(clean("Thank you."), "");
        assert_eq!(clean("*sighs* Right."), "Right.");
    }

    #[test]
    fn downsamples_in_threes() {
        let mut d = Downsample::default();
        assert_eq!(d.run(&[3.0, 3.0, 3.0, 6.0]), vec![3.0]);
        assert_eq!(d.run(&[6.0, 6.0]), vec![6.0]);
    }

    #[test]
    fn picks_models() {
        assert_eq!(model_file("", SpeechModel::Fast).file, "ggml-base.en-q5_1.bin");
        assert_eq!(model_file("en-AU", SpeechModel::Accurate).file, "ggml-small.en-q5_1.bin");
        assert_eq!(model_file("de", SpeechModel::Fast).file, "ggml-base-q5_1.bin");
    }

    /// With whisper.cpp's small test model (CUTAWAY_TEST_WHISPER=path):
    /// loads and runs without falling over.
    #[test]
    fn runs_whisper_cpp() {
        let Some(path) = std::env::var_os("CUTAWAY_TEST_WHISPER") else {
            eprintln!("skipping: set CUTAWAY_TEST_WHISPER to a ggml model");
            return;
        };
        let ctx = whisper_rs::WhisperContext::new_with_params(PathBuf::from(path), whisper_rs::WhisperContextParameters::default()).unwrap();
        let state = ctx.create_state().unwrap();
        let mut t = WhisperCpp { state, language: "en".into(), threads: 2 };
        let segs = t.transcribe(&speech(2.0, true)).unwrap();
        eprintln!("segments: {segs:?}");
    }
}
