//! Headphone / monitor output (via cpal): CoreAudio on macOS, Windows
//! audio (WASAPI) on Windows.

use crate::devices::DeviceInfo;
use crate::engine::sources::Running;
use crate::media::AudioBlock;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| "Unknown output".into())
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let name = device_name(&d);
            let cfg = d.default_output_config().ok()?;
            Some(DeviceInfo {
                summary: format!("{} out · {} kHz", cfg.channels(), cfg.sample_rate() as f64 / 1000.0),
                id: name.clone(),
                name,
                max_channels: cfg.channels() as u32,
                rate: cfg.sample_rate(),
                modes: Vec::new(),
            })
        })
        .collect()
}

struct OutputRunning {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for OutputRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Plays 48 kHz stereo blocks from `rx` on an output device ("" or
/// "default" = the system output). `volume` is a linear gain (f32 bits).
pub fn open_audio_output(name: &str, rx: Receiver<AudioBlock>, volume: Arc<AtomicU32>) -> Result<Box<dyn Running>> {
    let wanted = name.trim().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let (tx, ready) = mpsc::channel::<Result<(u32, usize)>>();
    let (stop2, ring2) = (stop.clone(), ring.clone());
    let player = std::thread::Builder::new().name("monitor output".into()).spawn(move || {
        let start = || -> Result<(cpal::Stream, u32, usize)> {
            let host = cpal::default_host();
            let device = if wanted.is_empty() || wanted.eq_ignore_ascii_case("default") {
                host.default_output_device().ok_or_else(|| anyhow!("there's no default audio output"))?
            } else {
                let lower = wanted.to_lowercase();
                host.output_devices()?
                    .find(|d| device_name(d).to_lowercase().contains(&lower))
                    .ok_or_else(|| anyhow!("{wanted} isn't connected"))?
            };
            let base = device.default_output_config()?;
            let rate = base.sample_rate();
            let channels = base.channels() as usize;
            let config = cpal::StreamConfig { channels: base.channels(), sample_rate: rate, buffer_size: cpal::BufferSize::Default };
            let ring = ring2.clone();
            let stream = device.build_output_stream::<f32, _, _>(
                config,
                move |data, _| {
                    let mut r = ring.lock().unwrap();
                    for frame in data.chunks_exact_mut(channels) {
                        let (l, rr) = if r.len() >= 2 { (r.pop_front().unwrap(), r.pop_front().unwrap()) } else { (0.0, 0.0) };
                        for (c, s) in frame.iter_mut().enumerate() {
                            *s = match c {
                                0 => l,
                                1 => rr,
                                _ => 0.0,
                            };
                        }
                        if channels == 1 {
                            frame[0] = (l + rr) * 0.5;
                        }
                    }
                },
                |e| eprintln!("monitor output: {e}"),
                None,
            )?;
            stream.play()?;
            Ok((stream, rate, channels))
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
    let (rate, _) = ready.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("the audio output didn't start"))??;

    // Feed the ring: resample 48 kHz to the device rate and keep the delay
    // short (the device clock drifts against the mix).
    let stop3 = stop.clone();
    let feeder = std::thread::Builder::new().name("monitor feed".into()).spawn(move || {
        let step = 48_000.0 / rate as f64;
        let mut pos = 0.0f64;
        let mut prev = [0.0f32; 2];
        let max_frames = rate as usize / 8; // 125 ms
        while !stop3.load(Ordering::Relaxed) {
            let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
            let g = f32::from_bits(volume.load(Ordering::Relaxed));
            let frames = b.samples.len() / 2;
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
            if frames > 0 {
                prev = [b.samples[(frames - 1) * 2], b.samples[(frames - 1) * 2 + 1]];
            }
            let mut r = ring.lock().unwrap();
            r.extend(out);
            if r.len() / 2 > max_frames {
                let drop = r.len() - max_frames;
                r.drain(..drop & !1);
            }
        }
    })?;
    Ok(Box::new(OutputRunning { stop, threads: vec![player, feeder] }))
}
