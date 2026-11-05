//! A few long-lived worker threads for splitting a big picture's rows
//! between cores. (Starting threads for every frame of every source cost
//! hundreds of thread starts a second.)

use crossbeam_channel::{unbounded, Sender};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Pictures smaller than this (output pixels) are done on the caller's
/// thread: splitting them costs more than it saves.
pub const SPLIT_ABOVE: usize = 1_000_000;

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
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(1).min(3);
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
        assert_eq!(parts_for(640 * 360), 1);
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
}
