//! Audio interfaces and microphones (via cpal): CoreAudio on macOS;
//! Windows audio (WASAPI) and, in builds with the `asio` feature, ASIO
//! drivers on Windows.

use crate::devices::DeviceInfo;
use crate::engine::sources::AudioStarted;
use crate::media::{clock, AudioBlock, NS_PER_SEC};
use crate::platform::AudioSink;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| "Unknown input".into())
}

/// The audio systems to look in, with the prefix their devices' names get
/// in Cutaway, and whether they stamp audio with the host clock.
pub(crate) fn hosts() -> Vec<(cpal::Host, &'static str, bool)> {
    #[allow(unused_mut)]
    let mut v = vec![(cpal::default_host(), "", true)];
    // ASIO drivers keep their own time: their audio is stamped on arrival.
    #[cfg(all(windows, feature = "asio"))]
    if let Ok(h) = cpal::host_from_id(cpal::HostId::Asio) {
        v.push((h, "ASIO: ", false));
    }
    v
}

pub fn list_audio_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for (host, prefix, _) in hosts() {
        let Ok(devices) = host.input_devices() else { continue };
        out.extend(devices.filter_map(|d| {
            let name = format!("{prefix}{}", device_name(&d));
            let max_channels = d.supported_input_configs().ok()?.map(|c| c.channels() as u32).max().unwrap_or(0);
            if max_channels == 0 {
                return None;
            }
            let rate = d.default_input_config().map(|c| c.sample_rate()).unwrap_or(48_000);
            Some(DeviceInfo {
                summary: format!("{max_channels} in · {} kHz", rate as f64 / 1000.0),
                id: name.clone(),
                name,
                max_channels,
                rate,
                modes: Vec::new(),
            })
        }));
    }
    out
}

/// Keeps the input running; the stream lives on its own thread (CoreAudio
/// streams must be dropped where they were made).
struct InputRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for InputRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

pub fn open_audio_input(dev: Option<&DeviceInfo>, channels: u32, sink: AudioSink) -> Result<AudioStarted> {
    #[cfg(target_os = "macos")]
    super::macos::check_microphone_access()?;
    let wanted = dev.map(|d| d.name.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(String, u32, u16)>>();
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new().name("audio input".into()).spawn(move || {
        let start = || -> Result<(cpal::Stream, String, u32, u16)> {
            let (device, name, host_time) = match &wanted {
                Some(name) => hosts()
                    .into_iter()
                    .find_map(|(host, prefix, host_time)| {
                        let d = host.input_devices().ok()?.find(|d| format!("{prefix}{}", device_name(d)) == *name)?;
                        Some((d, name.clone(), host_time))
                    })
                    .ok_or_else(|| anyhow!("{name} is no longer connected"))?,
                None => {
                    let d = cpal::default_host().default_input_device().ok_or_else(|| anyhow!("there's no default audio input"))?;
                    let n = device_name(&d);
                    (d, n, true)
                }
            };
            let base = device.default_input_config()?;
            let rate = base.sample_rate();
            // Capture every channel the device has; keep the ones asked for.
            let dev_channels = device
                .supported_input_configs()?
                .filter(|c| c.min_sample_rate() <= rate && c.max_sample_rate() >= rate)
                .map(|c| c.channels())
                .max()
                .unwrap_or(base.channels());
            let keep = (channels.max(1) as u16).min(dev_channels);
            let config = cpal::StreamConfig { channels: dev_channels, sample_rate: rate, buffer_size: cpal::BufferSize::Default };
            let ch = dev_channels as usize;
            let k = keep as usize;
            let stream = device.build_input_stream::<f32, _, _>(
                config,
                move |data, info| {
                    let frames = data.len() / ch;
                    // CoreAudio and WASAPI stamp input with host time, the
                    // same clock as everything else; ASIO audio is stamped
                    // as it arrives (the mixer smooths the jitter).
                    let now = clock::now();
                    let arrival = now - frames as i64 * NS_PER_SEC / rate as i64;
                    let mut pts = if host_time { info.timestamp().capture.as_nanos() as i64 } else { arrival };
                    if (pts - now).abs() > 5 * NS_PER_SEC {
                        pts = arrival;
                    }
                    let samples = if k == ch {
                        data.to_vec()
                    } else {
                        let mut v = Vec::with_capacity(frames * k);
                        for f in data.chunks_exact(ch) {
                            v.extend_from_slice(&f[..k]);
                        }
                        v
                    };
                    sink(AudioBlock { pts, rate, channels: k as u16, samples });
                },
                |e| eprintln!("audio input: {e}"),
                None,
            )?;
            stream.play()?;
            Ok((stream, name, rate, keep))
        };
        match start() {
            Ok((stream, name, rate, keep)) => {
                let _ = tx.send(Ok((name, rate, keep)));
                while !stop2.load(Ordering::Relaxed) {
                    std::thread::park_timeout(Duration::from_millis(500));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    })?;
    let (name, rate, keep) = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| {
            // Don't let it start capturing later with nothing to stop it.
            stop.store(true, Ordering::Relaxed);
            anyhow!("the audio device didn't start")
        })??;
    Ok(AudioStarted { running: Box::new(InputRunning { stop, thread: Some(thread) }), description: name, rate, channels: keep })
}
