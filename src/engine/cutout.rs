//! Background removal without a green screen: for every picture of a
//! source, a mask of where the people are (Apple's Vision framework, on the
//! Neural Engine). The compositor uses it as the source's transparency, so
//! the scene's background or the layers below show around the people.
//!
//! Masks are matched to pictures by capture time; the compositor draws a
//! moment behind real time, so the mask for each picture is normally ready
//! by the time it's shown.

use super::sources::SourceSlot;
use crate::media::{Ns, VideoFrame};
use crate::platform::vision::{self, Mask};
use anyhow::Result;
use crossbeam_channel::{bounded, RecvTimeoutError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Masks kept per source (a little more than the compositor looks back).
const KEEP: usize = 12;

/// Recent masks of one source, by the capture time of their picture.
#[derive(Default)]
pub struct MaskSlot {
    masks: Mutex<VecDeque<(Ns, Arc<Mask>)>>,
    /// Masks are being made.
    pub on: AtomicBool,
    /// Masks made per second (×10), for the health window.
    pub rate_x10: AtomicU64,
}

impl MaskSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(MaskSlot::default())
    }

    pub fn push(&self, pts: Ns, mask: Mask) {
        let mut m = self.masks.lock().unwrap();
        m.push_back((pts, Arc::new(mask)));
        while m.len() > KEEP {
            m.pop_front();
        }
    }

    /// The mask for the picture captured at `pts`: its own, or else the
    /// newest one from before it (or the oldest kept, if all are newer).
    pub fn pick(&self, pts: Ns) -> Option<(Ns, Arc<Mask>)> {
        let m = self.masks.lock().unwrap();
        m.iter().rev().find(|(t, _)| *t <= pts).or(m.front()).cloned()
    }

    pub fn clear(&self) {
        self.masks.lock().unwrap().clear();
    }
}

/// Keeps masks coming for one source; dropping it stops them.
pub struct Cutout {
    stop: Arc<AtomicBool>,
    source: Arc<SourceSlot>,
    watcher: u64,
    masks: Arc<MaskSlot>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Cutout {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.source.remove_watcher(self.watcher);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.masks.on.store(false, Ordering::Relaxed);
        self.masks.clear();
    }
}

/// Watcher ids (kept clear of the engine's tap ids and auto-framing's).
static WATCHER_IDS: AtomicU64 = AtomicU64::new(1 << 49);

/// Start making masks for `source`. `quality`: 0 fast, 1 balanced, 2 best.
/// `failed` is told (once) if masks stop coming.
pub fn start(name: &str, source: Arc<SourceSlot>, masks: Arc<MaskSlot>, quality: u8, failed: impl Fn(String) + Send + 'static) -> Result<Cutout> {
    let mut segmenter = vision::segmenter(quality)?;
    let (tx, rx) = bounded::<VideoFrame>(2);
    let watcher = WATCHER_IDS.fetch_add(1, Ordering::Relaxed);
    source.add_watcher(watcher, tx);
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, m2) = (stop.clone(), masks.clone());
    masks.clear();
    masks.on.store(true, Ordering::Relaxed);
    let thread = std::thread::Builder::new().name(format!("cut out {name}")).spawn(move || {
        let mut told = false;
        let (mut count, mut since) = (0u64, Instant::now());
        while !s2.load(Ordering::Relaxed) {
            let mut frame = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // Behind: the newest picture is the one that matters.
            while let Ok(f) = rx.try_recv() {
                frame = f;
            }
            match segmenter.mask(&frame) {
                Ok(mask) => {
                    m2.push(frame.pts, mask);
                    count += 1;
                    told = false;
                }
                Err(e) => {
                    if !told {
                        told = true;
                        failed(format!("{e:#}"));
                    }
                }
            }
            if since.elapsed() >= Duration::from_secs(2) {
                m2.rate_x10.store((count as f64 * 10.0 / since.elapsed().as_secs_f64()).round() as u64, Ordering::Relaxed);
                (count, since) = (0, Instant::now());
            }
        }
    })?;
    Ok(Cutout { stop, source, watcher, masks, thread: Some(thread) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(v: u8) -> Mask {
        Mask { width: 2, height: 2, data: vec![v; 4] }
    }

    #[test]
    fn picks_the_mask_of_the_picture_shown() {
        let slot = MaskSlot::new();
        assert!(slot.pick(100).is_none());
        for (t, v) in [(100, 1), (133, 2), (166, 3)] {
            slot.push(t, mask(v));
        }
        assert_eq!(slot.pick(133).unwrap().0, 133, "its own");
        assert_eq!(slot.pick(150).unwrap().0, 133, "the one before");
        assert_eq!(slot.pick(50).unwrap().0, 100, "all newer: the oldest");
        for t in 0..20 {
            slot.push(200 + t, mask(0));
        }
        assert_eq!(slot.masks.lock().unwrap().len(), KEEP);
    }
}
