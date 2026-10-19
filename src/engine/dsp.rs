//! Audio processing for the mixes: a voice channel strip (high-pass, gate,
//! EQ, compressor, de-esser), a limiter, a loudness meter (EBU R128 /
//! ITU-R BS.1770) and stream levelling. Everything works on interleaved
//! stereo at 48 kHz, one 10 ms block at a time, with no added latency.

use crate::config::VoiceProcessing;
use std::collections::VecDeque;

const FS: f64 = 48_000.0;

fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn lin_to_db(v: f64) -> f64 {
    if v <= 1e-10 {
        -200.0
    } else {
        20.0 * v.log10()
    }
}

/// Coefficient for a one-pole smoother with time constant `ms`.
fn coeff(ms: f64) -> f64 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * FS)).exp()
    }
}

/// Second-order filter (RBJ cookbook), stereo state.
#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Biquad { b0: b[0] / a[0], b1: b[1] / a[0], b2: b[2] / a[0], a1: a[1] / a[0], a2: a[2] / a[0], z1: [0.0; 2], z2: [0.0; 2] }
    }

    pub fn raw(b: [f64; 3], a: [f64; 3]) -> Self {
        Self::new(b, a)
    }

    pub fn highpass(f: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Self::new([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    pub fn lowpass(f: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Self::new([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    pub fn bandpass(f: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Self::new([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    pub fn peaking(f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Self::new([1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a])
    }

    pub fn low_shelf(f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, s) = (w.cos(), w.sin());
        let alpha = s / 2.0 * std::f64::consts::SQRT_2; // shelf slope 1
        let sq = 2.0 * a.sqrt() * alpha;
        Self::new(
            [a * ((a + 1.0) - (a - 1.0) * c + sq), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - sq)],
            [(a + 1.0) + (a - 1.0) * c + sq, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - sq],
        )
    }

    pub fn high_shelf(f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f / FS;
        let (c, s) = (w.cos(), w.sin());
        let alpha = s / 2.0 * std::f64::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * alpha;
        Self::new(
            [a * ((a + 1.0) + (a - 1.0) * c + sq), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - sq)],
            [(a + 1.0) - (a - 1.0) * c + sq, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sq],
        )
    }

    #[inline]
    pub fn run(&mut self, ch: usize, x: f64) -> f64 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            f[0] = self.run(0, f[0] as f64) as f32;
            f[1] = self.run(1, f[1] as f64) as f32;
        }
    }
}

struct Gate {
    open_lin: f64,
    close_lin: f64,
    env: f64,
    gain: f64,
    hold: usize,
    held: usize,
    open: bool,
    floor: f64,
}

impl Gate {
    fn new(threshold_db: f64) -> Self {
        Gate {
            open_lin: db_to_lin(threshold_db),
            close_lin: db_to_lin(threshold_db - 6.0),
            env: 0.0,
            gain: 0.0,
            hold: (0.2 * FS) as usize,
            held: 0,
            open: false,
            floor: db_to_lin(-40.0),
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        let rel = coeff(50.0);
        let (open_c, close_c) = (coeff(1.0), coeff(120.0));
        for f in buf.chunks_exact_mut(2) {
            let x = (f[0].abs().max(f[1].abs())) as f64;
            self.env = if x > self.env { x } else { x + rel * (self.env - x) };
            if self.env >= self.open_lin {
                self.open = true;
                self.held = 0;
            } else if self.open && self.env < self.close_lin {
                self.held += 1;
                if self.held > self.hold {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { self.floor };
            let c = if target > self.gain { open_c } else { close_c };
            self.gain = target + c * (self.gain - target);
            f[0] = (f[0] as f64 * self.gain) as f32;
            f[1] = (f[1] as f64 * self.gain) as f32;
        }
    }
}

struct Compressor {
    threshold: f64,
    ratio: f64,
    knee: f64,
    makeup: f64,
    env: f64,
    gr_db: f64,
    pub reduction_db: f64,
}

impl Compressor {
    fn new(threshold_db: f64, ratio: f64, makeup_db: f64) -> Self {
        Compressor { threshold: threshold_db, ratio: ratio.max(1.0), knee: 6.0, makeup: db_to_lin(makeup_db), env: 0.0, gr_db: 0.0, reduction_db: 0.0 }
    }

    /// Gain reduction (dB, positive) for a level.
    fn curve(&self, level_db: f64) -> f64 {
        let over = level_db - self.threshold;
        let slope = 1.0 - 1.0 / self.ratio;
        if over <= -self.knee / 2.0 {
            0.0
        } else if over >= self.knee / 2.0 {
            over * slope
        } else {
            slope * (over + self.knee / 2.0).powi(2) / (2.0 * self.knee)
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        let (att, rel) = (coeff(5.0), coeff(150.0));
        let mut max_gr: f64 = 0.0;
        for f in buf.chunks_exact_mut(2) {
            let x = (f[0].abs().max(f[1].abs())) as f64;
            let c = if x > self.env { att } else { rel };
            self.env = x + c * (self.env - x);
            let target = self.curve(lin_to_db(self.env));
            let c = if target > self.gr_db { att } else { rel };
            self.gr_db = target + c * (self.gr_db - target);
            max_gr = max_gr.max(self.gr_db);
            let g = db_to_lin(-self.gr_db) * self.makeup;
            f[0] = (f[0] as f64 * g) as f32;
            f[1] = (f[1] as f64 * g) as f32;
        }
        self.reduction_db = max_gr;
    }
}

/// Turns down the "s" band when it gets loud.
struct DeEsser {
    split: Biquad,
    detect: Biquad,
    threshold: f64,
    env: f64,
    gain: f64,
}

impl DeEsser {
    fn new(threshold_db: f64) -> Self {
        DeEsser { split: Biquad::lowpass(4500.0, std::f64::consts::FRAC_1_SQRT_2), detect: Biquad::bandpass(7000.0, 1.4), threshold: threshold_db, env: 0.0, gain: 1.0 }
    }

    fn process(&mut self, buf: &mut [f32]) {
        let (att, rel) = (coeff(1.0), coeff(60.0));
        for f in buf.chunks_exact_mut(2) {
            let (l, r) = (f[0] as f64, f[1] as f64);
            let d = self.detect.run(0, (l + r) * 0.5).abs();
            let c = if d > self.env { att } else { rel };
            self.env = d + c * (self.env - d);
            let over = lin_to_db(self.env) - self.threshold;
            let target = if over > 0.0 { db_to_lin(-(over * 0.8).min(12.0)) } else { 1.0 };
            let c = if target < self.gain { att } else { rel };
            self.gain = target + c * (self.gain - target);
            let (ll, rl) = (self.split.run(0, l), self.split.run(1, r));
            f[0] = (ll + (l - ll) * self.gain) as f32;
            f[1] = (rl + (r - rl) * self.gain) as f32;
        }
    }
}

/// Noise reduction with RNNoise: a small neural network trained on speech
/// that turns down everything that isn't a voice. It works on 10 ms frames
/// at 48 kHz (stereo: each side on its own; a mono mic on both sides is
/// cleaned once) and delays the input by 10 ms.
pub struct Denoiser {
    states: [Box<nnnoiseless::DenoiseState<'static>>; 2],
    amount: f32,
    /// Waiting for a whole frame (interleaved stereo).
    pending: Vec<f32>,
    /// Cleaned, waiting to go out (interleaved stereo).
    ready: VecDeque<f32>,
    /// The untouched input, delayed to line up with the cleaned one.
    dry: VecDeque<f32>,
    primed: bool,
}

const NR_FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE;

impl Denoiser {
    pub fn new(amount: f32) -> Self {
        Denoiser {
            states: [nnnoiseless::DenoiseState::new(), nnnoiseless::DenoiseState::new()],
            amount: amount.clamp(0.0, 1.0),
            pending: Vec::with_capacity(NR_FRAME * 4),
            ready: VecDeque::with_capacity(NR_FRAME * 4),
            dry: VecDeque::with_capacity(NR_FRAME * 6),
            primed: false,
        }
    }

    /// One 10 ms frame of each side through RNNoise.
    fn frame(&mut self, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let scale = |v: &[f32]| -> Vec<f32> { v.iter().map(|x| x * 32767.0).collect() };
        let mut out_l = vec![0f32; NR_FRAME];
        self.states[0].process_frame(&mut out_l, &scale(l));
        let out_r = if l == r {
            // Keep the second state in step for when the sides differ.
            let mut o = vec![0f32; NR_FRAME];
            self.states[1].process_frame(&mut o, &scale(r));
            out_l.clone()
        } else {
            let mut o = vec![0f32; NR_FRAME];
            self.states[1].process_frame(&mut o, &scale(r));
            o
        };
        (out_l.iter().map(|v| v / 32767.0).collect(), out_r.iter().map(|v| v / 32767.0).collect())
    }

    /// Clean an interleaved stereo buffer in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        if !self.primed {
            self.primed = true;
            // Blocks that aren't whole frames wait for one: start a frame
            // behind so there's always output. RNNoise itself is a frame
            // behind, so the dry side waits for that too.
            let extra = if (buf.len() / 2) % NR_FRAME == 0 { 0 } else { NR_FRAME };
            self.ready.extend(std::iter::repeat_n(0.0, extra * 2));
            self.dry.extend(std::iter::repeat_n(0.0, (extra + NR_FRAME) * 2));
        }
        self.pending.extend_from_slice(buf);
        self.dry.extend(buf.iter().copied());
        while self.pending.len() >= NR_FRAME * 2 {
            let l: Vec<f32> = self.pending[..NR_FRAME * 2].iter().step_by(2).copied().collect();
            let r: Vec<f32> = self.pending[1..NR_FRAME * 2].iter().step_by(2).copied().collect();
            self.pending.drain(..NR_FRAME * 2);
            let (cl, cr) = self.frame(&l, &r);
            for k in 0..NR_FRAME {
                self.ready.push_back(cl[k]);
                self.ready.push_back(cr[k]);
            }
        }
        let wet = self.amount;
        for v in buf.iter_mut() {
            let w = self.ready.pop_front().unwrap_or(0.0);
            let d = self.dry.pop_front().unwrap_or(0.0);
            *v = w * wet + d * (1.0 - wet);
        }
    }
}

/// One input's processing chain, built from its settings.
pub struct VoiceChain {
    hpf: Option<Biquad>,
    denoise: Option<Denoiser>,
    gate: Option<Gate>,
    eq: Vec<Biquad>,
    comp: Option<Compressor>,
    de_esser: Option<DeEsser>,
}

impl VoiceChain {
    /// None when processing is off.
    pub fn new(p: &VoiceProcessing) -> Option<Self> {
        if !p.enabled {
            return None;
        }
        let mut eq = Vec::new();
        if p.eq {
            if p.eq_low_db.abs() > 0.05 {
                eq.push(Biquad::low_shelf(120.0, p.eq_low_db as f64));
            }
            if p.eq_mid_db.abs() > 0.05 {
                eq.push(Biquad::peaking((p.eq_mid_hz as f64).clamp(200.0, 8000.0), 1.0, p.eq_mid_db as f64));
            }
            if p.eq_high_db.abs() > 0.05 {
                eq.push(Biquad::high_shelf(8000.0, p.eq_high_db as f64));
            }
        }
        Some(VoiceChain {
            hpf: (p.high_pass_hz >= 20.0).then(|| Biquad::highpass((p.high_pass_hz as f64).min(400.0), std::f64::consts::FRAC_1_SQRT_2)),
            denoise: p.noise_reduction.then(|| Denoiser::new(p.noise_amount)),
            gate: p.gate.then(|| Gate::new(p.gate_threshold_db as f64)),
            eq,
            comp: p.compressor.then(|| Compressor::new(p.comp_threshold_db as f64, p.comp_ratio as f64, p.comp_makeup_db as f64)),
            de_esser: p.de_esser.then(|| DeEsser::new(p.de_esser_threshold_db as f64)),
        })
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if let Some(h) = &mut self.hpf {
            h.process(buf);
        }
        if let Some(d) = &mut self.denoise {
            d.process(buf);
        }
        if let Some(g) = &mut self.gate {
            g.process(buf);
        }
        for b in &mut self.eq {
            b.process(buf);
        }
        if let Some(c) = &mut self.comp {
            c.process(buf);
        }
        if let Some(d) = &mut self.de_esser {
            d.process(buf);
        }
    }

    /// Compressor gain reduction in the last block (dB).
    pub fn reduction_db(&self) -> f32 {
        self.comp.as_ref().map(|c| c.reduction_db as f32).unwrap_or(0.0)
    }
}

/// Keeps peaks under a ceiling: instant attack, smooth release.
pub struct Limiter {
    ceiling: f64,
    gain: f64,
    pub reduction_db: f64,
}

impl Limiter {
    pub fn new(ceiling_db: f64) -> Self {
        Limiter { ceiling: db_to_lin(ceiling_db.min(0.0)), gain: 1.0, reduction_db: 0.0 }
    }

    pub fn set_ceiling(&mut self, ceiling_db: f64) {
        self.ceiling = db_to_lin(ceiling_db.min(0.0));
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let rel = coeff(80.0);
        let mut min_g: f64 = 1.0;
        for f in buf.chunks_exact_mut(2) {
            let peak = (f[0].abs().max(f[1].abs())) as f64;
            let need = if peak * self.gain > self.ceiling { self.ceiling / peak } else { 1.0 };
            if need < self.gain {
                self.gain = need;
            } else {
                self.gain = 1.0 + rel * (self.gain - 1.0);
                self.gain = self.gain.min(need.max(self.gain));
            }
            min_g = min_g.min(self.gain);
            f[0] = (f[0] as f64 * self.gain).clamp(-self.ceiling, self.ceiling) as f32;
            f[1] = (f[1] as f64 * self.gain).clamp(-self.ceiling, self.ceiling) as f32;
        }
        self.reduction_db = -lin_to_db(min_g);
    }
}

/// A limiter that looks ahead (for files, where a little delay doesn't
/// matter): it starts turning down before a peak arrives, so peaks are held
/// under the ceiling without the click of an instant gain change.
pub struct LookaheadLimiter {
    ceiling: f64,
    ahead: usize,
    /// Frames waiting to go out, with the gain each one needs.
    delay: std::collections::VecDeque<([f32; 2], f64)>,
    /// Sliding minimum of the gain each frame needs: (frame number, gain).
    need: std::collections::VecDeque<(u64, f64)>,
    n: u64,
    gain: f64,
    attack: f64,
    release: f64,
}

impl LookaheadLimiter {
    pub fn new(ceiling_db: f64, lookahead_ms: f64) -> Self {
        let ahead = ((lookahead_ms / 1000.0 * FS) as usize).max(1);
        LookaheadLimiter {
            ceiling: db_to_lin(ceiling_db.min(0.0)),
            ahead,
            delay: std::collections::VecDeque::with_capacity(ahead + 1),
            need: std::collections::VecDeque::new(),
            n: 0,
            gain: 1.0,
            attack: (-4.0 / ahead as f64).exp(),
            release: coeff(150.0),
        }
    }

    /// Stereo frames in, the same number out (delayed by the look-ahead;
    /// call `flush` at the end).
    pub fn process(&mut self, buf: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(buf.len());
        for f in buf.chunks_exact(2) {
            let peak = f[0].abs().max(f[1].abs()) as f64;
            let need = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            while self.need.back().is_some_and(|b| b.1 >= need) {
                self.need.pop_back();
            }
            self.need.push_back((self.n, need));
            self.delay.push_back(([f[0], f[1]], need));
            self.n += 1;
            if self.delay.len() > self.ahead {
                self.emit(&mut out);
            }
        }
        out
    }

    fn emit(&mut self, out: &mut Vec<f32>) {
        let oldest = self.n - self.delay.len() as u64;
        while self.need.front().is_some_and(|f| f.0 < oldest) {
            self.need.pop_front();
        }
        let target = self.need.front().map(|f| f.1).unwrap_or(1.0);
        self.gain = if target < self.gain { target + (self.gain - target) * self.attack } else { target + (self.gain - target) * self.release };
        let (f, need) = self.delay.pop_front().unwrap();
        // Never more than this frame itself allows (the smoothing only gets
        // within a whisker of the target).
        let g = self.gain.min(need);
        let c = self.ceiling as f32;
        out.push((f[0] as f64 * g) as f32);
        out.push((f[1] as f64 * g) as f32);
        let n = out.len();
        out[n - 2] = out[n - 2].clamp(-c, c);
        out[n - 1] = out[n - 1].clamp(-c, c);
    }

    /// What's still held back.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        while !self.delay.is_empty() {
            self.emit(&mut out);
        }
        out
    }
}

/// Loudness as EBU R128 measures it: momentary (400 ms), short-term (3 s)
/// and integrated (gated, since the last reset), in LUFS.
pub struct LoudnessMeter {
    pre: Biquad,
    rlb: Biquad,
    /// Mean square of each 100 ms sub-block (K-weighted, summed over channels).
    subs: std::collections::VecDeque<f64>,
    acc: f64,
    acc_n: usize,
    /// Integrated: histogram of 400 ms block loudness, 0.1 LU bins from -70.
    hist_count: Vec<u64>,
    hist_energy: Vec<f64>,
}

const HIST_MIN: f64 = -70.0;
const HIST_BINS: usize = 800; // -70 .. +10 LUFS

fn lufs(ms: f64) -> f64 {
    if ms <= 1e-20 {
        -200.0
    } else {
        -0.691 + 10.0 * ms.log10()
    }
}

impl Default for LoudnessMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessMeter {
    pub fn new() -> Self {
        // BS.1770 K-weighting at 48 kHz.
        LoudnessMeter {
            pre: Biquad::raw([1.53512485958697, -2.69169618940638, 1.19839281085285], [1.0, -1.69065929318241, 0.73248077421585]),
            rlb: Biquad::raw([1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]),
            subs: std::collections::VecDeque::new(),
            acc: 0.0,
            acc_n: 0,
            hist_count: vec![0; HIST_BINS],
            hist_energy: vec![0.0; HIST_BINS],
        }
    }

    pub fn reset_integrated(&mut self) {
        self.hist_count.iter_mut().for_each(|c| *c = 0);
        self.hist_energy.iter_mut().for_each(|e| *e = 0.0);
    }

    pub fn push(&mut self, buf: &[f32]) {
        let sub = (FS / 10.0) as usize;
        for f in buf.chunks_exact(2) {
            let l = self.rlb.run(0, self.pre.run(0, f[0] as f64));
            let r = self.rlb.run(1, self.pre.run(1, f[1] as f64));
            self.acc += l * l + r * r;
            self.acc_n += 1;
            if self.acc_n == sub {
                self.subs.push_back(self.acc / sub as f64);
                self.acc = 0.0;
                self.acc_n = 0;
                while self.subs.len() > 30 {
                    self.subs.pop_front();
                }
                if self.subs.len() >= 4 {
                    // A new 400 ms gating block every 100 ms (75% overlap).
                    let ms = self.window(4);
                    let l = lufs(ms);
                    if l > HIST_MIN {
                        let bin = (((l - HIST_MIN) * 10.0) as usize).min(HIST_BINS - 1);
                        self.hist_count[bin] += 1;
                        self.hist_energy[bin] += ms;
                    }
                }
            }
        }
    }

    fn window(&self, n: usize) -> f64 {
        let n = n.min(self.subs.len()).max(1);
        self.subs.iter().rev().take(n).sum::<f64>() / n as f64
    }

    pub fn momentary(&self) -> f64 {
        if self.subs.len() < 4 {
            return -200.0;
        }
        lufs(self.window(4))
    }

    pub fn short_term(&self) -> f64 {
        if self.subs.is_empty() {
            return -200.0;
        }
        lufs(self.window(30))
    }

    pub fn integrated(&self) -> f64 {
        let (n, e): (u64, f64) = self.hist_count.iter().zip(&self.hist_energy).fold((0, 0.0), |a, (c, e)| (a.0 + c, a.1 + e));
        if n == 0 {
            return -200.0;
        }
        let relative = lufs(e / n as f64) - 10.0;
        let from = (((relative - HIST_MIN) * 10.0).max(0.0) as usize).min(HIST_BINS);
        let (n2, e2) = self.hist_count[from..].iter().zip(&self.hist_energy[from..]).fold((0u64, 0.0), |a, (c, e)| (a.0 + c, a.1 + e));
        if n2 == 0 {
            return -200.0;
        }
        lufs(e2 / n2 as f64)
    }
}

/// Slowly steers the stream towards a target loudness (like a broadcast
/// leveller): it never chases individual words and holds still in silence.
pub struct Leveller {
    meter: LoudnessMeter,
    pub gain_db: f64,
    applied: f64,
}

impl Default for Leveller {
    fn default() -> Self {
        Leveller { meter: LoudnessMeter::new(), gain_db: 0.0, applied: 1.0 }
    }
}

impl Leveller {
    pub fn process(&mut self, buf: &mut [f32], target_lufs: f64) {
        self.meter.push(buf);
        let st = self.meter.short_term();
        if st > -45.0 {
            let want = (target_lufs - st).clamp(-12.0, 12.0);
            // About 1 dB per second at most, slower when close.
            let step = (want - self.gain_db).clamp(-0.01, 0.01);
            self.gain_db += step;
        }
        let target = db_to_lin(self.gain_db);
        let n = buf.len() / 2;
        for (i, f) in buf.chunks_exact_mut(2).enumerate() {
            let g = self.applied + (target - self.applied) * (i + 1) as f64 / n as f64;
            f[0] = (f[0] as f64 * g) as f32;
            f[1] = (f[1] as f64 * g) as f32;
        }
        self.applied = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, secs: f64) -> Vec<f32> {
        let n = (secs * FS) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / FS).sin()) as f32;
            v.push(s);
            v.push(s);
        }
        v
    }

    fn peak(buf: &[f32]) -> f64 {
        buf.iter().fold(0.0f64, |m, s| m.max(s.abs() as f64))
    }

    #[test]
    fn loudness_of_a_reference_tone() {
        // A 1 kHz sine peaking at -20 dBFS on both channels reads -20 LUFS
        // (K-weighting is +0.69 dB at 1 kHz, cancelling the -0.691 offset).
        let mut m = LoudnessMeter::new();
        let tone = sine(1000.0, db_to_lin(-20.0), 5.0);
        for b in tone.chunks(960) {
            m.push(b);
        }
        assert!((m.momentary() - -20.0).abs() < 0.3, "{}", m.momentary());
        assert!((m.short_term() - -20.0).abs() < 0.3);
        assert!((m.integrated() - -20.0).abs() < 0.3, "{}", m.integrated());
    }

    #[test]
    fn lookahead_limiter_holds_the_ceiling_smoothly() {
        let mut lim = LookaheadLimiter::new(-1.0, 5.0);
        // Quiet tone with a loud burst in the middle.
        let mut x = sine(440.0, 0.2, 1.0);
        let burst = sine(440.0, 1.5, 0.2);
        let at = 48_000; // frames in
        x[at..at + burst.len()].copy_from_slice(&burst);
        let mut y = Vec::new();
        for b in x.chunks(1000) {
            y.extend(lim.process(b));
        }
        y.extend(lim.flush());
        assert_eq!(y.len(), x.len());
        let ceiling = db_to_lin(-1.0);
        assert!(peak(&y) <= ceiling + 1e-6, "{}", peak(&y));
        // Through the burst the gain is steady: the waveform keeps its shape
        // (no flattened peaks). Output frame k is input frame k, later.
        let d = 0;
        let (from, to) = (at + 1920, at + burst.len() - 1920);
        let ratios: Vec<f32> = (from..to).filter(|&i| x[i].abs() > 0.5).map(|i| y[i + d] / x[i]).collect();
        let (lo, hi) = ratios.iter().fold((f32::MAX, f32::MIN), |a, r| (a.0.min(*r), a.1.max(*r)));
        assert!(hi - lo < 0.01 && (lo - 0.594).abs() < 0.01, "{lo} {hi}");
        // The quiet part before the burst is untouched (allowing for the delay).
        assert!((y[2000 + d] - x[2000]).abs() < 1e-6);
    }

    /// Pink-ish noise (a fan or air conditioner), stereo.
    fn fan_noise(frames: usize, amp: f32) -> Vec<f32> {
        let mut seed = 12345u32;
        let (mut b0, mut b1, mut b2) = (0f32, 0f32, 0f32);
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let w = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            let v = (b0 + b1 + b2 + w * 0.1848) * amp;
            out.push(v);
            out.push(v);
        }
        out
    }

    #[test]
    fn noise_reduction_turns_down_a_fan() {
        let noise = fan_noise(48_000 * 2, 0.1);
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        for (amount, want_below_db) in [(1.0f32, -6.0f32), (0.0, 0.5)] {
            let mut d = Denoiser::new(amount);
            let mut out = Vec::new();
            for block in noise.chunks(480 * 2) {
                let mut b = block.to_vec();
                d.process(&mut b);
                assert_eq!(b.len(), block.len());
                out.extend(b);
            }
            // After the first second (it settles), compare levels.
            let before = rms(&noise[48_000 * 2..]);
            let after = rms(&out[48_000 * 2..]);
            let db = 20.0 * (after / before).log10();
            assert!(db < want_below_db, "amount {amount}: {db:.1} dB");
            if amount == 0.0 {
                // Untouched, 10 ms late.
                assert!((out[60_000 * 2] - noise[(60_000 - 480) * 2]).abs() < 1e-6);
            }
        }
        // Blocks that aren't 10 ms work too (and keep their length).
        let mut d = Denoiser::new(1.0);
        let mut b = noise[..700 * 2].to_vec();
        d.process(&mut b);
        assert_eq!(b.len(), 1400);
    }

    #[test]
    fn high_pass_removes_rumble() {
        let mut f = Biquad::highpass(80.0, std::f64::consts::FRAC_1_SQRT_2);
        let mut low = sine(30.0, 0.5, 1.0);
        f.process(&mut low);
        assert!(peak(&low[48000..]) < 0.5 * 0.2);
        let mut f = Biquad::highpass(80.0, std::f64::consts::FRAC_1_SQRT_2);
        let mut voice = sine(1000.0, 0.5, 1.0);
        f.process(&mut voice);
        assert!(peak(&voice[48000..]) > 0.48);
    }

    #[test]
    fn limiter_holds_the_ceiling() {
        let mut l = Limiter::new(-1.0);
        let mut loud = sine(200.0, 1.5, 1.0);
        for b in loud.chunks_mut(960) {
            l.process(b);
        }
        assert!(peak(&loud) <= db_to_lin(-1.0) + 1e-6);
        let mut quiet = sine(200.0, 0.3, 0.5);
        let mut l = Limiter::new(-1.0);
        let before = quiet.clone();
        l.process(&mut quiet);
        assert_eq!(quiet, before, "quiet audio passes untouched");
    }

    #[test]
    fn compressor_and_gate() {
        let p = VoiceProcessing { enabled: true, high_pass_hz: 0.0, compressor: true, comp_threshold_db: -20.0, comp_ratio: 4.0, comp_makeup_db: 0.0, ..Default::default() };
        let mut c = VoiceChain::new(&p).unwrap();
        let mut loud = sine(500.0, db_to_lin(-4.0), 1.0);
        for b in loud.chunks_mut(960) {
            c.process(b);
        }
        // 16 dB over at 4:1 → about 12 dB of reduction.
        let out = lin_to_db(peak(&loud[48000..]));
        assert!(out < -13.0 && out > -19.0, "{out}");

        let p = VoiceProcessing { enabled: true, high_pass_hz: 0.0, compressor: false, gate: true, gate_threshold_db: -40.0, ..Default::default() };
        let mut g = VoiceChain::new(&p).unwrap();
        let mut hiss = sine(3000.0, db_to_lin(-60.0), 1.0);
        for b in hiss.chunks_mut(960) {
            g.process(b);
        }
        assert!(lin_to_db(peak(&hiss[40000..])) < -95.0);
        assert!(VoiceChain::new(&VoiceProcessing::default()).is_none(), "off by default");
    }

    #[test]
    fn leveller_moves_towards_target() {
        let mut lv = Leveller::default();
        let tone = sine(1000.0, db_to_lin(-30.0), 30.0); // -30 LUFS
        let mut out_meter = LoudnessMeter::new();
        for b in tone.chunks(960) {
            let mut b = b.to_vec();
            lv.process(&mut b, -14.0);
            out_meter.push(&b);
        }
        assert!(lv.gain_db > 11.0, "{}", lv.gain_db); // capped at +12 dB
        assert!(out_meter.short_term() > -19.0);
    }
}
