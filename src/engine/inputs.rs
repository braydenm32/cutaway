//! The engine's devices: each video source and audio input can be closed
//! and reopened while everything else keeps running, so a camera that's
//! unplugged (or wasn't plugged in at start) comes back by itself.

use super::audio::InputShared;
use super::feed::AudioHub;
use super::netsrc::NetStatus;
use super::player::PlayerControl;
use super::sources::{self, Running, SourceSlot};
use crate::config::{AudioConfig, SourceConfig};
use crate::devices::{self, DeviceInfo, Kind};
use crate::media::{clock, AudioBlock, NS_PER_SEC};
use anyhow::Result;
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Test pictures and tones never disconnect.
fn is_generated(device: &str) -> bool {
    let d = device.trim().to_lowercase();
    d == "test" || d.starts_with("test:") || d == "pads"
}

/// Whether opening needs the list of connected devices.
pub fn needs_device_list(device: &str) -> bool {
    let d = device.trim().to_lowercase();
    !(is_generated(&d) || d == "screen" || d == "default" || d.is_empty() || d == "media" || d.starts_with("source:") || d.starts_with("ndi:") || d.starts_with("frame:") || d.starts_with("vptz:") || d.starts_with("app:") || d == "pads" || d.starts_with("guest:") || d.starts_with("phone:") || super::netsrc::is_network(&d))
}

pub struct VideoInput {
    pub index: usize,
    pub cfg: SourceConfig,
    pub slot: Arc<SourceSlot>,
    hub: Arc<AudioHub>,
    running: Mutex<Option<Box<dyn Running>>>,
    description: Mutex<String>,
    /// Media player controls, when this source is a player.
    pub player: Mutex<Option<Arc<PlayerControl>>>,
    /// Connection state, for network and NDI inputs.
    pub net: NetStatus,
    links: sources::Links,
}

impl VideoInput {
    /// `peers`: every source's frames by name (auto-framed sources are cut
    /// from one of them).
    pub fn new(index: usize, cfg: SourceConfig, slot: Arc<SourceSlot>, hub: Arc<AudioHub>, peers: Arc<Vec<(String, Arc<SourceSlot>)>>) -> Arc<Self> {
        let framing = cfg.framed_from().is_some().then(|| super::framing::FramingShared::new(&cfg.framing));
        let ptz = super::ptz::PtzCamera::for_source(&cfg);
        Arc::new(VideoInput {
            index,
            cfg,
            slot,
            hub,
            running: Mutex::new(None),
            description: Mutex::new("not connected".into()),
            player: Mutex::new(None),
            net: Default::default(),
            links: sources::Links { peers, framing, ptz: Mutex::new(ptz) },
        })
    }

    /// Pan, tilt and zoom controls (a PTZ camera, or a virtual one).
    pub fn ptz(&self) -> Option<Arc<super::ptz::PtzCamera>> {
        self.links.ptz.lock().unwrap().clone()
    }

    /// Follow new PTZ settings: adjust the controls, or remake them (and
    /// reopen an NDI camera so its receiver gets the new ones).
    pub fn update_ptz(&self, cfg: &SourceConfig, devices: &[DeviceInfo]) {
        let current = self.ptz();
        match &current {
            Some(p) if !p.needs_remaking(cfg) => p.configure(&cfg.ptz),
            None if !cfg.has_ptz() => {}
            _ => {
                *self.links.ptz.lock().unwrap() = super::ptz::PtzCamera::for_source(cfg);
                let ndi = |p: &Option<Arc<super::ptz::PtzCamera>>| p.as_ref().is_some_and(|p| p.ndi_rx.is_some());
                if ndi(&current) || ndi(&self.links.ptz.lock().unwrap()) {
                    let _ = self.open(devices);
                }
            }
        }
    }

    /// Live framing settings and state, for auto-framed sources.
    pub fn framing(&self) -> Option<&Arc<super::framing::FramingShared>> {
        self.links.framing.as_ref()
    }

    /// (Re)open the device. `devices` is only needed for real cameras.
    pub fn open(&self, devices: &[DeviceInfo]) -> Result<String> {
        self.close();
        let mut extras = sources::Extras::default();
        let (running, desc) =
            crate::util::guard("Opening the device", || sources::start_video(self.index, &self.cfg, devices, self.slot.clone(), &self.hub, &self.net, &self.links, &mut extras))?;
        *self.running.lock().unwrap() = Some(running);
        *self.description.lock().unwrap() = desc.clone();
        *self.player.lock().unwrap() = extras.player;
        Ok(desc)
    }

    /// Network/NDI inputs reconnect by themselves (and auto-framed sources
    /// follow their camera); the watchdog only reports them.
    pub fn self_managed(&self) -> bool {
        let d = self.cfg.device.trim().to_lowercase();
        super::netsrc::is_network(&d) || d.starts_with("ndi:") || d == "media" || d.starts_with("frame:") || d.starts_with("guest:") || d.starts_with("phone:")
    }

    pub fn close(&self) {
        let old = self.running.lock().unwrap().take();
        drop(old);
    }

    pub fn is_open(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }

    /// Open and delivering pictures.
    pub fn alive(&self) -> bool {
        self.is_open() && !self.slot.stale(3.0)
    }

    pub fn description(&self) -> String {
        self.description.lock().unwrap().clone()
    }

    pub fn reconnectable(&self) -> bool {
        !is_generated(&self.cfg.device)
    }
}

pub struct AudioInputDev {
    pub index: usize,
    pub cfg: AudioConfig,
    tx: Sender<AudioBlock>,
    hub: Arc<AudioHub>,
    pub shared: Arc<InputShared>,
    running: Mutex<Option<Box<dyn Running>>>,
    description: Mutex<String>,
    channels: AtomicU32,
}

impl AudioInputDev {
    pub fn new(index: usize, cfg: AudioConfig, tx: Sender<AudioBlock>, shared: Arc<InputShared>, hub: Arc<AudioHub>) -> Arc<Self> {
        let channels = AtomicU32::new(cfg.channels);
        Arc::new(AudioInputDev { index, cfg, tx, hub, shared, running: Mutex::new(None), description: Mutex::new("not connected".into()), channels })
    }

    /// (Re)open the device. Returns a description and, if the device has
    /// fewer channels than configured, a note saying so.
    pub fn open(&self, devices: &[DeviceInfo]) -> Result<(String, Option<String>)> {
        self.close();
        let started = crate::util::guard("Opening the device", || sources::start_audio(self.index, &self.cfg, devices, self.tx.clone(), &self.hub))?;
        let desc = format!("{} · {} ch @ {} Hz", started.description, started.channels, started.rate);
        let note = (started.channels as u32 != self.cfg.channels)
            .then(|| format!("the device gives {} channels ({} set)", started.channels, self.cfg.channels));
        self.channels.store(started.channels as u32, Ordering::Relaxed);
        *self.running.lock().unwrap() = Some(started.running);
        *self.description.lock().unwrap() = desc.clone();
        Ok((desc, note))
    }

    pub fn close(&self) {
        let old = self.running.lock().unwrap().take();
        drop(old);
    }

    pub fn is_open(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }

    /// Open and delivering audio.
    pub fn alive(&self) -> bool {
        self.is_open() && clock::now() - self.shared.last_seen() < 3 * NS_PER_SEC
    }

    pub fn description(&self) -> String {
        self.description.lock().unwrap().clone()
    }

    pub fn channels(&self) -> u32 {
        self.channels.load(Ordering::Relaxed)
    }

    pub fn reconnectable(&self) -> bool {
        !is_generated(&self.cfg.device) && !self.from_source()
    }

    /// Sound from a video source: present whenever that source sends it
    /// (silence between clips is normal, not a fault).
    pub fn from_source(&self) -> bool {
        self.cfg.device.trim().starts_with("source:")
    }

    /// Quiet spells and gaps are normal: sound from a source or an app.
    pub fn gaps_expected(&self) -> bool {
        let d = self.cfg.device.trim();
        self.from_source() || d.starts_with("app:")
    }
}

/// Lists devices at most once per call site, only when needed.
#[derive(Default)]
pub struct DeviceCache {
    video: Option<Vec<DeviceInfo>>,
    audio: Option<Vec<DeviceInfo>>,
}

impl DeviceCache {
    pub fn video(&mut self, spec: &str) -> &[DeviceInfo] {
        if !needs_device_list(spec) {
            return &[];
        }
        self.video.get_or_insert_with(|| devices::list(Kind::Video))
    }

    pub fn audio(&mut self, spec: &str) -> &[DeviceInfo] {
        if !needs_device_list(spec) {
            return &[];
        }
        self.audio.get_or_insert_with(|| devices::list(Kind::Audio))
    }
}
