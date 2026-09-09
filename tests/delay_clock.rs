//! Impulses exercise the native insert's clock, including live parameter changes.
use muz::audio::{AudioConfig, DeviceProcessor, ProcessContext, TransportSnapshot};

fn delay() -> Box<dyn DeviceProcessor> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delay.muz");
    std::fs::write(
        &path,
        r#"song({tracks:[track("p",note(60),synth("init"),{
        chain:[fx("delay",{time_ms:11.7,time_beats:0.03125,mix:1,feedback:0})]
    })]})"#,
    )
    .unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    muz::audio::create_processor(
        &session.tracks[0].inserts[0],
        AudioConfig {
            sample_rate: 48_000.,
            max_frames: 256,
        },
    )
    .unwrap()
}

fn impulse(
    processor: &mut dyn DeviceProcessor,
    block: usize,
    bpm: f64,
    change_tempo: bool,
) -> Vec<f32> {
    processor.reset();
    let mut output = Vec::new();
    while output.len() < 1800 {
        let start = output.len();
        let frames = block.min(1800 - start);
        let mut left = vec![0.; frames];
        let mut right = vec![0.; frames];
        if start == 0 {
            left[0] = 1.;
            right[0] = 0.5;
        }
        let transport = TransportSnapshot {
            sample_rate: 48_000.,
            running: true,
            sample_position: start as u64,
            beat_position: 0.,
            current_tick: 0.,
            project_frame: start as f64,
            bpm: if change_tempo && start > 0 {
                bpm / 2.
            } else {
                bpm
            },
            meter: [4, 4],
            loop_ticks: 0,
            ended: false,
        };
        processor
            .process(
                ProcessContext {
                    frames,
                    block_start_sample: start as u64,
                    transport,
                },
                &[],
                &mut left,
                &mut right,
            )
            .unwrap();
        assert!(left.iter().zip(&right).all(|(l, r)| *r == *l * 0.5));
        output.extend(left);
    }
    output
}

fn expected(at: usize) -> Vec<f32> {
    let mut output = vec![0.; 1800];
    output[at] = 1.;
    output
}

#[test]
fn clock_delay_is_sample_rounded_and_independent_of_tempo_and_blocks() {
    let mut processor = delay();
    for (ms, sample) in [(11.7, 562), (17.3, 830), (0.00001, 1)] {
        processor.set_parameter("time_ms", ms).unwrap();
        for (bpm, block, changes) in [(132., 97, false), (60., 256, false), (120., 97, true)] {
            assert_eq!(
                impulse(&mut *processor, block, bpm, changes),
                expected(sample)
            );
        }
    }
    // Zero explicitly switches back to tempo timing, even after clock automation.
    processor.set_parameter("time_ms", 0.).unwrap();
    assert_eq!(impulse(&mut *processor, 97, 120., false), expected(750));
    assert_eq!(impulse(&mut *processor, 256, 60., false), expected(1500));
    processor.set_parameter("time_ms", 17.3).unwrap();
    processor.set_parameter("time_beats", 2.).unwrap();
    assert_eq!(impulse(&mut *processor, 256, 132., false), expected(830));
}

#[test]
fn clock_delay_preserves_feedback_and_validates_its_storage_bounds() {
    let mut processor = delay();
    processor.set_parameter("feedback", 0.5).unwrap();
    let mut echoes = expected(562);
    echoes[1124] = 0.5;
    echoes[1686] = 0.25;
    assert_eq!(impulse(&mut *processor, 97, 132., true), echoes);
    processor.set_parameter("time_ms", 48_000.).unwrap();
    for invalid in [-1., 48_001., f32::NAN, f32::INFINITY] {
        assert!(processor.set_parameter("time_ms", invalid).is_err());
    }
    let catalog = muz::plugins::native("delay").unwrap();
    assert!(
        catalog["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "time_ms")
    );
}
