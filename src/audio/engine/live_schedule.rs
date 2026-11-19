//! Prepared ownership and dependency information for the live audio workers.
//!
//! Tracks run in sidechain order. A bus fed exclusively by track outputs can
//! run on the last input's worker, before unrelated tracks finish. The rest
//! of the buses run in topological order, with independent inserts sharing a
//! wave. Routing always adds contributions in the same order as serial DSP.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{AudioEngine, BusRuntime, EngineError, EngineFade, MixSpan};
use crate::audio::ProcessContext;
use crate::model::{Device, DeviceKind, Session};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Affinity {
    Native,
    Clap,
    Callback,
}

impl Affinity {
    fn chain<'a>(devices: impl Iterator<Item = &'a Device>) -> Self {
        devices.map(Self::device).max().unwrap_or(Self::Native)
    }

    fn device(device: &Device) -> Self {
        match device.kind {
            DeviceKind::Vst3 => Self::Callback,
            DeviceKind::Clap => {
                if device
                    .vst3
                    .as_ref()
                    .is_some_and(|plugin| plugin.render_thread)
                {
                    Self::Callback
                } else {
                    Self::Clap
                }
            }
            DeviceKind::Rack => device.rack.as_ref().map_or(Self::Native, |rack| {
                Self::chain(rack.branches.iter().flatten())
            }),
            _ => Self::Native,
        }
    }
}

struct TrackJob {
    track: usize,
    affinity: Affinity,
    early_bus: Option<usize>,
}

struct EarlyBus {
    // In the same order as the serial track mix.
    inputs: Vec<usize>,
    pending: AtomicUsize,
}

struct BusWave {
    // A contiguous range of the serial bus order, including already-run buses.
    order: Range<usize>,
    // Empty unless this wave has enough work to send to the pool.
    workers: Vec<usize>,
    pinned: Vec<usize>,
}

/// Rebuilt off the callback whenever the graph structure changes.
#[derive(Default)]
pub(super) struct LiveSchedule {
    track_waves: Vec<Vec<TrackJob>>,
    early_buses: Vec<Option<EarlyBus>>,
    early_track_bus: Vec<Option<usize>>,
    bus_waves: Vec<BusWave>,
    widest: usize,
}

impl LiveSchedule {
    pub(super) fn new(engine: &AudioEngine, session: &Session) -> Self {
        let descriptions = std::iter::once(&session.master).chain(&session.buses);
        let bus_affinity: Vec<_> = descriptions
            .clone()
            .map(|bus| Affinity::chain(bus.inserts.iter()))
            .collect();
        let track_affinity: Vec<_> = session
            .tracks
            .iter()
            .map(|track| Affinity::chain(std::iter::once(&track.instrument).chain(&track.inserts)))
            .collect();

        let mut exclusive = vec![true; engine.buses.len()];
        for bus in &engine.buses {
            for route in &bus.routes {
                exclusive[route.target] = false;
            }
        }
        let mut inputs = vec![Vec::new(); engine.buses.len()];
        for &index in &engine.track_order {
            let track = &engine.tracks[index];
            inputs[track.output.target].push(index);
            // If another bus needs this track's routing state, leave the
            // whole track to the main mixing pass.
            if !track.sends.is_empty() {
                exclusive[track.output.target] = false;
                for send in &track.sends {
                    exclusive[send.target] = false;
                }
            }
        }

        let mut early_track_bus = vec![None; engine.tracks.len()];
        let early_buses: Vec<_> = descriptions
            .enumerate()
            .map(|(index, bus)| {
                if bus_affinity[index] != Affinity::Clap
                    || !exclusive[index]
                    || inputs[index].is_empty()
                    || bus
                        .inserts
                        .iter()
                        .any(|d| d.sidechain.is_some() || d.streams_song_time())
                {
                    return None;
                }
                for &track in &inputs[index] {
                    early_track_bus[track] = Some(index);
                }
                Some(EarlyBus {
                    inputs: std::mem::take(&mut inputs[index]),
                    pending: AtomicUsize::new(0),
                })
            })
            .collect();

        let track_waves: Vec<Vec<_>> = engine
            .track_waves
            .iter()
            .map(|wave| {
                let mut jobs: Vec<_> = wave
                    .iter()
                    .map(|&track| TrackJob {
                        track,
                        affinity: track_affinity[track],
                        early_bus: early_track_bus[track],
                    })
                    .collect();
                // Start inputs to early effects and expensive standalone CLAP
                // tracks before native jobs. Stable ordering within each group.
                jobs.sort_by_key(|job| (job.early_bus.is_none(), job.affinity == Affinity::Native));
                jobs
            })
            .collect();

        let mut bus_waves = Vec::new();
        let mut last_feeder = vec![0; engine.buses.len()];
        let mut start = 0;
        let mut finish_wave = |order: Range<usize>| {
            let jobs = || {
                engine.bus_order[order.clone()]
                    .iter()
                    .copied()
                    .filter(|&i| early_buses[i].is_none() && !engine.buses[i].inserts.is_empty())
            };
            let mut workers = Vec::new();
            let mut pinned = Vec::new();
            if jobs().count() > 1 && jobs().any(|i| bus_affinity[i] == Affinity::Clap) {
                for i in jobs() {
                    if bus_affinity[i] == Affinity::Callback {
                        pinned.push(i);
                    } else {
                        workers.push(i);
                    }
                }
            }
            bus_waves.push(BusWave {
                order,
                workers,
                pinned,
            });
        };
        for (pos, &bus) in engine.bus_order.iter().enumerate() {
            // A route from within this batch must be mixed before its target's
            // inserts run. Keep each batch contiguous to preserve rounding.
            if last_feeder[bus] > start {
                finish_wave(start..pos);
                start = pos;
            }
            for route in &engine.buses[bus].routes {
                last_feeder[route.target] = pos + 1;
            }
        }
        finish_wave(start..engine.bus_order.len());
        let widest = track_waves
            .iter()
            .map(Vec::len)
            .chain(
                bus_waves
                    .iter()
                    .map(|wave| wave.workers.len() + wave.pinned.len()),
            )
            .max()
            .unwrap_or(0);
        Self {
            track_waves,
            early_buses,
            early_track_bus,
            bus_waves,
            widest,
        }
    }

    pub(super) fn widest(&self) -> usize {
        self.widest
    }

    pub(super) fn early_track(&self, index: usize) -> bool {
        self.early_track_bus[index].is_some()
    }

    pub(super) fn early_bus(&self, index: usize) -> bool {
        self.early_buses[index].is_some()
    }

    pub(super) fn has_bus_workers(&self) -> bool {
        self.bus_waves.iter().any(|wave| !wave.workers.is_empty())
    }
}

/// The wave's exclusive items stay in their original slots. Shared access to
/// the pointer conveys no access to the items until a job has claimed a slot.
struct Slots<T>(*mut T);

// SAFETY: with() requires that the caller already owns the index. A slot can
// move between threads, so its value must be Send.
unsafe impl<T: Send> Sync for Slots<T> {}

impl<T> Slots<T> {
    /// # Safety
    /// The slot is in bounds and exclusively owned for the call. The
    /// underlying slice outlives all jobs and is inaccessible until they join.
    unsafe fn with<R>(&self, index: usize, f: impl FnOnce(&mut T) -> R) -> R {
        f(unsafe { &mut *self.0.add(index) })
    }
}

impl AudioEngine {
    pub(super) fn process_tracks_live(
        &mut self,
        context: ProcessContext,
        span: MixSpan,
        fade: Option<EngineFade>,
        early: bool,
    ) -> Result<(), EngineError> {
        if early {
            for bus in self.live_schedule.early_buses.iter_mut().flatten() {
                *bus.pending.get_mut() = bus.inputs.len();
            }
        }
        for w in 0..self.track_waves.len() {
            self.feed_wave_detectors(w, context, span.start, span.end);
            let jobs = &self.live_schedule.track_waves[w];
            let tracks = Slots(self.tracks.as_mut_ptr());
            let buses = Slots(self.buses.as_mut_ptr());
            let pinned = |job: &TrackJob| {
                job.affinity == Affinity::Callback
                    || (self.plugins_on_render_thread && job.affinity == Affinity::Clap)
            };
            #[cfg(test)]
            let caller = std::thread::current().id();
            let run = |job: &TrackJob| {
                // SAFETY: each track appears once. Pinning assigns it to one
                // executor before either executor borrows it.
                unsafe {
                    tracks.with(job.track, |track| {
                        #[cfg(test)]
                        {
                            track.pooled |= caller != std::thread::current().id();
                        }
                        track.failed = track.process_chain(context, span.start, span.end).err();
                    })
                };
                if early && let Some(index) = job.early_bus {
                    let bus = self.live_schedule.early_buses[index].as_ref().unwrap();
                    // Publish the track's writes and end its mutable borrow
                    // before handing ownership of its routing to the bus.
                    if bus.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                        // SAFETY: this is the last input to finish. The atomic
                        // releases publish all of its inputs' state to this
                        // job. Those tracks and this bus belong to this group
                        // alone until the wave joins.
                        unsafe {
                            buses.with(index, |runtime| {
                                if bus
                                    .inputs
                                    .iter()
                                    .any(|&i| tracks.with(i, |t| t.failed.is_some()))
                                {
                                    return;
                                }
                                for &i in &bus.inputs {
                                    tracks.with(i, |track| {
                                        track.mix_output(
                                            &mut runtime.scratch,
                                            span,
                                            context,
                                            fade.filter(|mask| mask.track(i)),
                                        );
                                    });
                                }
                                #[cfg(test)]
                                {
                                    runtime.pooled |= caller != std::thread::current().id();
                                }
                                runtime.failed = runtime.process_chain(context, span).err();
                            });
                        }
                    }
                }
            };
            if let [job] = jobs.as_slice() {
                run(job);
            } else {
                self.pool.as_mut().expect("live track pool").run(
                    jobs.len(),
                    &|k| {
                        if !pinned(&jobs[k]) {
                            run(&jobs[k]);
                        }
                    },
                    || {
                        for job in jobs.iter().filter(|job| pinned(job)) {
                            run(job);
                        }
                    },
                );
            }
            // Read errors in the original track order, independently of
            // dispatch order and worker completion.
            let mut failure = None;
            for &track in &self.track_waves[w] {
                if let Some(error) = self.tracks[track].failed.take() {
                    failure.get_or_insert(error);
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn process_buses_live(
        &mut self,
        context: ProcessContext,
        span: MixSpan,
        fade: Option<EngineFade>,
    ) -> Result<(), EngineError> {
        for w in 0..self.live_schedule.bus_waves.len() {
            let wave = &self.live_schedule.bus_waves[w];
            let order = wave.order.clone();
            if wave.workers.is_empty() {
                for pos in order {
                    self.process_and_route_bus(self.bus_order[pos], context, span, fade, true)?;
                }
                continue;
            }
            for &index in wave.workers.iter().chain(&wave.pinned) {
                self.buses[index].feed_detectors(&self.tracks, context, span);
            }
            let buses = Slots::<BusRuntime>(self.buses.as_mut_ptr());
            #[cfg(test)]
            let caller = std::thread::current().id();
            let run = |index| {
                // SAFETY: the wave contains distinct ready buses partitioned
                // between the pool and the audio thread. They touch only
                // their own insert chains and scratch.
                unsafe {
                    buses.with(index, |bus| {
                        #[cfg(test)]
                        {
                            bus.pooled |= caller != std::thread::current().id();
                        }
                        bus.failed = bus.process_chain(context, span).err();
                    })
                };
            };
            self.pool.as_mut().expect("live bus pool").run(
                wave.workers.len(),
                &|i| run(wave.workers[i]),
                || wave.pinned.iter().copied().for_each(run),
            );
            for pos in order {
                let index = self.bus_order[pos];
                if let Some(error) = self.buses[index].failed.take() {
                    return Err(error);
                }
                self.route_bus(index, context, span, fade);
            }
        }
        Ok(())
    }
}
