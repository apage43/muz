//! Persistent workers that share a live block's ready jobs.
//!
//! The audio thread publishes a job of `count` items, wakes the workers and
//! claims items itself; whoever claims an item runs it. A worker that is slow
//! to wake simply finds nothing left, so the audio thread never waits for a
//! wake-up, only for items already running. Publishing allocates nothing and
//! takes no lock: one atomic store and a futex wake per parked worker.
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{JoinHandle, Thread};

/// How long an idle worker polls for the next job before it parks.
const SPIN: std::time::Duration = std::time::Duration::from_micros(20);

type Job = *const (dyn Fn(usize) + Sync);

struct Shared {
    /// The job's generation (high 32 bits), item count (16) and next unclaimed
    /// item (16). Claiming an item is a compare-exchange on this word, so no
    /// item of a finished job can be claimed late.
    ticket: AtomicU64,
    /// The current job; written only while no item of the previous one is
    /// claimed or running, then published by `ticket`.
    job: UnsafeCell<Option<Job>>,
    /// Items of the current job finished.
    done: AtomicUsize,
    /// An item panicked.
    panicked: AtomicBool,
    shutdown: AtomicBool,
}
// SAFETY: `job` is written by the owner only between jobs (see `run`) and read
// by a thread only after it claims an item, which happens after the write.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

fn split(ticket: u64) -> (u64, usize, usize) {
    (
        ticket >> 32,
        ((ticket >> 16) & 0xffff) as usize,
        (ticket & 0xffff) as usize,
    )
}

impl Shared {
    /// Claim and run items of the current job until none are left. Returns
    /// whether any item was run.
    fn help(&self) -> bool {
        let mut ran = false;
        loop {
            let ticket = self.ticket.load(Ordering::Acquire);
            let (_, count, next) = split(ticket);
            if next >= count {
                return ran;
            }
            if self
                .ticket
                .compare_exchange_weak(ticket, ticket + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            // SAFETY: an item is claimed, so `run` published this job and
            // will not return, or replace it, before the item is done.
            let job = unsafe { (*self.job.get()).expect("a published job") };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*job)(next) }));
            if result.is_err() {
                self.panicked.store(true, Ordering::Relaxed);
            }
            self.done.fetch_add(1, Ordering::Release);
            ran = true;
        }
    }
}

pub(super) struct TrackPool {
    shared: Arc<Shared>,
    workers: Vec<(Thread, JoinHandle<()>)>,
    generation: u64,
}

impl TrackPool {
    /// `workers` threads beside the caller's.
    pub(super) fn new(workers: usize) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            ticket: AtomicU64::new(0),
            job: UnsafeCell::new(None),
            done: AtomicUsize::new(0),
            panicked: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });
        // Dropping a partially constructed pool also shuts down and joins
        // the workers already started if a later spawn fails.
        let mut pool = Self {
            shared,
            workers: Vec::with_capacity(workers),
            generation: 0,
        };
        for k in 0..workers {
            let shared = Arc::clone(&pool.shared);
            let handle = std::thread::Builder::new()
                .name(format!("muz-track-{k}"))
                .spawn(move || worker(&shared))?;
            pool.workers.push((handle.thread().clone(), handle));
        }
        Ok(pool)
    }

    pub(super) fn workers(&self) -> usize {
        self.workers.len()
    }

    /// Publish `job(0..count)` to the workers. The caller runs `first` and
    /// then helps with unclaimed items. All items finish before this returns
    /// or unwinds.
    pub(super) fn run(&mut self, count: usize, job: &(dyn Fn(usize) + Sync), first: impl FnOnce()) {
        assert!(count <= 0xffff, "a job has at most 65535 items");
        let shared = &*self.shared;
        // SAFETY: every item of the previous job is done (the wait below), so
        // no thread reads `job` until the ticket below publishes it. The
        // pointer outlives its use: this function returns only once every
        // item is done, and a late thread cannot claim one.
        unsafe {
            *shared.job.get() =
                Some(std::mem::transmute::<*const (dyn Fn(usize) + Sync + '_), Job>(job));
        }
        shared.done.store(0, Ordering::Relaxed);
        self.generation = (self.generation + 1) & 0xffff_ffff;
        shared.ticket.store(
            self.generation << 32 | (count as u64) << 16,
            Ordering::Release,
        );
        for (thread, _) in &self.workers {
            thread.unpark();
        }
        // A panic on the caller must not let the job or its captured
        // buffers go out of scope while any worker can still use them.
        let caller = std::panic::catch_unwind(std::panic::AssertUnwindSafe(first));
        shared.help();
        while shared.done.load(Ordering::Acquire) < count {
            std::hint::spin_loop();
        }
        let worker_panicked = shared.panicked.swap(false, Ordering::Relaxed);
        if let Err(payload) = caller {
            std::panic::resume_unwind(payload);
        }
        if worker_panicked {
            panic!("a live audio job panicked");
        }
    }
}

fn worker(shared: &Shared) {
    let mut seen = 0;
    loop {
        let idle = std::time::Instant::now();
        loop {
            if shared.shutdown.load(Ordering::Acquire) {
                return;
            }
            let (generation, count, next) = split(shared.ticket.load(Ordering::Acquire));
            if generation != seen && next < count {
                seen = generation;
                break;
            }
            if idle.elapsed() < SPIN {
                std::hint::spin_loop();
            } else {
                std::thread::park();
            }
        }
        shared.help();
    }
}

impl Drop for TrackPool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        for (thread, _) in &self.workers {
            thread.unpark();
        }
        for (_, handle) in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn caller_unwind_joins_live_work() {
        let mut pool = TrackPool::new(1).unwrap();
        let entered = Barrier::new(2);
        let finished = AtomicBool::new(false);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.run(
                1,
                &|_| {
                    entered.wait();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    finished.store(true, Ordering::Release);
                },
                || {
                    entered.wait();
                    panic!("caller");
                },
            );
        }));
        assert!(result.is_err());
        assert!(finished.load(Ordering::Acquire));
        pool.run(1, &|_| {}, || {});
    }
}
