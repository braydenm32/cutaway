//! Audio outputs (via cpal): headphones, presenters' feeds, mix-minus
//! returns. CoreAudio on macOS; Windows audio (WASAPI) and ASIO on Windows.
//!
//! Several feeds can share one device, each on its own pair of channels
//! (an interface's outputs 3-4 for one presenter, 5-6 for a caller): the
//! device is opened once and its stream fills every feed's channels.

use crate::util::LockExt;
use super::audio_in::hosts;
use crate::devices::DeviceInfo;
use crate::engine::sources::Running;
use crate::media::AudioBlock;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| "Unknown output".into())
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for (host, prefix, _) in hosts() {
        let Ok(devices) = host.output_devices() else { continue };
        out.extend(devices.filter_map(|d| {
            let name = format!("{prefix}{}", device_name(&d));
            let cfg = d.default_output_config().ok()?;
            let max_channels = d.supported_output_configs().ok().and_then(|c| c.map(|c| c.channels() as u32).max()).unwrap_or(cfg.channels() as u32);
            Some(DeviceInfo {
                summary: format!("{max_channels} out · {} kHz", cfg.sample_rate() as f64 / 1000.0),
                id: name.clone(),
                name,
                max_channels,
                rate: cfg.sample_rate(),
                modes: Vec::new(),
            })
        }));
    }
    out
}

/// One feed's samples on its way to the device, already at the device rate.
struct Feed {
    id: u64,
    /// Output channels (0-based).
    left: usize,
    right: usize,
    ring: VecDeque<f32>,
}

/// An open output device and the feeds playing on it.
struct DeviceOut {
    feeds: Arc<Mutex<Vec<Feed>>>,
    rate: u32,
    channels: usize,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DeviceOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn registry() -> &'static Mutex<HashMap<String, DeviceOut>> {
    static R: OnceLock<Mutex<HashMap<String, DeviceOut>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn feed_ids() -> &'static AtomicU64 {
    static N: AtomicU64 = AtomicU64::new(1);
    &N
}

/// Find a device by name ("" or "default" = the system output); names
/// match like inputs ("ASIO: …" for ASIO drivers), or partly.
fn find_device(wanted: &str) -> Result<(cpal::Device, String)> {
    if wanted.is_empty() || wanted.eq_ignore_ascii_case("default") {
        let d = cpal::default_host().default_output_device().ok_or_else(|| anyhow!("there's no default audio output"))?;
        let n = device_name(&d);
        return Ok((d, n));
    }
    let lower = wanted.to_lowercase();
    let mut partial = None;
    for (host, prefix, _) in hosts() {
        let Ok(devices) = host.output_devices() else { continue };
        for d in devices {
            let name = format!("{prefix}{}", device_name(&d));
            if name == wanted {
                return Ok((d, name));
            }
            if partial.is_none() && name.to_lowercase().contains(&lower) {
                partial = Some((d, name));
            }
        }
    }
    partial.ok_or_else(|| anyhow!("{wanted} isn't connected"))
}

/// Open `wanted` (if it isn't already), on its own thread: CoreAudio
/// streams must be dropped where they were made.
fn open_device(wanted: &str) -> Result<DeviceOut> {
    let wanted = wanted.trim().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let feeds: Arc<Mutex<Vec<Feed>>> = Arc::new(Mutex::new(Vec::new()));
    let (tx, ready) = mpsc::channel::<Result<(u32, usize)>>();
    let (stop2, feeds2) = (stop.clone(), feeds.clone());
    let thread = std::thread::Builder::new().name("audio output".into()).spawn(move || {
        let start = || -> Result<(cpal::Stream, u32, usize)> {
            let (device, _name) = find_device(&wanted)?;
            let base = device.default_output_config()?;
            let rate = base.sample_rate();
            // Every channel the device has, so any pair can be fed.
            let channels = device
                .supported_output_configs()?
                .filter(|c| c.min_sample_rate() <= rate && c.max_sample_rate() >= rate)
                .map(|c| c.channels())
                .max()
                .unwrap_or(base.channels());
            let config = cpal::StreamConfig { channels, sample_rate: rate, buffer_size: cpal::BufferSize::Default };
            let ch = channels as usize;
            let feeds = feeds2.clone();
            let stream = device.build_output_stream::<f32, _, _>(
                config,
                move |data, _| {
                    data.fill(0.0);
                    let mut feeds = feeds.lock_ok();
                    for f in feeds.iter_mut() {
                        for frame in data.chunks_exact_mut(ch) {
                            if f.ring.len() < 2 {
                                break;
                            }
                            let (l, r) = (f.ring.pop_front().unwrap(), f.ring.pop_front().unwrap());
                            if ch == 1 {
                                frame[0] += (l + r) * 0.5;
                            } else {
                                if f.left < ch {
                                    frame[f.left] += l;
                                }
                                if f.right < ch {
                                    frame[f.right] += r;
                                }
                            }
                        }
                    }
                },
                |e| eprintln!("audio output: {e}"),
                None,
            )?;
            stream.play()?;
            Ok((stream, rate, ch))
        };
        match start() {
            Ok((stream, rate, ch)) => {
                let _ = tx.send(Ok((rate, ch)));
                while !stop2.load(Ordering::Relaxed) {
                    std::thread::park_timeout(Duration::from_millis(300));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    })?;
    let (rate, channels) = ready.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("the audio output didn't start"))??;
    Ok(DeviceOut { feeds, rate, channels, stop, thread: Some(thread) })
}

/// A feed playing on a device; removing it closes the device once nothing
/// else plays there.
struct OutputRunning {
    key: String,
    id: u64,
    stop: Arc<AtomicBool>,
    feeder: Option<JoinHandle<()>>,
}

impl Drop for OutputRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.feeder.take() {
            let _ = t.join();
        }
        let mut reg = registry().lock_ok();
        let empty = match reg.get(&self.key) {
            Some(d) => {
                let mut feeds = d.feeds.lock_ok();
                feeds.retain(|f| f.id != self.id);
                feeds.is_empty()
            }
            None => false,
        };
        if empty {
            reg.remove(&self.key);
        }
    }
}

/// Plays 48 kHz stereo blocks from `rx` on output channels `left` and
/// `right` (1-based) of a device ("" or "default" = the system output).
/// `volume` is a linear gain (f32 bits).
pub fn open_audio_output(name: &str, channels: (u32, u32), rx: Receiver<AudioBlock>, volume: Arc<AtomicU32>) -> Result<Box<dyn Running>> {
    let key = name.trim().to_lowercase();
    let id = feed_ids().fetch_add(1, Ordering::Relaxed);
    let (rate, dev_channels, feeds) = {
        let mut reg = registry().lock_ok();
        if !reg.contains_key(&key) {
            let d = open_device(name)?;
            reg.insert(key.clone(), d);
        }
        let d = &reg[&key];
        (d.rate, d.channels, d.feeds.clone())
    };
    let (left, right) = (channels.0.max(1) as usize - 1, channels.1.max(1) as usize - 1);
    if dev_channels > 1 && (left >= dev_channels || right >= dev_channels) {
        // Don't leave an unused device open.
        let mut reg = registry().lock_ok();
        if reg.get(&key).is_some_and(|d| d.feeds.lock_ok().is_empty()) {
            reg.remove(&key);
        }
        return Err(anyhow!("{name} has {dev_channels} outputs; channels {}-{} don't exist", left + 1, right + 1));
    }
    feeds.lock_ok().push(Feed { id, left, right, ring: VecDeque::new() });

    // Resample 48 kHz to the device rate and keep the delay short (the
    // device clock drifts against the mix).
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let feeder = std::thread::Builder::new().name("audio output feed".into()).spawn(move || {
        let step = 48_000.0 / rate as f64;
        let mut pos = 0.0f64;
        let mut prev = [0.0f32; 2];
        let max_frames = rate as usize / 8; // 125 ms
        while !stop2.load(Ordering::Relaxed) {
            let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
            let g = f32::from_bits(volume.load(Ordering::Relaxed));
            let frames = b.samples.len() / 2;
            if frames == 0 {
                continue;
            }
            let mut out = Vec::with_capacity((frames as f64 / step) as usize * 2 + 4);
            while pos < frames as f64 {
                let i = pos.floor() as usize;
                let f = (pos - i as f64) as f32;
                let at = |k: usize, c: usize| if k == 0 { prev[c] } else { b.samples[(k - 1) * 2 + c] };
                for c in 0..2 {
                    let a = at(i, c);
                    let n = b.samples[i.min(frames - 1) * 2 + c];
                    out.push((a + (n - a) * f) * g);
                }
                pos += step;
            }
            pos -= frames as f64;
            prev = [b.samples[(frames - 1) * 2], b.samples[(frames - 1) * 2 + 1]];
            let mut feeds = feeds.lock_ok();
            if let Some(feed) = feeds.iter_mut().find(|f| f.id == id) {
                feed.ring.extend(out);
                if feed.ring.len() / 2 > max_frames {
                    let drop = feed.ring.len() - max_frames;
                    feed.ring.drain(..drop & !1);
                }
            }
        }
    })?;
    Ok(Box::new(OutputRunning { key, id, stop, feeder: Some(feeder) }))
}
