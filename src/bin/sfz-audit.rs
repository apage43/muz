//! Reproducible source/dependency audit; parsing is not playback certification.
use anyhow::{Context, Result, ensure};
use muz::{
    audio::{
        AudioConfig,
        device::{
            DeviceEvent, DeviceEventKind, DeviceProcessor, ProcessContext, SfzStatistics,
            create_processor,
        },
        transport::TransportSnapshot,
    },
    model::{Device, DeviceKind, Id, SfzConfig},
    sfz::{Options, Program},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead},
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    path: PathBuf,
    #[serde(default)]
    defines: BTreeMap<String, String>,
    #[serde(default)]
    source_overlays: Vec<muz::sfz::SourceOverlay>,
    #[serde(default)]
    profile_workload: ProfileWorkload,
    #[serde(default)]
    profile_switch: Option<u8>,
}
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileWorkload {
    #[default]
    FourHeldNotes,
    DrumPattern,
}
fn audit(request: &Request, prepare: bool, exercise: bool) -> Result<serde_json::Value> {
    let program = muz::sfz::load(
        &request.path,
        &Options {
            defines: request.defines.clone(),
            source_overlays: request.source_overlays.clone(),
            ..Options::default()
        },
    )?;
    let mut opcodes = BTreeMap::<String, BTreeSet<String>>::new();
    for region in &program.regions {
        for (name, value) in &region.opcodes {
            opcodes
                .entry(name.clone())
                .or_default()
                .insert(value.clone());
        }
    }
    let unsupported = muz::audio::device::unsupported_behaviors(&program);
    let mut report = serde_json::json!({
        "path":request.path,"regions":program.regions.len(),
        "normalized_program_bytes":serde_json::to_vec(&program)?.len(),
        "dependencies":program.dependencies,"opcodes":opcodes,
        "diagnostics":program.diagnostics,"unsupported_behaviors":unsupported,
        "controls":program.controls,"labels":program.labels,"curves":program.curves,
        "stage":"normalized_source",
    });
    if prepare {
        let mut compatibility = Vec::new();
        let samples: BTreeSet<_> = program
            .regions
            .iter()
            .filter_map(|region| region.sample.as_ref())
            .collect();
        for sample in samples {
            match muz::audio_file::info(sample) {
                Ok(info) if !info.compatibility.is_empty() => compatibility
                    .push(serde_json::json!({"path":sample,"interpretations":info.compatibility})),
                Ok(_) => {}
                Err(error) => compatibility
                    .push(serde_json::json!({"path":sample,"metadata_error":format!("{error:#}")})),
            }
        }
        report["sample_compatibility"] = serde_json::json!(compatibility);
        let device = device(request, &program);
        // Preparation loads every required recording, but does not certify audible parity.
        let result = create_processor(
            &device,
            AudioConfig {
                sample_rate: 48000.0,
                max_frames: 256,
                offline: true,
            },
        );
        report["prepared"] = serde_json::json!(result.is_ok());
        match result {
            Ok(mut processor) => {
                if let Some(stats) = processor.sfz_statistics() {
                    report["decoded_frames"] = serde_json::json!(stats.decoded_frames);
                    report["compiled_dsp_programs"] =
                        serde_json::json!(stats.compiled_dsp_programs);
                    report["compiled_dsp_bytes_shallow"] =
                        serde_json::json!(stats.compiled_dsp_bytes_shallow);
                    report["sample_budget_frames"] = serde_json::json!(stats.max_sample_frames);
                    report["max_voices"] = serde_json::json!(stats.max_voices);
                }
                if exercise {
                    match exercise_program(processor.as_mut(), &program) {
                        Ok(result) => report["exercise"] = result,
                        Err(error) => {
                            report["exercise_error"] = serde_json::json!(format!("{error:#}"))
                        }
                    }
                }
            }
            Err(error) => {
                report["preparation_error"] = serde_json::json!(error.to_string());
            }
        }
    }
    Ok(report)
}
fn device(request: &Request, program: &Program) -> Device {
    Device {
        id: Id::new("sfz-audit"),
        kind: DeviceKind::Sfz,
        params: BTreeMap::new(),
        sfz: Some(SfzConfig {
            program: program.clone(),
            path: request.path.display().to_string(),
            defines: request.defines.clone(),
            source_overlays: request.source_overlays.clone(),
            max_voices: 256,
            max_sample_frames: muz::model::MAX_SFZ_SAMPLE_FRAMES,
            seed: 1,
            embed_assets: false,
        }),
        sample: None,
        patch: None,
        rack: None,
        vst3: None,
        sidechain: None,
        generation: 0,
        asset_versions: vec![],
    }
}
/// A region-derived predicate fixture. A tested fixture does not claim each
/// random region fired, or that every modulation value matches another player.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Probe {
    key: u8,
    velocity: u8,
    switch: Option<u8>,
    previous: Option<u8>,
    legato: bool,
    cc: Vec<(u8, u8)>,
}
fn boundaries(a: u8, b: u8) -> BTreeSet<u8> {
    BTreeSet::from([a, a + (b - a) / 2, b])
}
fn physical_suffix(name: &str) -> Option<u8> {
    if !name.contains("cc") {
        return None;
    }
    let start = name.rfind(|c: char| !c.is_ascii_digit())? + 1;
    name[start..].parse::<u8>().ok().filter(|cc| *cc < 128)
}
fn probe_plan(
    program: &Program,
) -> Result<(Vec<Probe>, BTreeMap<u8, BTreeSet<u8>>, u64, Vec<Vec<usize>>)> {
    let mut probes = BTreeMap::<Probe, Vec<usize>>::new();
    let mut controls = BTreeMap::<u8, BTreeSet<u8>>::new();
    let mut delay = 0f64;
    for &cc in program.controls.keys().chain(program.labels.keys()) {
        if cc < 128 {
            controls
                .entry(cc as u8)
                .or_default()
                .extend([0, 63, 64, 127]);
        }
    }
    for (region_index, region) in program.regions.iter().enumerate() {
        let key = region.key("key", 60).clamp(0, 127) as u8;
        let (lo, hi) = if region.opcodes.contains_key("key") {
            (key, key)
        } else {
            (
                region.key("lokey", 0).clamp(0, 127) as u8,
                region.key("hikey", 127).clamp(0, 127) as u8,
            )
        };
        let vlo = region.integer("lovel", 0).clamp(0, 127) as u8;
        let vhi = region.integer("hivel", 127).clamp(0, 127) as u8;
        ensure!(lo <= hi && vlo <= vhi, "inverted probe range");
        let mut cc = Vec::new();
        for number in 0..128u8 {
            let low = format!("locc{number}");
            let high = format!("hicc{number}");
            if region.opcodes.contains_key(&low) || region.opcodes.contains_key(&high) {
                let a = region.number(&low, 0.).ceil().clamp(0., 127.) as u8;
                let b = region.number(&high, 127.).floor().clamp(0., 127.) as u8;
                if a <= b {
                    cc.push((number, a + (b - a) / 2));
                }
                let values = controls.entry(number).or_default();
                values.extend([
                    a,
                    b,
                    a.saturating_sub(1),
                    b.saturating_add(1).min(127),
                    a + (b.saturating_sub(a)) / 2,
                ]);
            }
        }
        for name in region.opcodes.keys() {
            if let Some(number) = physical_suffix(name) {
                controls.entry(number).or_default().extend([0, 63, 64, 127]);
            }
        }
        let switch = region
            .opcodes
            .contains_key("sw_last")
            .then(|| region.key("sw_last", 0).clamp(0, 127) as u8);
        let previous = region
            .opcodes
            .contains_key("sw_previous")
            .then(|| region.key("sw_previous", 0).clamp(0, 127) as u8);
        let legato = region.opcodes.get("trigger").is_some_and(|s| s == "legato");
        for key in boundaries(lo, hi) {
            for velocity in boundaries(vlo, vhi) {
                probes
                    .entry(Probe {
                        key,
                        velocity,
                        switch,
                        previous,
                        legato,
                        cc: cc.clone(),
                    })
                    .or_default()
                    .push(region_index);
            }
        }
        let mut maximum =
            region.number("delay", 0.).max(0.) + region.number("delay_random", 0.).max(0.);
        for (name, value) in &region.opcodes {
            if name.starts_with("delay_cc") || name.starts_with("delay_oncc") {
                maximum += value.parse::<f64>().unwrap_or(0.).max(0.);
            }
        }
        delay = delay.max(maximum);
    }
    ensure!(
        probes.len() <= 100_000,
        "exercise predicate fixture budget exceeded"
    );
    // One dedicated long attack/release fixture covers delayed starts; boundary
    // fixtures otherwise intentionally inspect short event/state transitions.
    let frames = (delay * 48000.).ceil() as u64 + 1024;
    let (probes, targets): (Vec<_>, Vec<_>) = probes.into_iter().unzip();
    Ok((probes, controls, frames, targets))
}
#[derive(Default)]
struct ExerciseResult {
    hash: u64,
    frames: u64,
    cases: u64,
    peak: f32,
    max_active: usize,
    matched: u64,
    started: u64,
    stolen: u64,
    dropped: u64,
    nonzero: u64,
    finite: bool,
    process_nanos: u128,
    max_block_nanos: u128,
    deadline_exceedances: u64,
    baseline_stolen: u64,
    baseline_dropped: u64,
    overlap_stolen: u64,
    overlap_dropped: u64,
    region_started: Vec<u64>,
}
impl ExerciseResult {
    fn new() -> Self {
        Self {
            hash: 0xcbf29ce484222325,
            finite: true,
            ..Self::default()
        }
    }
    fn hash(&mut self, value: u64) {
        self.hash ^= value;
        self.hash = self.hash.wrapping_mul(0x100000001b3);
    }
    fn observe(&mut self, stats: SfzStatistics) {
        self.max_active = self.max_active.max(stats.active_voices);
    }
    fn finish_case(&mut self, processor: &dyn DeviceProcessor) {
        if let Some(stats) = processor.sfz_statistics() {
            self.matched += stats.matched_regions;
            self.started += stats.started_voices;
            self.stolen += stats.stolen_voices;
            self.dropped += stats.dropped_regions;
            for v in [
                stats.matched_regions,
                stats.started_voices,
                stats.stolen_voices,
                stats.dropped_regions,
            ] {
                self.hash(v);
            }
        }
        if let Some(activity) = processor.sfz_region_activity() {
            self.region_started.resize(activity.len(), 0);
            for (total, count) in self.region_started.iter_mut().zip(activity) {
                *total = total.saturating_add(*count);
            }
        }
        self.cases += 1;
    }
}
fn run_block(
    processor: &mut dyn DeviceProcessor,
    result: &mut ExerciseResult,
    frames: usize,
    events: &[DeviceEvent],
) -> Result<()> {
    let mut left = [0.; 256];
    let mut right = [0.; 256];
    let process_start = std::time::Instant::now();
    processor.process(
        ProcessContext {
            frames,
            block_start_sample: result.frames,
            transport: TransportSnapshot {
                sample_rate: 48000.,
                running: true,
                sample_position: result.frames,
                beat_position: result.frames as f64 / 24000.,
                current_tick: 0.,
                project_frame: result.frames as f64,
                bpm: 120.,
                meter: [4, 4],
                loop_ticks: 0,
                ended: false,
            },
        },
        events,
        &mut left,
        &mut right,
    )?;
    let elapsed = process_start.elapsed().as_nanos();
    result.process_nanos += elapsed;
    result.max_block_nanos = result.max_block_nanos.max(elapsed);
    result.deadline_exceedances += u64::from(elapsed > frames as u128 * 1_000_000_000 / 48_000);
    for (&a, &b) in left[..frames].iter().zip(&right[..frames]) {
        result.finite &= a.is_finite() && b.is_finite();
        result.peak = result.peak.max(a.abs()).max(b.abs());
        result.nonzero += u64::from(a != 0. || b != 0.);
        result.hash(a.to_bits() as u64);
        result.hash(b.to_bits() as u64);
    }
    result.frames += frames as u64;
    if let Some(stats) = processor.sfz_statistics() {
        result.observe(stats);
    }
    Ok(())
}
fn run_frames(
    processor: &mut dyn DeviceProcessor,
    result: &mut ExerciseResult,
    mut frames: u64,
) -> Result<()> {
    while frames > 0 {
        let count = frames.min(256) as usize;
        run_block(processor, result, count, &[])?;
        frames -= count as u64;
    }
    Ok(())
}
fn note(id: u64, key: u8, velocity: u8, on: bool) -> DeviceEventKind {
    if on {
        DeviceEventKind::NoteOn {
            sample_zone: None,
            pitch: key as f32,
            elapsed_frames: 0,
            note_id: id,
            channel: 0,
            key,
            velocity: velocity as f32 / 127.,
        }
    } else {
        DeviceEventKind::NoteOff {
            note_id: id,
            channel: 0,
            key,
            velocity: velocity as f32 / 127.,
        }
    }
}
fn event(kind: DeviceEventKind) -> DeviceEvent {
    DeviceEvent { offset: 0, kind }
}
fn start_probe(
    processor: &mut dyn DeviceProcessor,
    result: &mut ExerciseResult,
    probe: &Probe,
) -> Result<()> {
    let mut setup = Vec::new();
    for &(controller, value) in &probe.cc {
        setup.push(event(DeviceEventKind::Controller {
            channel: 0,
            controller,
            value,
        }));
    }
    if let Some(key) = probe.switch {
        setup.push(event(note(10, key, 127, true)));
        setup.push(event(note(10, key, 0, false)));
    }
    if probe.legato || probe.previous.is_some() {
        let key = probe.previous.unwrap_or(probe.key.saturating_sub(1));
        setup.push(event(note(11, key, probe.velocity, true)));
    }
    run_block(processor, result, 1, &setup)?;
    Ok(())
}
fn exercise_once(
    processor: &mut dyn DeviceProcessor,
    probes: &[Probe],
    controls: &BTreeMap<u8, BTreeSet<u8>>,
    hold_frames: u64,
    targets: &[Vec<usize>],
    program: &Program,
) -> Result<ExerciseResult> {
    let mut result = ExerciseResult::new();
    for (probe, targets) in probes.iter().zip(targets) {
        processor.reset();
        start_probe(processor, &mut result, probe)?;
        // Continue selection history within this fixture instead of resetting
        // RNG/sequence for every attempt. The explicit cap reports remaining
        // authored branches as unhit, rather than claiming exhaustive reach.
        let attempts = targets
            .iter()
            .map(|&index| {
                let region = &program.regions[index];
                let sequence = region.integer("seq_length", 1).max(1) as usize;
                let width = region.number("hirand", 1.) - region.number("lorand", 0.);
                let random = if width > 0. && width < 1. {
                    (16. / width).ceil() as usize
                } else {
                    1
                };
                sequence.max(random).min(256)
            })
            .max()
            .unwrap_or(1);
        for attempt in 0..attempts {
            let id = attempt as u64 + 1;
            run_block(
                processor,
                &mut result,
                32,
                &[event(note(id, probe.key, probe.velocity, true))],
            )?;
            run_block(
                processor,
                &mut result,
                32,
                &[event(note(id, probe.key, 0, false))],
            )?;
            if processor
                .sfz_region_activity()
                .is_some_and(|activity| targets.iter().all(|&index| activity[index] > 0))
            {
                break;
            }
        }
        result.finish_case(processor);
    }
    if let Some(anchor) = probes.iter().max_by_key(|probe| {
        (
            probe.velocity,
            std::cmp::Reverse((probe.key as i16 - 60).abs()),
        )
    }) {
        for (&controller, values) in controls {
            for &value in values {
                processor.reset();
                start_probe(processor, &mut result, anchor)?;
                run_block(
                    processor,
                    &mut result,
                    32,
                    &[event(note(1, anchor.key, anchor.velocity, true))],
                )?;
                run_block(
                    processor,
                    &mut result,
                    32,
                    &[event(DeviceEventKind::Controller {
                        channel: 0,
                        controller,
                        value,
                    })],
                )?;
                run_block(
                    processor,
                    &mut result,
                    32,
                    &[event(note(1, anchor.key, 0, false))],
                )?;
                result.finish_case(processor);
            }
        }
        result.baseline_stolen = result.stolen;
        result.baseline_dropped = result.dropped;
        // Repeated overlapping same-key notes exercise sequence/random selection,
        // sustain, owned release, and source-authored choking/polyphony.
        processor.reset();
        start_probe(processor, &mut result, anchor)?;
        run_block(
            processor,
            &mut result,
            1,
            &[event(DeviceEventKind::Controller {
                channel: 0,
                controller: 64,
                value: 127,
            })],
        )?;
        for id in 1..=64 {
            run_block(
                processor,
                &mut result,
                32,
                &[event(note(id, anchor.key, anchor.velocity, true))],
            )?;
            run_block(
                processor,
                &mut result,
                32,
                &[event(note(id, anchor.key, 0, false))],
            )?;
        }
        run_block(
            processor,
            &mut result,
            32,
            &[event(DeviceEventKind::Controller {
                channel: 0,
                controller: 64,
                value: 0,
            })],
        )?;
        run_frames(processor, &mut result, 2048)?;
        result.finish_case(processor);
        result.overlap_stolen = result.stolen - result.baseline_stolen;
        result.overlap_dropped = result.dropped - result.baseline_dropped;
        processor.reset();
        start_probe(processor, &mut result, anchor)?;
        run_block(
            processor,
            &mut result,
            1,
            &[event(note(1, anchor.key, anchor.velocity, true))],
        )?;
        run_frames(processor, &mut result, hold_frames.min(48000))?;
        run_block(
            processor,
            &mut result,
            1,
            &[event(note(1, anchor.key, 0, false))],
        )?;
        run_frames(processor, &mut result, 4096)?;
        result.finish_case(processor);
        result.baseline_stolen = result.stolen - result.overlap_stolen;
        result.baseline_dropped = result.dropped - result.overlap_dropped;
    }
    Ok(result)
}
fn exercise_program(
    processor: &mut dyn DeviceProcessor,
    program: &Program,
) -> Result<serde_json::Value> {
    let (probes, controls, hold_frames, targets) = probe_plan(program)?;
    let start = std::time::Instant::now();
    let first = exercise_once(
        processor,
        &probes,
        &controls,
        hold_frames,
        &targets,
        program,
    )?;
    let elapsed = start.elapsed();
    let second = exercise_once(
        processor,
        &probes,
        &controls,
        hold_frames,
        &targets,
        program,
    )?;
    ensure!(
        first.hash == second.hash && first.frames == second.frames,
        "exercise render/state trace was not deterministic"
    );
    ensure!(
        first.finite && second.finite,
        "exercise produced nonfinite samples"
    );
    let unhit: Vec<_> = program.regions.iter().enumerate().filter_map(|(index, region)| {
        (first.region_started.get(index).copied().unwrap_or(0) == 0).then(|| serde_json::json!({
            "index":index,"source":region.source.path,"line":region.source.line,"sample":region.sample,
            "trigger":region.opcodes.get("trigger"),"seq_length":region.opcodes.get("seq_length"),
            "seq_position":region.opcodes.get("seq_position"),"lorand":region.opcodes.get("lorand"),"hirand":region.opcodes.get("hirand")
        }))
    }).collect();
    Ok(
        serde_json::json!({"passed":true,"predicate_fixtures":probes.len(),"physical_controller_values":controls,"cases":first.cases,"rendered_frames":first.frames,"nonzero_frames":first.nonzero,"peak":first.peak,"max_active_voices":first.max_active,"matched_regions":first.matched,"started_voices":first.started,"stolen_voices":first.stolen,"dropped_regions":first.dropped,"baseline_stolen_voices":first.baseline_stolen,"baseline_dropped_regions":first.baseline_dropped,"overlap_stolen_voices":first.overlap_stolen,"overlap_dropped_regions":first.overlap_dropped,"deterministic_trace_fnv64":format!("{:016x}",first.hash),"deterministic_repeat":true,"finite_output":true,"elapsed_seconds_first_pass":elapsed.as_secs_f64(),"process_seconds_total":first.process_nanos as f64/1e9,"process_max_block_seconds":first.max_block_nanos as f64/1e9,"process_deadline_exceedances":first.deadline_exceedances,"timing_profile":if cfg!(debug_assertions){"debug"}else{"release"},"render_audio_seconds":first.frames as f64/48000.,"dedicated_delay_hold_frames":hold_frames.min(48000),"delay_hold_capped":hold_frames>48000,"region_reach_complete":unhit.is_empty(),"regions_total":program.regions.len(),"regions_started":first.region_started.iter().filter(|n|**n>0).count(),"unhit_regions":unhit,"selection_repeat_cap":256,"region_identity_coverage":"started region identities measured; unhit random/sequence branches and coupled variable/controller state space remain unqualified","scope":"native predicate/control/lifecycle smoke; not per-region or reference-player audio qualification"}),
    )
}
fn callback_percentiles(mut nanos: Vec<u64>) -> Result<(u64, u64, u64)> {
    ensure!(!nanos.is_empty(), "profile has no measured callbacks");
    nanos.sort_unstable();
    let rank = |percent: usize| nanos[(nanos.len() * percent).div_ceil(100).saturating_sub(1)];
    Ok((rank(50), rank(99), *nanos.last().unwrap()))
}
fn profile_request(request: &Request) -> Result<serde_json::Value> {
    use sha2::{Digest, Sha256};
    let program = muz::sfz::load(
        &request.path,
        &Options {
            defines: request.defines.clone(),
            source_overlays: request.source_overlays.clone(),
            ..Options::default()
        },
    )?;
    let executable = std::env::current_exe()?;
    let binary_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&executable)?));
    let normalized_bytes = serde_json::to_vec(&program)?;
    let normalized_sha256 = format!("{:x}", Sha256::digest(&normalized_bytes));
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|line| line.split_once(':'))
                .map(|(_, value)| value.trim().to_string())
        });
    let command = |name: &str, args: &[&str]| {
        std::process::Command::new(name)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let affinity = std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            line.strip_prefix("Cpus_allowed_list:")
                .map(|value| value.trim().to_string())
        });
    let pinned_cpu = affinity
        .as_ref()
        .and_then(|value| value.parse::<u16>().ok());
    let frequency_snapshot = pinned_cpu.map(|cpu| {
        let read = |name: &str| std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/{name}")).ok().map(|value|value.trim().to_string());
        serde_json::json!({"cpu":cpu,"governor":read("scaling_governor"),"min_khz":read("scaling_min_freq"),"max_khz":read("scaling_max_freq"),"current_khz":read("scaling_cur_freq"),"scope":"snapshot before preparations, outside callback timers; unavailable fields null; no configuration changes"})
    });
    let hardware = serde_json::json!({"cpu":cpu,"pinned_cpu_frequency_snapshot":frequency_snapshot,"architecture":std::env::consts::ARCH,"os":std::env::consts::OS,"logical_parallelism":std::thread::available_parallelism().ok().map(|n|n.get()),"cpu_affinity":std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().find_map(|line|line.strip_prefix("Cpus_allowed_list:").map(|value|value.trim().to_string())),"rustc":command("rustc",&["--version","--verbose"]),"git_revision":command("git",&["rev-parse","HEAD"]),"git_status":command("git",&["status","--porcelain"]),"timing":"std::time::Instant around DeviceProcessor::process only; inspection/event construction excluded","build_profile":if cfg!(debug_assertions){"debug"}else{"release"}});
    let (probes, _, _, targets) = probe_plan(&program)?;
    ensure!(
        request.profile_switch.is_none_or(|key| key <= 127),
        "profile_switch exceeds MIDI key range"
    );
    let anchor = probes
        .iter()
        .zip(&targets)
        .filter(|(probe, _)| {
            request
                .profile_switch
                .is_none_or(|key| probe.switch == Some(key))
        })
        .filter(|(_, indices)| {
            indices.iter().any(|&i| {
                program.regions[i].sample.is_some()
                    && program.regions[i].number("end", 0.) != -1.
                    && !matches!(
                        program.regions[i]
                            .opcodes
                            .get("trigger")
                            .map(String::as_str),
                        Some("release" | "release_key")
                    )
            })
        })
        .min_by_key(|(probe, _)| {
            (
                (probe.key as i16 - 60).abs(),
                (probe.velocity as i16 - 100).abs(),
            )
        })
        .map(|(probe, _)| probe)
        .context("profile has no audible attack predicate")?;
    let switches: BTreeSet<_> = program
        .regions
        .iter()
        .filter(|r| r.opcodes.contains_key("sw_last"))
        .map(|r| r.key("sw_last", 0) as u8)
        .collect();
    let mut playable = BTreeSet::new();
    for region in &program.regions {
        if region.sample.is_none() || region.number("end", 0.) == -1. {
            continue;
        }
        let (lo, hi) = if region.opcodes.contains_key("key") {
            let key = region.key("key", 60);
            (key, key)
        } else {
            (region.key("lokey", 0), region.key("hikey", 127))
        };
        for key in lo.max(0)..=hi.min(127) {
            if !switches.contains(&(key as u8)) {
                playable.insert(key as u8);
            }
        }
    }
    ensure!(!playable.is_empty(), "profile has no playable keys");
    let nearest = |wanted: u8| {
        *playable
            .iter()
            .min_by_key(|&&key| (key as i16 - wanted as i16).abs())
            .unwrap()
    };
    let keys = match request.profile_workload {
        ProfileWorkload::FourHeldNotes => vec![nearest(48), nearest(55), nearest(60), nearest(64)],
        ProfileWorkload::DrumPattern => [36, 42, 38, 42, 36, 46, 38, 51].map(nearest).to_vec(),
    };
    let dev = device(request, &program);
    let mut rate_reports = Vec::new();
    let mut passed = !cfg!(debug_assertions);
    for sample_rate in [44100.0_f64, 48000., 96000.] {
        let mut prepare_seconds = Vec::new();
        let mut prepared = None;
        for attempt in 0..1 {
            drop(prepared.take());
            eprintln!(
                "SFZ profile {} rate={sample_rate} preparation={}",
                request.path.display(),
                attempt + 1
            );
            let start = std::time::Instant::now();
            let processor = create_processor(
                &dev,
                AudioConfig {
                    sample_rate: sample_rate as f32,
                    max_frames: 1024,
                    offline: true,
                },
            )?;
            prepare_seconds.push(start.elapsed().as_secs_f64());
            prepared = Some(processor);
        }
        let processor = prepared.as_mut().unwrap();
        let prepared_stats = processor
            .sfz_statistics()
            .context("profile requires native SFZ statistics")?;
        let rss_kib = std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<u64>().ok())
            });
        let mut block_reports = Vec::new();
        for frames in [64, 256, 1024] {
            processor.reset();
            let mut setup: Vec<_> = anchor
                .cc
                .iter()
                .map(|&(controller, value)| {
                    event(DeviceEventKind::Controller {
                        channel: 0,
                        controller,
                        value,
                    })
                })
                .collect();
            if let Some(key) = anchor.switch {
                setup.push(event(note(u64::MAX - 10, key, 127, true)));
                setup.push(event(note(u64::MAX - 10, key, 0, false)));
            }
            let mut left = [0.; 1024];
            let mut right = [0.; 1024];
            let context = |position: u64| ProcessContext {
                frames,
                block_start_sample: position,
                transport: TransportSnapshot {
                    sample_rate,
                    running: true,
                    sample_position: position,
                    beat_position: position as f64 / sample_rate * 2.,
                    current_tick: 0.,
                    project_frame: position as f64,
                    bpm: 120.,
                    meter: [4, 4],
                    loop_ticks: 0,
                    ended: false,
                },
            };
            processor.process(context(0), &setup, &mut left, &mut right)?;
            let total = (sample_rate * 3.) as u64;
            let mut scheduled = Vec::<(u64, DeviceEventKind)>::new();
            match request.profile_workload {
                ProfileWorkload::FourHeldNotes => {
                    for (index, &key) in keys.iter().enumerate() {
                        scheduled.push((0, note(index as u64 + 1, key, 100, true)));
                        scheduled.push((
                            (sample_rate * 2.) as u64,
                            note(index as u64 + 1, key, 0, false),
                        ));
                    }
                }
                ProfileWorkload::DrumPattern => {
                    for index in 0..24 {
                        let at = (sample_rate * index as f64 / 8.).round() as u64;
                        let key = keys[index % keys.len()];
                        let velocity = [100, 72, 110, 80, 90, 105, 100, 70][index % 8];
                        scheduled.push((at, note(index as u64 + 1, key, velocity, true)));
                        scheduled.push((
                            at + (sample_rate / 32.).round() as u64,
                            note(index as u64 + 1, key, 0, false),
                        ));
                    }
                }
            }
            scheduled.sort_by_key(|(at, _)| *at);
            let mut durations = Vec::new();
            let mut position = 0u64;
            let mut next_event = 0;
            let mut finite = true;
            let mut max_active = 0;
            let mut max_notes = 0;
            let mut nonzero = 0u64;
            let mut peak = 0f32;
            let mut deadline_exceedances = 0u64;
            let mut hash = 0xcbf29ce484222325u64;
            while position < total {
                let mut events = Vec::new();
                while next_event < scheduled.len()
                    && scheduled[next_event].0 < position + frames as u64
                {
                    let (at, kind) = scheduled[next_event];
                    events.push(DeviceEvent {
                        offset: (at - position) as u32,
                        kind,
                    });
                    next_event += 1;
                }
                let process_context = context(position);
                let start = std::time::Instant::now();
                processor.process(process_context, &events, &mut left, &mut right)?;
                let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                durations.push(elapsed);
                deadline_exceedances +=
                    u64::from(elapsed as f64 >= frames as f64 / sample_rate * 1e9);
                for (&a, &b) in left[..frames].iter().zip(&right[..frames]) {
                    finite &= a.is_finite() && b.is_finite();
                    peak = peak.max(a.abs()).max(b.abs());
                    nonzero += u64::from(a != 0. || b != 0.);
                    for value in [a, b] {
                        hash ^= value.to_bits() as u64;
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                }
                if let Some(stats) = processor.sfz_statistics() {
                    max_active = max_active.max(stats.active_voices);
                    max_notes = max_notes.max(stats.active_notes);
                }
                position += frames as u64;
            }
            let callbacks = durations.len();
            let (p50, p99, max) = callback_percentiles(durations)?;
            let stats = processor.sfz_statistics().unwrap();
            let half_deadline = frames as f64 / sample_rate * 0.5e9;
            let gate = finite
                && nonzero > 0
                && stats.dropped_regions == 0
                && deadline_exceedances == 0
                && (p99 as f64) < half_deadline;
            passed &= gate;
            block_reports.push(serde_json::json!({"frames_per_callback":frames,"callbacks":callbacks,"rendered_frames":position,"audio_seconds":position as f64/sample_rate,"p50_seconds":p50 as f64/1e9,"p99_seconds":p99 as f64/1e9,"max_seconds":max as f64/1e9,"half_callback_deadline_seconds":half_deadline/1e9,"deadline_exceedances":deadline_exceedances,"passed":gate,"finite_output":finite,"nonzero_frames":nonzero,"peak":peak,"max_active_voices_at_callback_end":max_active,"max_active_notes_at_callback_end":max_notes,"started_voices":stats.started_voices,"stolen_voices":stats.stolen_voices,"dropped_regions":stats.dropped_regions,"pcm_fnv64":format!("{hash:016x}")}));
        }
        rate_reports.push(serde_json::json!({"sample_rate":sample_rate,"prepare_seconds":prepare_seconds,"preparation_repeats":1,"preparation_cache_policy":"sequential fresh processor; filesystem/asset resolver caches remain warm","decoded_frames":prepared_stats.decoded_frames,"decoded_bytes":prepared_stats.decoded_frames as u64*8,"process_rss_kib_after_preparation":rss_kib,"process_peak_rss_kib_cumulative":std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().find_map(|line|line.strip_prefix("VmHWM:").and_then(|value|value.split_whitespace().next()).and_then(|value|value.parse::<u64>().ok())),"compiled_dsp_programs":prepared_stats.compiled_dsp_programs,"compiled_dsp_bytes_shallow":prepared_stats.compiled_dsp_bytes_shallow,"compiled_memory_exclusions":"shallow sizeof only, excludes heap allocations/Arc headers/sample storage","voice_capacity":prepared_stats.max_voices,"blocks":block_reports}));
    }
    Ok(
        serde_json::json!({"path":request.path,"stage":"declared_workload_profile","profile":{"passed":passed,"hardware":hardware,"binary_sha256":binary_sha256,"normalized_program_sha256":normalized_sha256,"normalized_program_bytes":normalized_bytes.len(),"normalized_hash_scope":"run identity includes resolved absolute paths; not a portable publisher patch hash; original source/version pins remain in corpus manifests","workload":format!("{:?}",request.profile_workload),"keys":keys,"velocity_policy":"held=100; drum pattern=[100,72,110,80,90,105,100,70]","tempo_bpm":120,"setup_controllers":anchor.cc,"setup_switch":anchor.switch,"workload_duration_seconds":3,"workload_definition":"four simultaneous owned notes held2seconds then release1second, or24 drum hits at8steps/second with31.25ms note gates; mapped to nearest available source key","gate":"release build; finite audible output; no dropped region starts; no deadline overruns; nearest-rank p99 below half callback deadline; callback-end voice maxima only","rates":rate_reports}}),
    )
}

fn main() -> Result<()> {
    let mut prepare = false;
    let mut exercise = false;
    let mut profile = false;
    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        if arg == "--profile" {
            profile = true;
        } else if arg == "--exercise" {
            exercise = true;
            prepare = true;
        } else if arg == "--prepare" {
            prepare = true;
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    ensure!(
        !profile || (!prepare && !exercise),
        "--profile is a separate workload mode; combine neither --prepare nor --exercise"
    );
    let requests = if paths.is_empty() {
        let mut requests = Vec::new();
        for (line_no, line) in io::stdin().lock().lines().enumerate() {
            let line = line?;
            ensure!(line.len() <= 64 * 1024, "audit request exceeds size limit");
            if !line.trim().is_empty() {
                requests.push(
                    serde_json::from_str::<Request>(&line)
                        .with_context(|| format!("invalid audit request line {}", line_no + 1))?,
                );
            }
        }
        requests
    } else {
        paths
            .into_iter()
            .map(|path| Request {
                path,
                defines: BTreeMap::new(),
                source_overlays: Vec::new(),
                profile_workload: ProfileWorkload::default(),
                profile_switch: None,
            })
            .collect()
    };
    ensure!(requests.len() <= 10000, "too many audit requests");
    let mut failed = false;
    for request in requests {
        let report = match if profile {
            profile_request(&request)
        } else {
            audit(&request, prepare, exercise)
        } {
            Ok(report) => report,
            Err(error) => serde_json::json!({"path":request.path,"error":format!("{error:#}")}),
        };
        failed |= report.get("error").is_some()
            || (profile && report["profile"]["passed"] != true)
            || (prepare && report["prepared"] != true)
            || (exercise
                && (report.get("exercise_error").is_some()
                    || report["exercise"]["passed"] != true));
        println!("{}", serde_json::to_string(&report)?);
    }
    ensure!(
        !failed,
        "SFZ audit contains failed sources/preparation/exercise/profile; inspect JSON reports"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Request) {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        let mut writer = hound::WavWriter::create(
            wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for i in 0..8000 {
            writer.write_sample((i as f32 * 0.15).sin() * 0.25).unwrap();
        }
        writer.finalize().unwrap();
        let path = dir.path().join("test.sfz");
        std::fs::write(&path,"<control> set_cc7=127 set_cc11=127 label_cc25=Offset <group> sample=tone.wav locc1=10 hicc1=90 lokey=60 hikey=64 lovel=20 hivel=100 sw_lokey=10 sw_hikey=11 sw_last=10 sw_default=10 ampeg_release=0.01 offset_oncc25=10 offset_curvecc25=2 <region> <region> trigger=release rt_decay=7").unwrap();
        (
            dir,
            Request {
                path,
                defines: BTreeMap::new(),
                source_overlays: vec![],
                profile_workload: ProfileWorkload::default(),
                profile_switch: None,
            },
        )
    }
    #[test]
    fn region_probe_plan_covers_key_velocity_cc_boundaries_and_switches() {
        let (_dir, request) = fixture();
        let program = muz::sfz::load(&request.path, &Options::default()).unwrap();
        let (probes, controls, delay, targets) = probe_plan(&program).unwrap();
        assert!(targets.iter().all(|indices| indices == &[0, 1]));
        assert_eq!(probes.len(), 9);
        assert!(
            probes
                .iter()
                .all(|p| p.switch == Some(10) && p.cc == vec![(1, 50)])
        );
        assert_eq!(
            probes.iter().map(|p| p.key).collect::<BTreeSet<_>>(),
            BTreeSet::from([60, 62, 64])
        );
        assert_eq!(
            probes.iter().map(|p| p.velocity).collect::<BTreeSet<_>>(),
            BTreeSet::from([20, 60, 100])
        );
        assert!(controls[&1].is_superset(&BTreeSet::from([9, 10, 50, 90, 91])));
        assert!(controls.contains_key(&25));
        assert_eq!(delay, 1024);
    }
    #[test]
    fn native_exercise_reports_finite_deterministic_reachable_lifecycle() {
        let (_dir, request) = fixture();
        let report = audit(&request, true, true).unwrap();
        assert_eq!(report["prepared"], true);
        assert_eq!(report["exercise"]["passed"], true);
        assert_eq!(report["exercise"]["deterministic_repeat"], true);
        assert_eq!(report["exercise"]["finite_output"], true);
        assert_eq!(report["exercise"]["regions_total"], 2);
        assert_eq!(report["exercise"]["regions_started"], 2);
        assert_eq!(report["exercise"]["region_reach_complete"], true);
        assert_eq!(report["exercise"]["unhit_regions"], serde_json::json!([]));
        assert!(report["exercise"]["nonzero_frames"].as_u64().unwrap() > 0);
        assert!(report["exercise"]["started_voices"].as_u64().unwrap() > 0);
        assert!(report["exercise"]["cases"].as_u64().unwrap() > 9);
    }
    #[test]
    fn native_reach_repeats_sequence_and_reports_authored_unreachable_region() {
        let (_dir, request) = fixture();
        std::fs::write(&request.path, "<control> set_cc7=127 set_cc11=127 <group> sample=tone.wav key=60 seq_length=3 <region> seq_position=1 <region> seq_position=2 <region> seq_position=3 <region> seq_position=4").unwrap();
        let report = audit(&request, true, true).unwrap();
        assert_eq!(report["prepared"], true);
        assert_eq!(report["exercise"]["passed"], true);
        assert_eq!(report["exercise"]["regions_started"], 3);
        assert_eq!(report["exercise"]["region_reach_complete"], false);
        assert_eq!(
            report["exercise"]["unhit_regions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(report["exercise"]["unhit_regions"][0]["index"], 3);
    }
    #[test]
    fn callback_profile_percentiles_use_documented_nearest_rank() {
        assert_eq!(
            callback_percentiles((0..100).collect()).unwrap(),
            (49, 98, 99)
        );
        assert_eq!(callback_percentiles(vec![9, 1, 5]).unwrap(), (5, 9, 9));
        assert!(callback_percentiles(vec![]).is_err());
        let request: Request =
            serde_json::from_str(r#"{"path":"kit.sfz","profile_workload":"drum_pattern"}"#)
                .unwrap();
        assert!(matches!(
            request.profile_workload,
            ProfileWorkload::DrumPattern
        ));
        let request: Request =
            serde_json::from_str(r#"{"path":"metal.sfz","profile_switch":17}"#).unwrap();
        assert_eq!(request.profile_switch, Some(17));
        assert!(
            serde_json::from_str::<Request>(r#"{"path":"metal.sfz","profile_switch":256}"#)
                .is_err()
        );
    }
}
