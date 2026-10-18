//! Turning compressed media (from a file, a network stream or NDI) into
//! source pictures and mixer audio on the host clock.

use super::decode::{create_aac_decoder, create_video_decoder, AudioDecoder, DecoderSetup, Reorder, ToMix, VideoDecoder};
use super::sources::SourceSlot;
use crate::media::{clock, AudioBlock, Ns, VideoCodec, VideoFrame, NS_PER_SEC};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Audio from video sources (media players, network inputs), for audio
/// inputs set to "source:<name>".
#[derive(Default)]
pub struct AudioHub {
    map: Mutex<HashMap<String, Sender<AudioBlock>>>,
    /// The sound pads, for an input set to "pads".
    pub pads: Mutex<Option<Arc<super::pads::PadBank>>>,
}

impl AudioHub {
    pub fn new() -> Arc<Self> {
        Arc::new(AudioHub::default())
    }

    fn key(name: &str) -> String {
        name.trim().to_lowercase()
    }

    pub fn register(&self, name: &str, tx: Sender<AudioBlock>) {
        self.map.lock().unwrap().insert(Self::key(name), tx);
    }

    pub fn unregister(&self, name: &str) {
        self.map.lock().unwrap().remove(&Self::key(name));
    }

    pub fn send(&self, name: &str, block: AudioBlock) {
        if let Some(tx) = self.map.lock().unwrap().get(&Self::key(name)) {
            let _ = tx.try_send(block);
        }
    }
}

/// Keeps the hub registration for an audio input.
pub struct HubInput {
    pub hub: Arc<AudioHub>,
    pub name: String,
}

impl Drop for HubInput {
    fn drop(&mut self) {
        self.hub.unregister(&self.name);
    }
}

/// Delivers pictures to a source slot at their (host clock) times, like a
/// camera would.
pub struct Pacer {
    tx: Option<Sender<VideoFrame>>,
    thread: Option<JoinHandle<()>>,
}

impl Pacer {
    pub fn new(slot: Arc<SourceSlot>, stop: Arc<AtomicBool>) -> Pacer {
        let (tx, rx): (Sender<VideoFrame>, Receiver<VideoFrame>) = bounded(120);
        let thread = std::thread::Builder::new()
            .name("pacer".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Ok(f) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
                    let wait = f.pts - clock::now();
                    if wait > 2 * NS_PER_SEC {
                        continue; // nonsense time: drop
                    }
                    clock::sleep_until(f.pts);
                    slot.push(f);
                }
            })
            .expect("thread");
        Pacer { tx: Some(tx), thread: Some(thread) }
    }

    pub fn send(&self, f: VideoFrame) {
        if let Some(tx) = &self.tx {
            let _ = tx.send_timeout(f, Duration::from_millis(500));
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Decodes one incoming stream (network or NDI) and plays it out a fixed
/// delay behind real time, which absorbs network jitter.
pub struct Feeder {
    pub name: String,
    hub: Arc<AudioHub>,
    out_size: (u32, u32),
    latency: Ns,
    pacer: Pacer,
    video: Option<(VideoCodec, Vec<Vec<u8>>, Box<dyn VideoDecoder>)>,
    pending_params: Option<(VideoCodec, Vec<Vec<u8>>)>,
    reorder: Reorder,
    bframes: bool,
    waiting_for_key: bool,
    audio: Option<(Vec<u8>, Box<dyn AudioDecoder>, ToMix)>,
    audio_next: Option<Ns>,
    /// host time = stream time + offset
    offset: Option<Ns>,
    pub errors: Vec<String>,
    decoded: Vec<VideoFrame>,
}

impl Feeder {
    pub fn new(name: &str, slot: Arc<SourceSlot>, hub: Arc<AudioHub>, out_size: (u32, u32), latency_ms: u32, stop: Arc<AtomicBool>) -> Feeder {
        Feeder {
            name: name.to_string(),
            hub,
            out_size,
            latency: latency_ms.clamp(20, 5000) as i64 * 1_000_000,
            pacer: Pacer::new(slot, stop),
            video: None,
            pending_params: None,
            reorder: Reorder::new(0),
            bframes: false,
            waiting_for_key: true,
            audio: None,
            audio_next: None,
            offset: None,
            errors: Vec::new(),
            decoded: Vec::new(),
        }
    }

    /// The stream restarted (new connection): forget timing and decoders.
    pub fn reset(&mut self) {
        self.video = None;
        self.pending_params = None;
        self.audio = None;
        self.audio_next = None;
        self.offset = None;
        self.reorder.clear();
        self.bframes = false;
        self.waiting_for_key = true;
    }

    /// Host time for a stream time, re-anchoring after jumps.
    fn host(&mut self, t: Ns) -> Ns {
        let target = clock::now() + self.latency;
        match self.offset {
            Some(o) if ((t + o) - target).abs() < 3 * NS_PER_SEC / 2 => t + o,
            _ => {
                self.offset = Some(target - t);
                self.audio_next = None;
                target
            }
        }
    }

    fn note(&mut self, e: String) {
        if self.errors.len() < 20 {
            eprintln!("{}: {e}", self.name);
            self.errors.push(e);
        }
    }

    /// The most recent decoding problem, if any.
    pub fn take_error(&mut self) -> Option<String> {
        self.errors.pop()
    }

    pub fn video_config(&mut self, codec: VideoCodec, params: Vec<Vec<u8>>) {
        if self.video.as_ref().is_some_and(|v| v.0 == codec && v.1 == params) {
            return;
        }
        self.pending_params = Some((codec, params));
    }

    /// One access unit of AVCC NAL units (parameter sets may be inline).
    pub fn video(&mut self, codec: VideoCodec, pts: Ns, dts: Ns, keyframe: bool, data: &[u8]) {
        if pts != dts {
            self.bframes = true;
        }
        if let Some((c, params)) = self.pending_params.take() {
            match create_video_decoder(&DecoderSetup { codec: c, parameter_sets: params.clone(), width: self.out_size.0, height: self.out_size.1, output: self.out_size }) {
                Ok(d) => {
                    self.video = Some((c, params, d));
                    self.waiting_for_key = true;
                }
                Err(e) => self.note(format!("{e:#}")),
            }
        }
        if self.video.as_ref().is_none_or(|v| v.0 != codec) {
            return;
        }
        if self.waiting_for_key {
            if !keyframe {
                return;
            }
            self.waiting_for_key = false;
        }
        let host = self.host(pts);
        self.decoded.clear();
        let mut decoded = std::mem::take(&mut self.decoded);
        let r = self.video.as_mut().unwrap().2.decode(data, host, &mut decoded);
        if let Err(e) = r {
            self.note(format!("{e:#}"));
            self.waiting_for_key = true;
            self.decoded = decoded;
            return;
        }
        // Streams with B-frames decode out of order: hold a few to sort.
        self.reorder.set_depth(if self.bframes { 3 } else { 0 });
        for f in decoded.drain(..) {
            let f = if (f.width, f.height) != self.out_size { f.scaled(self.out_size.0, self.out_size.1) } else { f };
            if let Some(out) = self.reorder.push(f) {
                self.pacer.send(out);
            }
        }
        self.decoded = decoded;
    }

    pub fn audio_config(&mut self, asc: &[u8]) {
        if self.audio.as_ref().is_some_and(|a| a.0 == asc) {
            return;
        }
        match create_aac_decoder(asc) {
            Ok(d) => {
                let mix = ToMix::new(d.rate(), d.channels());
                self.audio = Some((asc.to_vec(), d, mix));
                self.audio_next = None;
            }
            Err(e) => self.note(format!("{e:#}")),
        }
    }

    /// One raw AAC frame at stream time `pts`.
    pub fn audio(&mut self, pts: Ns, frame: &[u8]) {
        // The decoder's own delay makes its output that much late.
        let delay = self.audio.as_ref().map(|(_, d, _)| d.delay() as i64 * NS_PER_SEC / d.rate().max(1) as i64).unwrap_or(0);
        let host = self.host(pts - delay);
        let Some((_, dec, mix)) = self.audio.as_mut() else { return };
        let pcm = match dec.decode(frame) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{e:#}");
                self.note(msg);
                return;
            }
        };
        if !mix.matches(dec.rate(), dec.channels()) {
            *mix = ToMix::new(dec.rate(), dec.channels());
        }
        let out = mix.push(&pcm);
        if out.is_empty() {
            return;
        }
        let at = match self.audio_next {
            Some(n) if (n - host).abs() < 40_000_000 => n,
            _ => host,
        };
        let frames = out.len() / 2;
        self.audio_next = Some(at + frames as i64 * NS_PER_SEC / 48_000);
        self.hub.send(&self.name, AudioBlock { pts: at, rate: 48_000, channels: 2, samples: out });
    }

    /// Raw audio (already PCM) at stream time `pts`, any rate/channels.
    pub fn pcm(&mut self, pts: Ns, rate: u32, channels: u16, samples: &[f32], mix: &mut Option<ToMix>) {
        let host = self.host(pts);
        if mix.as_ref().is_none_or(|m| !m.matches(rate, channels)) {
            *mix = Some(ToMix::new(rate, channels));
        }
        let out = mix.as_mut().unwrap().push(samples);
        if out.is_empty() {
            return;
        }
        let at = match self.audio_next {
            Some(n) if (n - host).abs() < 40_000_000 => n,
            _ => host,
        };
        self.audio_next = Some(at + (out.len() / 2) as i64 * NS_PER_SEC / 48_000);
        self.hub.send(&self.name, AudioBlock { pts: at, rate: 48_000, channels: 2, samples: out });
    }

    /// An already-decoded picture at stream time `pts` (NDI).
    pub fn picture(&mut self, pts: Ns, f: VideoFrame) {
        let host = self.host(pts);
        let f = if (f.width, f.height) != self.out_size { f.scaled(self.out_size.0, self.out_size.1) } else { f };
        self.pacer.send(VideoFrame { pts: host, ..f });
    }

    /// The video codec this stream announced.
    pub fn codec(&self) -> Option<VideoCodec> {
        self.pending_params.as_ref().map(|p| p.0).or_else(|| self.video.as_ref().map(|v| v.0))
    }
}
