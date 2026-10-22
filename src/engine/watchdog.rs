//! Background housekeeping: notices devices that stop delivering and
//! reopens them, and keeps the health and disk figures up to date.

use crate::util::LockExt;
use super::inputs::{AudioInputDev, DeviceCache, VideoInput};
use super::{LogLevel, SharedLineup, Status, Waker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Reconnect bookkeeping for one device.
#[derive(Default)]
struct Outage {
    down: bool,
    /// Has worked at least once (so "lost" vs "not found" wording).
    ever_alive: bool,
    attempts: u32,
    next_try: Option<Instant>,
    /// Just added (and being opened): left alone until then.
    settle_until: Option<Instant>,
}

impl Outage {
    fn backoff(&self) -> Duration {
        Duration::from_secs((2 * self.attempts as u64).clamp(2, 10))
    }
}

pub trait Device {
    fn name(&self) -> &str;
    fn alive(&self) -> bool;
    fn reconnectable(&self) -> bool;
    /// Reconnects by itself (network inputs): only reported, never reopened.
    fn self_managed(&self) -> bool {
        false
    }
    fn reopen(&self, cache: &mut DeviceCache) -> anyhow::Result<String>;
}

impl Device for VideoInput {
    fn name(&self) -> &str {
        &self.cfg.name
    }
    fn alive(&self) -> bool {
        VideoInput::alive(self)
    }
    fn reconnectable(&self) -> bool {
        VideoInput::reconnectable(self)
    }
    fn self_managed(&self) -> bool {
        VideoInput::self_managed(self)
    }
    fn reopen(&self, cache: &mut DeviceCache) -> anyhow::Result<String> {
        let devs = cache.video(&self.cfg.device).to_vec();
        self.open(&devs)
    }
}

impl Device for AudioInputDev {
    fn name(&self) -> &str {
        &self.cfg.name
    }
    fn alive(&self) -> bool {
        AudioInputDev::alive(self)
    }
    fn reconnectable(&self) -> bool {
        AudioInputDev::reconnectable(self)
    }

    fn reopen(&self, cache: &mut DeviceCache) -> anyhow::Result<String> {
        let devs = cache.audio(&self.cfg.device).to_vec();
        self.open(&devs).map(|(d, _)| d)
    }
}

pub struct Watchdog {
    /// The sources and inputs (they change when they're added or removed).
    pub lineup: SharedLineup,
    pub status: Arc<Mutex<Status>>,
    pub stop: Arc<AtomicBool>,
    pub waker: Waker,
    /// Extra work every second (health and disk figures).
    pub every_second: Vec<Box<dyn FnMut() + Send>>,
}

impl Watchdog {
    /// The devices now, each with its reconnect bookkeeping (kept for those
    /// that were there before; new ones get a moment to open).
    fn devices(&self, seen: &mut u64, outages: &mut HashMap<usize, Outage>) -> Vec<Arc<dyn Device + Send + Sync>> {
        let l = self.lineup.lock_ok();
        let mut list: Vec<Arc<dyn Device + Send + Sync>> = Vec::new();
        for v in &l.video {
            list.push(v.clone());
        }
        for a in &l.audio {
            list.push(a.clone());
        }
        let first = *seen == 0;
        if l.generation != *seen {
            *seen = l.generation;
            let keys: Vec<usize> = list.iter().map(|d| key(d)).collect();
            outages.retain(|k, _| keys.contains(k));
            for k in keys {
                outages.entry(k).or_insert_with(|| Outage { settle_until: (!first).then(|| Instant::now() + Duration::from_secs(6)), ..Default::default() });
            }
        }
        list
    }

    pub fn run(mut self) {
        let started = Instant::now();
        let mut outages: HashMap<usize, Outage> = HashMap::new();
        let mut seen = 0u64;
        let mut last_second = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
            if last_second.elapsed() >= Duration::from_secs(1) {
                last_second = Instant::now();
                for f in &mut self.every_second {
                    f();
                }
            }
            // Devices take a moment to deliver their first frames.
            if started.elapsed() < Duration::from_secs(5) {
                continue;
            }
            let mut cache = DeviceCache::default();
            let mut changed = false;
            let devices = self.devices(&mut seen, &mut outages);
            for dev in &devices {
                if self.stop.load(Ordering::Relaxed) {
                    return;
                }
                let Some(out) = outages.get_mut(&key(dev)) else { continue };
                if out.settle_until.is_some_and(|t| Instant::now() < t) {
                    continue;
                }
                let name = dev.name().to_string();
                if dev.alive() {
                    out.ever_alive = true;
                    if out.down {
                        let mut st = self.status.lock_ok();
                        st.offline.remove(&name);
                        st.push(LogLevel::Info, format!("{name}: back"));
                        *out = Outage { ever_alive: true, ..Default::default() };
                        changed = true;
                    }
                    continue;
                }
                let managed = dev.self_managed();
                if !dev.reconnectable() && !managed {
                    continue;
                }
                let now = Instant::now();
                if !out.down {
                    out.down = true;
                    out.next_try = Some(now); // try straight away
                    let mut st = self.status.lock_ok();
                    st.offline.insert(name.clone());
                    if out.ever_alive {
                        st.push(LogLevel::Warning, if managed { format!("{name}: no signal (waiting for it to come back)") } else { format!("{name}: no signal. Reconnecting automatically…") });
                    }
                    changed = true;
                }
                if managed {
                    if changed {
                        (self.waker)();
                    }
                    continue;
                }
                if out.next_try.is_some_and(|t| now >= t) {
                    match dev.reopen(&mut cache) {
                        Ok(desc) => {
                            // Give it time to deliver before judging it.
                            out.attempts = 0;
                            out.next_try = Some(now + Duration::from_secs(6));
                            self.status.lock_ok().push(LogLevel::Info, format!("{name}: reopened ({desc})"));
                        }
                        Err(e) => {
                            out.attempts += 1;
                            out.next_try = Some(now + out.backoff());
                            if out.attempts == 1 && out.ever_alive {
                                self.status.lock_ok().push(LogLevel::Warning, format!("{name}: waiting for it to come back ({e:#})"));
                            }
                        }
                    }
                    changed = true;
                }
            }
            if changed {
                (self.waker)();
            }
        }
    }
}

/// A device's identity while it's in the line-up.
fn key(d: &Arc<dyn Device + Send + Sync>) -> usize {
    Arc::as_ptr(d) as *const () as usize
}
