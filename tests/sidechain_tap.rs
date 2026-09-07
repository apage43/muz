//! Constant probes expose routing boundaries without depending on a drum recipe.
fn bounce(source: &str, tap: Option<&str>, solo: &[String]) -> Vec<f32> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tap.muz");
    let audio = dir.path().join("tap.wav");
    std::fs::write(&path, source).unwrap();
    let session = muz::compile::compile(&path).unwrap().session;
    muz::render::render_with(
        session,
        &audio,
        &muz::render::RenderOptions {
            tap: tap.map(str::to_owned),
            solo: solo.to_vec(),
            block_size: 97,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    hound::WavReader::open(audio)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

fn source(effect: &str, on_bus: bool) -> String {
    let track_effect = if on_bus { "" } else { effect };
    let bus_effect = if on_bus { effect } else { "" };
    format!(
        r#"
        fn probe(level) = voice_patch("probe",{{gain_db:0,nodes:[
            {{id:"level",op:"param",value:level,min:0,max:1}}
        ],output:"level"}});
        let held = note("C4",2b,velocity=1).gate(1);
        song({{tempo:120,tracks:[
            track("receiver",held,probe(0.1),{{chain:[{track_effect}],output:"return"}}),
            track("key",held,probe(0.5),{{
                gain:-12,pan:0,chain:[fx("gain",{{id:"trim",gain_db:0}})],
                output:"key_bus",sends:{{key_send:-12}}
            }})
        ],buses:[
            bus("return",[{bus_effect}]),
            bus("key_bus",[fx("gain",{{gain_db:0}})]),
            bus("key_send",[])
        ],automation:[],tail:0.05}})
        "#
    )
}

#[test]
fn external_detectors_follow_inserts_and_pan_but_ignore_output_routes() {
    for effect in [
        r#"fx("compressor",{sidechain:"key",threshold_db:-18,ratio:4,knee_db:0,attack_ms:1,release_ms:20})"#,
        r#"rack([[fx("gain")]],{sidechain:"key",modulate:[{
            target:"0.0.gain_db",base:0,follower:-40,attack_ms:1,release_ms:20,min:-60,max:0
        }]})"#,
    ] {
        for on_bus in [false, true] {
            let src = source(effect, on_bus);
            let tap = if on_bus { "return" } else { "receiver" };
            let baseline = bounce(&src, Some(tap), &[]);
            let bypass = bounce(&source("", on_bus), Some(tap), &[]);
            assert!(baseline[24_000] < bypass[24_000] * 0.6, "no ducking");
            for downstream_edit in [
                src.replace("gain:-12,pan:0", "gain:-60,pan:0"),
                src.replace(
                    "automation:[]",
                    r#"automation:[automation("key.out",curve([[0b,-12],[1/2b,-60],[1b,0]],"step"))]"#,
                ),
                src.replace("key_send:-12", "key_send:{gain:0,pre:true}"),
                src.replace(
                    r#"bus("key_bus",[fx("gain",{gain_db:0})])"#,
                    r#"bus("key_bus",[fx("gain",{gain_db:-60})])"#,
                ),
                src.replace("automation:[]", r#"master:[fx("gain",{gain_db:-60})],automation:[]"#),
            ] {
                assert_ne!(src, downstream_edit, "test edit missed its target");
                assert_eq!(baseline, bounce(&downstream_edit, Some(tap), &[]));
            }
            assert_eq!(
                baseline,
                bounce(&src, Some(tap), &["receiver".into()]),
                "solo stopped the muted detector source"
            );

            // Reduce the final insert, compensating on the output fader. The
            // source remains equally audible, while the receiver ducks less.
            let trimmed = src
                .replace(r#"id:"trim",gain_db:0"#, r#"id:"trim",gain_db:-12"#)
                .replace("gain:-12,pan:0", "gain:0,pan:0");
            let weaker = bounce(&trimmed, Some(tap), &[]);
            assert!(weaker[24_000] > baseline[24_000] * 1.5);
            // Disable the send here so the source's output alone measures compensation.
            let audible = |s: &str| {
                bounce(
                    &s.replace("sends:{key_send:-12}", "sends:{}"),
                    None,
                    &["key".into()],
                )
            };
            let original = audible(&src);
            let compensated = audible(&trimmed);
            assert_eq!(original.len(), compensated.len());
            assert!(
                original
                    .iter()
                    .zip(&compensated)
                    .all(|(a, b)| (a - b).abs() < 1e-6)
            );

            // Track pan is appended to the inserts. A mono hard pan raises the
            // larger channel by sqrt(2); peak-linked detection then ducks harder.
            for pan in [-1, 1] {
                let panned = src.replace("pan:0", &format!("pan:{pan}"));
                let actual = bounce(&panned, Some(tap), &[]);
                assert!(actual[24_000] < baseline[24_000] * 0.85);
            }
        }
    }
}
