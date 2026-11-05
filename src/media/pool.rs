//! A few long-lived worker threads for splitting a big picture's rows
//! between cores. (Starting threads for every frame of every source cost
//! hundreds of thread starts a second.)

use crossbeam_channel::{unbounded, Sender};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Pictures smaller than this (output pixels) are done on the caller's
/// thread: splitting them costs more than it saves.
pub const SPLIT_ABOVE: usize = 640 * 360;

struct Job {
    /// Borrowed from the caller of `run`, which waits until every part has
    /// finished before returning.
    f: &'static (dyn Fn(usize) + Sync),
    part: usize,
    done: Arc<Done>,
}

struct Done {
    left: Mutex<(usize, bool)>,
    cv: Condvar,
}

impl Done {
    fn finish(&self, panicked: bool) {
        let mut g = self.left.lock().unwrap_or_else(|e| e.into_inner());
        g.0 -= 1;
        g.1 |= panicked;
        if g.0 == 0 {
            self.cv.notify_all();
        }
    }
}

fn workers() -> Option<&'static Sender<Job>> {
    static POOL: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(1).clamp(1, 7);
        if n == 0 {
            return None;
        }
        let (tx, rx) = unbounded::<Job>();
        let mut started = 0;
        for i in 0..n {
            let rx = rx.clone();
            let ok = std::thread::Builder::new()
                .name(format!("picture worker {}", i + 1))
                .spawn(move || {
                    for job in rx {
                        let r = catch_unwind(AssertUnwindSafe(|| (job.f)(job.part)));
                        job.done.finish(r.is_err());
                    }
                })
                .is_ok();
            started += ok as usize;
        }
        (started > 0).then_some(tx)
    })
    .as_ref()
}

/// How many parts to split a picture of `pixels` output pixels into.
pub fn parts_for(pixels: usize) -> usize {
    if pixels < SPLIT_ABOVE {
        return 1;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
}

/// Run `f(0)`..`f(parts - 1)`, spread over the workers and this thread,
/// and return once all of them have finished.
pub fn run(parts: usize, f: &(dyn Fn(usize) + Sync)) {
    let pool = workers().filter(|_| parts > 1);
    let Some(pool) = pool else {
        for i in 0..parts {
            f(i);
        }
        return;
    };
    let done = Arc::new(Done { left: Mutex::new((parts - 1, false)), cv: Condvar::new() });
    // SAFETY: a job only calls `f` before it calls `done.finish`, and this
    // function doesn't return (or unwind) before every job has done so.
    let f_static: &'static (dyn Fn(usize) + Sync) = unsafe { std::mem::transmute::<&(dyn Fn(usize) + Sync), &'static (dyn Fn(usize) + Sync)>(f) };
    let mut sent = 0;
    for part in 1..parts {
        if pool.send(Job { f: f_static, part, done: done.clone() }).is_err() {
            break;
        }
        sent += 1;
    }
    // Anything that couldn't be handed out is done here.
    let mine = catch_unwind(AssertUnwindSafe(|| {
        f(0);
        for part in sent + 1..parts {
            f(part);
        }
    }));
    let mut g = done.left.lock().unwrap_or_else(|e| e.into_inner());
    g.0 -= parts - 1 - sent;
    while g.0 > 0 {
        g = done.cv.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    let panicked = g.1;
    drop(g);
    if let Err(e) = mine {
        resume_unwind(e);
    }
    if panicked {
        panic!("a picture worker failed");
    }
}

/// Spare frame buffers, for GPU readbacks (a few frames' worth), and when
/// each came back.
static SPARE: Mutex<Vec<(Instant, Vec<u8>)>> = Mutex::new(Vec::new());
const SPARE_MAX: usize = 12;
/// A spare not taken again within this long is freed (recording stopped,
/// or the size changed).
const SPARE_KEEP: Duration = Duration::from_secs(2);

fn spare() -> std::sync::MutexGuard<'static, Vec<(Instant, Vec<u8>)>> {
    let mut g = SPARE.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(at, _)| at.elapsed() < SPARE_KEEP);
    g
}

/// An empty buffer that can hold `len` bytes without growing.
pub fn take(len: usize) -> Vec<u8> {
    let mut spare = spare();
    // A close fit only, so a small picture doesn't keep a big buffer.
    match spare.iter().position(|(_, v)| v.capacity() >= len && v.capacity() - len <= len / 8) {
        Some(i) => spare.swap_remove(i).1,
        None => Vec::with_capacity(len),
    }
}

/// A buffer finished with, for `take` to hand out again.
pub fn give_back(mut v: Vec<u8>) {
    if v.capacity() == 0 {
        return;
    }
    v.clear();
    let mut spare = spare();
    if spare.len() >= SPARE_MAX {
        // Full: the oldest goes.
        spare.remove(0);
    }
    spare.push((Instant::now(), v));
}

/// Free spares nobody has taken for a while.
pub fn trim() {
    drop(spare());
}

/// Free every spare (the engine stopped).
pub fn free_spares() {
    spare().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_part_runs_once() {
        for parts in [1, 2, 4, 9] {
            let hits: Vec<AtomicUsize> = (0..parts).map(|_| AtomicUsize::new(0)).collect();
            run(parts, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        }
        assert_eq!(parts_for(320 * 180), 1);
    }

    #[test]
    fn many_callers_at_once() {
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(|| {
                    for _ in 0..50 {
                        let sum = AtomicUsize::new(0);
                        run(4, &|i| {
                            sum.fetch_add(i + 1, Ordering::Relaxed);
                        });
                        assert_eq!(sum.load(Ordering::Relaxed), 10);
                    }
                });
            }
        });
    }

    #[test]
    fn buffers_come_back() {
        // (A size no other test reads back.)
        let len = 1_234_567;
        let a = take(len);
        assert!(a.capacity() >= len);
        let ptr = a.as_ptr();
        give_back(a);
        let b = take(len);
        assert_eq!(b.as_ptr(), ptr);
        assert!(b.is_empty());
        // Nothing much bigger than asked for.
        give_back(b);
        assert!(take(1000).capacity() < 2000);
        let c = take(len);
        assert_eq!(c.as_ptr(), ptr);
    }
}
