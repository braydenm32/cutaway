//! Platform-specific capture and device discovery.

use crate::media::{AudioBlock, VideoFrame};
use std::sync::Arc;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(not(target_os = "macos"))]
pub mod onnx;
#[cfg(all(not(target_os = "macos"), feature = "whisper"))]
pub mod whisper;
#[cfg(windows)]
pub mod win;

pub type FrameSink = Arc<dyn Fn(VideoFrame) + Send + Sync>;
pub type AudioSink = Arc<dyn Fn(AudioBlock) + Send + Sync>;

// Audio devices (cpal: CoreAudio on macOS; WASAPI and ASIO on Windows).
#[cfg(any(target_os = "macos", windows))]
mod audio_in;
#[cfg(any(target_os = "macos", windows))]
mod audio_out;
#[cfg(any(target_os = "macos", windows))]
pub use audio_in::{list_audio_devices, open_audio_input};
#[cfg(any(target_os = "macos", windows))]
pub use audio_out::{list_output_devices, open_audio_output};

#[cfg(target_os = "macos")]
pub use macos::{list_video_devices, open_camera, open_screen, permissions_changed, request_permissions};
#[cfg(windows)]
pub use win::{list_video_devices, open_camera, open_screen, permissions_changed, request_permissions};

#[cfg(not(any(target_os = "macos", windows)))]
mod none {
    use super::*;
    use crate::devices::DeviceInfo;
    use crate::engine::sources::{AudioStarted, Running};
    use anyhow::{bail, Result};

    pub fn list_video_devices() -> Vec<DeviceInfo> {
        Vec::new()
    }
    pub fn list_audio_devices() -> Vec<DeviceInfo> {
        Vec::new()
    }
    pub fn open_camera(_dev: &DeviceInfo, _w: u32, _h: u32, _fps: u32, _sink: FrameSink) -> Result<Box<dyn Running>> {
        bail!("camera capture isn't available in this build (macOS only for now)")
    }
    pub fn open_screen(_w: u32, _h: u32, _fps: u32, _sink: FrameSink) -> Result<Box<dyn Running>> {
        bail!("screen capture isn't available in this build (macOS only for now)")
    }
    pub fn open_audio_input(_dev: Option<&DeviceInfo>, _channels: u32, _sink: AudioSink) -> Result<AudioStarted> {
        bail!("audio input isn't available in this build (macOS only for now)")
    }
    pub fn list_output_devices() -> Vec<DeviceInfo> {
        Vec::new()
    }
    pub fn open_audio_output(
        _name: &str,
        _rx: crossbeam_channel::Receiver<crate::media::AudioBlock>,
        _volume: Arc<std::sync::atomic::AtomicU32>,
    ) -> Result<Box<dyn Running>> {
        bail!("headphone monitoring isn't available in this build (macOS only for now)")
    }
    pub fn request_permissions() {}
    pub fn permissions_changed() -> bool {
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub use none::*;

/// The virtual camera: a camera extension on macOS, a virtual camera
/// source on Windows 11.
pub mod vcam {
    use crate::engine::sources::Running;
    use crate::media::VideoFrame;
    use anyhow::Result;
    use crossbeam_channel::Receiver;

    pub fn available() -> bool {
        cfg!(any(target_os = "macos", windows))
    }

    pub fn installed() -> bool {
        #[cfg(target_os = "macos")]
        {
            super::macos::vcam::installed()
        }
        #[cfg(windows)]
        {
            super::win::vcam::installed()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            false
        }
    }

    pub fn install(remove: bool) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            super::macos::vcam::install(remove)
        }
        #[cfg(windows)]
        {
            super::win::vcam::install(remove)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = remove;
            anyhow::bail!("the virtual camera needs macOS or Windows 11")
        }
    }

    pub fn start(rx: Receiver<VideoFrame>) -> Result<Box<dyn Running>> {
        #[cfg(target_os = "macos")]
        {
            super::macos::vcam::start(rx)
        }
        #[cfg(windows)]
        {
            super::win::vcam::start(rx)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = rx;
            anyhow::bail!("the virtual camera needs macOS or Windows 11")
        }
    }
}

/// Speech recognition for live captions.
pub mod speech {
    use anyhow::Result;
    use crossbeam_channel::{unbounded, Receiver, Sender};

    /// A recognised word, timed from the start of its request's audio.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Word {
        pub text: String,
        pub start: f64,
        pub duration: f64,
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum Event {
        /// The words so far in request `request` (may still change).
        Partial { request: u64, text: String },
        /// The settled words of `request`; it's finished.
        Final { request: u64, text: String, words: Vec<Word> },
        #[cfg_attr(not(any(target_os = "macos", feature = "whisper")), allow(dead_code))]
        Error { request: u64, message: String },
    }

    /// Takes 48 kHz mono audio; results come back on `events()`.
    pub trait Recognizer: Send {
        fn push(&mut self, samples: &[f32]);
        /// End the current request (its final result follows) and start a
        /// new one; returns the new request's number.
        fn restart(&mut self) -> u64;
        fn events(&self) -> Receiver<Event>;
    }

    /// Why captions can't run here, if they can't.
    pub fn problem(language: &str, model: crate::config::SpeechModel) -> Option<String> {
        if scripted() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            let _ = (language, model);
            super::macos::speech::problem()
        }
        #[cfg(all(not(target_os = "macos"), feature = "whisper"))]
        {
            super::whisper::problem(language, model)
        }
        #[cfg(all(not(target_os = "macos"), not(feature = "whisper")))]
        {
            let _ = (language, model);
            Some("live captions aren't part of this build".into())
        }
    }

    /// Languages to offer: (identifier, name).
    pub fn languages() -> Vec<(String, String)> {
        #[cfg(target_os = "macos")]
        {
            super::macos::speech::languages()
        }
        #[cfg(not(target_os = "macos"))]
        {
            [
                ("en", "English"),
                ("es", "Spanish"),
                ("fr", "French"),
                ("de", "German"),
                ("it", "Italian"),
                ("pt", "Portuguese"),
                ("nl", "Dutch"),
                ("sv", "Swedish"),
                ("da", "Danish"),
                ("no", "Norwegian"),
                ("fi", "Finnish"),
                ("pl", "Polish"),
                ("cs", "Czech"),
                ("tr", "Turkish"),
                ("el", "Greek"),
                ("ru", "Russian"),
                ("uk", "Ukrainian"),
                ("ar", "Arabic"),
                ("hi", "Hindi"),
                ("ja", "Japanese"),
                ("ko", "Korean"),
                ("zh", "Chinese"),
                ("id", "Indonesian"),
                ("vi", "Vietnamese"),
            ]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
        }
    }

    /// Start recognising in `language` ("" = the system's on a Mac,
    /// English elsewhere).
    pub fn start(language: &str, model: crate::config::SpeechModel) -> Result<Box<dyn Recognizer>> {
        if scripted() {
            return Ok(Box::new(Scripted::new()));
        }
        #[cfg(target_os = "macos")]
        {
            let _ = model;
            super::macos::speech::start(language)
        }
        #[cfg(all(not(target_os = "macos"), feature = "whisper"))]
        {
            super::whisper::start(language, model)
        }
        #[cfg(all(not(target_os = "macos"), not(feature = "whisper")))]
        {
            let _ = (language, model);
            anyhow::bail!("live captions aren't part of this build")
        }
    }

    /// Set CUTAWAY_TEST_SPEECH to caption with a script instead (tests and
    /// demos without the Speech framework).
    fn scripted() -> bool {
        std::env::var_os("CUTAWAY_TEST_SPEECH").is_some()
    }

    const SCRIPT: &str = "Welcome back to the show. Today we are talking about live production on a laptop, \
        recording every camera to its own file and switching the program as it happens. Our guest has been \
        building studios for twenty years, so let's start with the basics: what does a small team really need?";

    /// A pretend recogniser: while the audio is loud it "hears" the next
    /// word of a script every 350 ms.
    pub struct Scripted {
        tx: Sender<Event>,
        rx: Receiver<Event>,
        request: u64,
        /// Samples into the current request.
        at: u64,
        loud_since: Option<u64>,
        next_word_at: u64,
        words: Vec<Word>,
        script: Vec<&'static str>,
        cursor: usize,
    }

    impl Scripted {
        pub fn new() -> Self {
            let (tx, rx) = unbounded();
            Scripted { tx, rx, request: 1, at: 0, loud_since: None, next_word_at: 0, words: Vec::new(), script: SCRIPT.split_whitespace().collect(), cursor: 0 }
        }

        fn text(&self) -> String {
            self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
        }
    }

    impl Default for Scripted {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Recognizer for Scripted {
        fn push(&mut self, samples: &[f32]) {
            for chunk in samples.chunks(480) {
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
                if rms > 0.01 {
                    if self.loud_since.is_none() {
                        self.loud_since = Some(self.at);
                        self.next_word_at = self.at;
                    }
                    if self.at >= self.next_word_at {
                        let w = self.script[self.cursor % self.script.len()];
                        self.cursor += 1;
                        self.words.push(Word { text: w.into(), start: self.at as f64 / 48_000.0, duration: 0.3 });
                        self.next_word_at = self.at + 16_800;
                        let _ = self.tx.send(Event::Partial { request: self.request, text: self.text() });
                    }
                } else {
                    self.loud_since = None;
                }
                self.at += chunk.len() as u64;
            }
        }

        fn restart(&mut self) -> u64 {
            let _ = self.tx.send(Event::Final { request: self.request, text: self.text(), words: std::mem::take(&mut self.words) });
            self.request += 1;
            self.at = 0;
            self.loud_since = None;
            self.request
        }

        fn events(&self) -> Receiver<Event> {
            self.rx.clone()
        }
    }
}

/// Finding people in pictures: faces and upper bodies (auto-framing) and a
/// person mask (background removal).
pub mod vision {
    use crate::media::VideoFrame;
    use anyhow::Result;

    /// Something found in a picture. `rect` is x, y, width, height as
    /// fractions of the picture, from the top left.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Detection {
        pub rect: [f32; 4],
        /// A face (otherwise an upper body).
        pub face: bool,
        pub confidence: f32,
    }

    /// How much of each pixel is a person (0 = background, 255 = person).
    #[derive(Clone, Debug)]
    pub struct Mask {
        pub width: u32,
        pub height: u32,
        pub data: Vec<u8>,
    }

    pub trait Detector: Send {
        fn detect(&mut self, frame: &VideoFrame) -> Result<Vec<Detection>>;
    }

    pub trait Segmenter: Send {
        fn mask(&mut self, frame: &VideoFrame) -> Result<Mask>;
    }

    /// Why people can't be found here, if they can't.
    pub fn problem() -> Option<String> {
        if testing() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            None
        }
        #[cfg(not(target_os = "macos"))]
        {
            super::onnx::problem()
        }
    }

    /// Finds faces and people: Apple's Vision on a Mac, ONNX Runtime
    /// elsewhere.
    pub fn people_detector() -> Result<Box<dyn Detector>> {
        if testing() {
            return Ok(Box::new(TestDetector));
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::vision::detector()
        }
        #[cfg(not(target_os = "macos"))]
        {
            super::onnx::detector()
        }
    }

    /// `quality`: 0 fast, 1 balanced, 2 accurate (Vision only).
    pub fn segmenter(quality: u8) -> Result<Box<dyn Segmenter>> {
        if testing() {
            return Ok(Box::new(TestSegmenter));
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::vision::segmenter(quality)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = quality;
            super::onnx::segmenter()
        }
    }

    /// CUTAWAY_TEST_VISION: find the people of the "people" test picture
    /// by their colours (tests and demos without the Vision framework).
    pub fn testing() -> bool {
        std::env::var_os("CUTAWAY_TEST_VISION").is_some()
    }

    fn near(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
        (a.0 as i32 - b.0 as i32).abs() <= tol && (a.1 as i32 - b.1 as i32).abs() <= tol && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    /// YUV of pixel (x, y), x and y even.
    fn yuv(p: &crate::media::Planes<'_>, x: usize, y: usize) -> (u8, u8, u8) {
        let uvi = (y / 2) * p.uv_stride + (x & !1);
        (p.y[y * p.y_stride + x], p.uv[uvi], p.uv[uvi + 1])
    }

    pub struct TestDetector;

    impl Detector for TestDetector {
        fn detect(&mut self, frame: &VideoFrame) -> Result<Vec<Detection>> {
            use crate::media::testsrc::{rgb_to_yuv, SKIN};
            let skin = rgb_to_yuv(SKIN.0, SKIN.1, SKIN.2);
            let (w, h) = (frame.width as usize, frame.height as usize);
            let cell = 4;
            let (gw, gh) = (w / cell, h / cell);
            let mut grid = vec![false; gw * gh];
            frame.pixels.with_planes(|p| {
                for gy in 0..gh {
                    for gx in 0..gw {
                        grid[gy * gw + gx] = near(yuv(&p, gx * cell, gy * cell), skin, 10);
                    }
                }
            });
            // Connected patches of skin are faces.
            let mut seen = vec![false; grid.len()];
            let mut out = Vec::new();
            for start in 0..grid.len() {
                if !grid[start] || seen[start] {
                    continue;
                }
                let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0, 0, 0);
                let mut stack = vec![start];
                seen[start] = true;
                while let Some(i) = stack.pop() {
                    let (x, y) = (i % gw, i / gw);
                    (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
                    let mut push = |j: usize| {
                        if grid[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    };
                    if x > 0 { push(i - 1) }
                    if x + 1 < gw { push(i + 1) }
                    if y > 0 { push(i - gw) }
                    if y + 1 < gh { push(i + gw) }
                }
                if n >= 6 {
                    let rect = [(x0 * cell) as f32 / w as f32, (y0 * cell) as f32 / h as f32, ((x1 - x0 + 1) * cell) as f32 / w as f32, ((y1 - y0 + 1) * cell) as f32 / h as f32];
                    out.push(Detection { rect, face: true, confidence: 1.0 });
                }
            }
            Ok(out)
        }
    }

    pub struct TestSegmenter;

    impl Segmenter for TestSegmenter {
        fn mask(&mut self, frame: &VideoFrame) -> Result<Mask> {
            use crate::media::testsrc::{rgb_to_yuv, SHIRTS, SKIN};
            let targets: Vec<(u8, u8, u8)> = std::iter::once(SKIN).chain(SHIRTS).map(|c| rgb_to_yuv(c.0, c.1, c.2)).collect();
            let (mw, mh) = (frame.width as usize / 2, frame.height as usize / 2);
            let mut data = vec![0u8; mw * mh];
            frame.pixels.with_planes(|p| {
                for y in 0..mh {
                    for x in 0..mw {
                        let c = yuv(&p, x * 2, y * 2);
                        if targets.iter().any(|t| near(c, *t, 10)) {
                            data[y * mw + x] = 255;
                        }
                    }
                }
            });
            Ok(Mask { width: mw as u32, height: mh as u32, data })
        }
    }
}
