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
        "dependencies":program.dependencies,"opcodes":opcodes,
        "diagnostics":program.diagnostics,"unsupported_behaviors":unsupported,
        "controls":program.controls,"labels":program.labels,"curves":program.curves,
        "stage":"normalized_source",
    });
    if prepare {
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
fn probe_plan(program: &Program) -> Result<(Vec<Probe>, BTreeMap<u8, BTreeSet<u8>>, u64)> {
    let mut probes = BTreeSet::new();
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
    for region in &program.regions {
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
                probes.insert(Probe {
                    key,
                    velocity,
                    switch,
                    previous,
                    legato,
                    cc: cc.clone(),
                });
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
    Ok((probes.into_iter().collect(), controls, frames))
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
) -> Result<ExerciseResult> {
    let mut result = ExerciseResult::new();
    for probe in probes {
        processor.reset();
        start_probe(processor, &mut result, probe)?;
        run_block(
            processor,
            &mut result,
            32,
            &[event(note(1, probe.key, probe.velocity, true))],
        )?;
        run_block(
            processor,
            &mut result,
            32,
            &[event(note(1, probe.key, 0, false))],
        )?;
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
    }
    Ok(result)
}
fn exercise_program(
    processor: &mut dyn DeviceProcessor,
    program: &Program,
) -> Result<serde_json::Value> {
    let (probes, controls, hold_frames) = probe_plan(program)?;
    let start = std::time::Instant::now();
    let first = exercise_once(processor, &probes, &controls, hold_frames)?;
    let elapsed = start.elapsed();
    let second = exercise_once(processor, &probes, &controls, hold_frames)?;
    ensure!(
        first.hash == second.hash && first.frames == second.frames,
        "exercise render/state trace was not deterministic"
    );
    ensure!(
        first.finite && second.finite,
        "exercise produced nonfinite samples"
    );
    Ok(
        serde_json::json!({"passed":true,"predicate_fixtures":probes.len(),"physical_controller_values":controls,"cases":first.cases,"rendered_frames":first.frames,"nonzero_frames":first.nonzero,"peak":first.peak,"max_active_voices":first.max_active,"matched_regions":first.matched,"started_voices":first.started,"stolen_voices":first.stolen,"dropped_regions":first.dropped,"deterministic_trace_fnv64":format!("{:016x}",first.hash),"deterministic_repeat":true,"finite_output":true,"elapsed_seconds_first_pass":elapsed.as_secs_f64(),"render_audio_seconds":first.frames as f64/48000.,"dedicated_delay_hold_frames":hold_frames.min(48000),"delay_hold_capped":hold_frames>48000,"scope":"native predicate/control/lifecycle smoke; not per-region or reference-player audio qualification"}),
    )
}
fn main() -> Result<()> {
    let mut prepare = false;
    let mut exercise = false;
    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        if arg == "--exercise" {
            exercise = true;
            prepare = true;
        } else if arg == "--prepare" {
            prepare = true;
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
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
            })
            .collect()
    };
    ensure!(requests.len() <= 10000, "too many audit requests");
    for request in requests {
        let report = match audit(&request, prepare, exercise) {
            Ok(report) => report,
            Err(error) => serde_json::json!({"path":request.path,"error":format!("{error:#}")}),
        };
        println!("{}", serde_json::to_string(&report)?);
    }
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
            },
        )
    }
    #[test]
    fn region_probe_plan_covers_key_velocity_cc_boundaries_and_switches() {
        let (_dir, request) = fixture();
        let program = muz::sfz::load(&request.path, &Options::default()).unwrap();
        let (probes, controls, delay) = probe_plan(&program).unwrap();
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
        assert!(report["exercise"]["nonzero_frames"].as_u64().unwrap() > 0);
        assert!(report["exercise"]["started_voices"].as_u64().unwrap() > 0);
        assert!(report["exercise"]["cases"].as_u64().unwrap() > 9);
    }
}
