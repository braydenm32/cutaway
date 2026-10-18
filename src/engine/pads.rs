//! Sound pads: jingles, stings, applause and music beds on keys, the
//! Stream Deck, MIDI pads and the phone remote. They play into the mixes
//! through their own input strip ("Sound pads"), so they have a fader, can
//! be ducked under voices and are recorded like any input.

use crate::config::{Pad, PadMode};
use crate::engine::sources::Running;
use crate::media::{clock, AudioBlock};
use crossbeam_channel::Sender;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const RATE: usize = 48_000;
const BLOCK: usize = 480;
/// The "pad" of sounds that aren't on one.
const EXTRA: usize = usize::MAX;

/// A sound on its way out.
struct Voice {
    pad: usize,
    sound: Arc<Vec<f32>>,
    /// Next frame to play.
    pos: usize,
    gain: f32,
    looping: bool,
    /// Fading out: how much the level drops per frame.
    fade_step: Option<f32>,
    level: f32,
}

#[derive(Clone)]
enum Loaded {
    None,
    Loading(PathBuf),
    Ready(PathBuf, Arc<Vec<f32>>),
    Failed(PathBuf, String),
}

struct Bank {
    pads: Vec<Pad>,
    sounds: Vec<Loaded>,
    voices: Vec<Voice>,
}

/// What a pad is doing, for buttons and Stream Deck keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadState {
    pub name: String,
    pub color: String,
    pub playing: bool,
    /// How far through (0-1) while playing.
    pub progress: f32,
    /// Seconds left while playing (looping pads: of this time round).
    pub remaining: f32,
    pub loading: bool,
    pub problem: Option<String>,
}

pub struct PadBank {
    bank: Arc<Mutex<Bank>>,
}

impl PadBank {
    pub fn new(pads: &[Pad]) -> Arc<PadBank> {
        let b = Arc::new(PadBank { bank: Arc::new(Mutex::new(Bank { pads: Vec::new(), sounds: Vec::new(), voices: Vec::new() })) });
        b.set_pads(pads);
        b
    }

    /// New pad settings: sounds whose file changed are (re)loaded in the
    /// background; what's playing carries on if its pad still exists.
    pub fn set_pads(&self, pads: &[Pad]) {
        let mut to_load = Vec::new();
        {
            let mut b = self.bank.lock().unwrap();
            let old = std::mem::take(&mut b.sounds);
            let mut sounds = Vec::with_capacity(pads.len());
            for (i, p) in pads.iter().enumerate() {
                // Keep a sound already loaded from the same file, wherever it was.
                let same = old.iter().find(|l| match l {
                    Loaded::Ready(f, _) | Loaded::Loading(f) | Loaded::Failed(f, _) => *f == p.file,
                    Loaded::None => false,
                });
                let l = match same {
                    Some(l) if !p.file.as_os_str().is_empty() => l.clone(),
                    _ if p.file.as_os_str().is_empty() => Loaded::None,
                    _ => {
                        to_load.push((i, p.file.clone()));
                        Loaded::Loading(p.file.clone())
                    }
                };
                sounds.push(l);
            }
            b.sounds = sounds;
            b.pads = pads.to_vec();
            let n = pads.len();
            b.voices.retain(|v| v.pad < n);
        }
        for (i, file) in to_load {
            let bank = self.bank.clone();
            let _ = std::thread::Builder::new().name("loading a pad".into()).spawn(move || {
                let result = crate::media::audiofile::load(&file);
                let mut b = bank.lock().unwrap();
                // Still wanted (settings may have changed meanwhile)?
                if let Some(slot) = b.sounds.get_mut(i) {
                    if matches!(slot, Loaded::Loading(f) if *f == file) {
                        *slot = match result {
                            Ok(s) => Loaded::Ready(file, Arc::new(s)),
                            Err(e) => Loaded::Failed(file, format!("{e:#}")),
                        };
                    }
                }
            });
        }
    }

    /// Wait until every sound has loaded (tests, headless runs).
    #[cfg(test)]
    pub fn wait_loaded(&self, timeout: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if !self.bank.lock().unwrap().sounds.iter().any(|l| matches!(l, Loaded::Loading(_))) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    pub fn names(&self) -> Vec<String> {
        self.bank.lock().unwrap().pads.iter().map(|p| p.name.clone()).collect()
    }

    pub fn is_playing(&self, i: usize) -> bool {
        self.bank.lock().unwrap().voices.iter().any(|v| v.pad == i && v.fade_step.is_none())
    }

    /// A press: play, or stop if it's playing (once and loop pads). Hold
    /// pads play until `release`.
    pub fn press(&self, i: usize) -> Result<bool, String> {
        let mode = self.bank.lock().unwrap().pads.get(i).map(|p| p.mode).ok_or("there's no such pad")?;
        match mode {
            PadMode::Hold => self.play(i).map(|_| true),
            _ if self.is_playing(i) => {
                self.stop(i);
                Ok(false)
            }
            _ => self.play(i).map(|_| true),
        }
    }

    /// Let go of a pad: stops hold pads.
    pub fn release(&self, i: usize) {
        let hold = self.bank.lock().unwrap().pads.get(i).is_some_and(|p| p.mode == PadMode::Hold);
        if hold {
            self.stop(i);
        }
    }

    /// Play from the start (restarting it if it's playing).
    pub fn play(&self, i: usize) -> Result<(), String> {
        let mut b = self.bank.lock().unwrap();
        let pad = b.pads.get(i).cloned().ok_or("there's no such pad")?;
        let sound = match b.sounds.get(i) {
            Some(Loaded::Ready(_, s)) => s.clone(),
            Some(Loaded::Loading(_)) => return Err(format!("{} is still loading", pad.name)),
            Some(Loaded::Failed(_, e)) => return Err(format!("{}: {e}", pad.name)),
            _ => return Err(format!("{} has no sound file", pad.name)),
        };
        // Restarting: the old one fades quickly under the new.
        for v in b.voices.iter_mut().filter(|v| v.pad == i && v.fade_step.is_none()) {
            v.fade_step = Some(1.0 / (RATE as f32 * 0.02));
        }
        b.voices.push(Voice { pad: i, sound, pos: 0, gain: 10f32.powf(pad.gain_db.clamp(-60.0, 12.0) / 20.0), looping: pad.mode == PadMode::Loop, fade_step: None, level: 1.0 });
        Ok(())
    }

    /// Stop with the pad's fade.
    pub fn stop(&self, i: usize) {
        let mut b = self.bank.lock().unwrap();
        let fade = b.pads.get(i).map(|p| p.fade_ms).unwrap_or(300).max(10) as f32 / 1000.0;
        for v in b.voices.iter_mut().filter(|v| v.pad == i && v.fade_step.is_none()) {
            v.fade_step = Some(1.0 / (RATE as f32 * fade));
        }
    }

    /// Play a sound that isn't on a pad (a stinger's), replacing the last one.
    pub fn play_extra(&self, sound: Arc<Vec<f32>>, gain_db: f32) {
        let mut b = self.bank.lock().unwrap();
        for v in b.voices.iter_mut().filter(|v| v.pad == EXTRA && v.fade_step.is_none()) {
            v.fade_step = Some(1.0 / (RATE as f32 * 0.02));
        }
        b.voices.push(Voice { pad: EXTRA, sound, pos: 0, gain: 10f32.powf(gain_db.clamp(-60.0, 12.0) / 20.0), looping: false, fade_step: None, level: 1.0 });
    }

    pub fn stop_all(&self) {
        let n = self.bank.lock().unwrap().pads.len();
        for i in 0..n {
            self.stop(i);
        }
    }

    pub fn state(&self) -> Vec<PadState> {
        let b = self.bank.lock().unwrap();
        b.pads
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let v = b.voices.iter().find(|v| v.pad == i && v.fade_step.is_none());
                let (playing, progress, remaining) = match v {
                    Some(v) => {
                        let len = (v.sound.len() / 2).max(1);
                        (true, v.pos as f32 / len as f32, (len - v.pos.min(len)) as f32 / RATE as f32)
                    }
                    None => (false, 0.0, 0.0),
                };
                let (loading, problem) = match b.sounds.get(i) {
                    Some(Loaded::Loading(_)) => (true, None),
                    Some(Loaded::Failed(_, e)) => (false, Some(e.clone())),
                    Some(Loaded::None) | None => (false, Some("no sound file".to_string())),
                    _ => (false, None),
                };
                PadState { name: p.name.clone(), color: p.color.clone(), playing, progress, remaining, loading, problem }
            })
            .collect()
    }

    /// Mix what's playing into `out` (interleaved 48 kHz stereo).
    pub fn render(&self, out: &mut [f32]) {
        out.fill(0.0);
        let mut b = self.bank.lock().unwrap();
        for v in b.voices.iter_mut() {
            let len = v.sound.len() / 2;
            if len == 0 {
                v.level = 0.0;
                continue;
            }
            for f in out.chunks_exact_mut(2) {
                if v.pos >= len {
                    if v.looping {
                        v.pos = 0;
                    } else {
                        break;
                    }
                }
                if let Some(step) = v.fade_step {
                    v.level -= step;
                    if v.level <= 0.0 {
                        v.level = 0.0;
                        break;
                    }
                }
                let g = v.gain * v.level;
                f[0] += v.sound[v.pos * 2] * g;
                f[1] += v.sound[v.pos * 2 + 1] * g;
                v.pos += 1;
            }
        }
        b.voices.retain(|v| v.level > 0.0 && (v.looping || v.pos < v.sound.len() / 2));
    }
}

/// The pads' input strip: a block of whatever is playing every 10 ms.
struct PadStrip {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PadStrip {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start_strip(bank: Arc<PadBank>, tx: Sender<AudioBlock>) -> std::io::Result<Box<dyn Running>> {
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("sound pads".into()).spawn(move || {
        let block = 10_000_000i64;
        let mut t = (clock::now() / block + 1) * block;
        let mut buf = vec![0.0f32; BLOCK * 2];
        while !s2.load(Ordering::Relaxed) {
            clock::sleep_until(t + block);
            bank.render(&mut buf);
            let _ = tx.send(AudioBlock { pts: t, rate: RATE as u32, channels: 2, samples: buf.clone() });
            t += block;
            // Asleep or stalled: carry on from now rather than catching up.
            let now = clock::now();
            if now > t + 200_000_000 {
                t = (now / block) * block;
            }
        }
    })?;
    Ok(Box::new(PadStrip { stop, thread: Some(thread) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank_with(sounds: Vec<(PadMode, Vec<f32>)>) -> Arc<PadBank> {
        let pads: Vec<Pad> = sounds.iter().enumerate().map(|(i, (m, _))| Pad { name: format!("P{i}"), file: PathBuf::from(format!("/x{i}.wav")), mode: *m, fade_ms: 10, ..Default::default() }).collect();
        let bank = PadBank { bank: Arc::new(Mutex::new(Bank { pads: pads.clone(), sounds: Vec::new(), voices: Vec::new() })) };
        bank.bank.lock().unwrap().sounds = sounds.into_iter().zip(pads).map(|((_, s), p)| Loaded::Ready(p.file, Arc::new(s))).collect();
        Arc::new(bank)
    }

    fn block(b: &PadBank) -> Vec<f32> {
        let mut out = vec![0.0; BLOCK * 2];
        b.render(&mut out);
        out
    }

    #[test]
    fn plays_once_loops_and_holds() {
        // 15 ms of 0.5, 5 ms of 0.25.
        let once = vec![0.5f32; 720 * 2];
        let looped = vec![0.25f32; 240 * 2];
        let held = vec![0.1f32; 48_000 * 2];
        let b = bank_with(vec![(PadMode::Once, once), (PadMode::Loop, looped), (PadMode::Hold, held)]);
        assert!(block(&b).iter().all(|s| *s == 0.0), "silent until pressed");

        assert_eq!(b.press(0), Ok(true));
        let first = block(&b);
        assert!(first.iter().all(|s| *s == 0.5));
        let second = block(&b);
        assert!(second[..480].iter().all(|s| *s == 0.5) && second[480..].iter().all(|s| *s == 0.0), "ends after 15 ms");
        assert!(!b.is_playing(0));

        b.press(1).unwrap();
        for _ in 0..5 {
            assert!(block(&b).iter().all(|s| *s == 0.25), "loops");
        }
        assert_eq!(b.press(1), Ok(false), "a second press stops it");
        let fading = block(&b);
        assert!(fading[0] > 0.2 && fading[fading.len() - 1] == 0.0, "fades out");
        assert!(block(&b).iter().all(|s| *s == 0.0));

        b.press(2).unwrap();
        b.press(2).unwrap(); // still held (a key repeating)
        assert!(block(&b).iter().any(|s| *s > 0.0));
        b.release(2);
        block(&b);
        assert!(block(&b).iter().all(|s| *s == 0.0), "stops when let go");
    }

    #[test]
    fn state_and_problems() {
        let b = bank_with(vec![(PadMode::Once, vec![0.5; 4800 * 2])]);
        b.press(0).unwrap();
        block(&b);
        let s = &b.state()[0];
        assert!(s.playing && (s.progress - 0.1).abs() < 0.01 && (s.remaining - 0.09).abs() < 0.001, "{s:?}");
        let missing = PadBank::new(&[Pad { name: "Nothing".into(), ..Default::default() }]);
        assert!(missing.play(0).unwrap_err().contains("no sound file"));
        assert!(missing.press(5).is_err());
    }

    #[test]
    fn loads_files_in_the_background() {
        let path = std::env::temp_dir().join(format!("cutaway-pad-{}.wav", std::process::id()));
        let mut w = crate::media::wav::WavWriter::create(&path, crate::media::wav::SampleFormat::from_bits(16), 2, 48_000).unwrap();
        w.write(&vec![0.25f32; 9600]).unwrap();
        w.finish().unwrap();
        let b = PadBank::new(&[Pad { name: "Sting".into(), file: path.clone(), ..Default::default() }]);
        assert!(b.wait_loaded(std::time::Duration::from_secs(10)));
        b.press(0).unwrap();
        let out = block(&b);
        let _ = std::fs::remove_file(&path);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 0.001), "{:?}", &out[..4]);
    }
}
