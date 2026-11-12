//! Logical insert lanes lower to the same physical graph as explicit lanes.
use muz::{
    Session,
    audio::{AudioConfig, AudioEngine},
};

fn compile(source: &str) -> anyhow::Result<Session> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("kit.muz");
    std::fs::write(&path, source)?;
    Ok(muz::compile::compile(&path)?.session)
}

fn song(lanes: &str) -> String {
    format!(
        r#"song({{
        tracks:[track("drums",drums({{kick:"X...",hat:"x.x."}}),kit("default",{{
            kick:{{instrument:synth("kick"),chain:[fx("gain",{{gain_db:-3}})]}}
        }}),{{chain:[fx("lowpass"),fx("gain",{{id:"level"}})],sends:{{room:-6}}}})],
        buses:[bus("room",[])],
        automation:[{lanes}],tail:0
    }})"#
    )
}

fn audio(session: &Session) -> Vec<f32> {
    let mut engine = AudioEngine::new(
        session,
        AudioConfig {
            sample_rate: 8000.,
            max_frames: 64,
            offline: true,
        },
    )
    .unwrap();
    engine.set_running(true);
    let mut samples = vec![0.; 8192];
    for block in samples.chunks_mut(128) {
        engine.render_interleaved(block, 2).unwrap();
    }
    samples
}

#[test]
fn logical_lanes_match_explicit_lanes_with_voice_inserts_and_sends() {
    let logical = compile(&song(
        r#"
        automation("drums.fx0.cutoff_hz",curve([[0b,200],[1b,8000]],"smooth")),
        automation("drums.level.gain_db",curve([[0b,-12],[1b,0]],"step"))
    "#,
    ))
    .unwrap();
    let explicit = compile(&song(
        r#"
        automation("drums.hat.fx0.cutoff_hz",curve([[0b,200],[1b,8000]],"smooth")),
        automation("drums.kick.fx1.cutoff_hz",curve([[0b,200],[1b,8000]],"smooth")),
        automation("drums.hat.level.gain_db",curve([[0b,-12],[1b,0]],"step")),
        automation("drums.kick.level.gain_db",curve([[0b,-12],[1b,0]],"step"))
    "#,
    ))
    .unwrap();
    let lanes = |s: &Session| serde_json::to_value(&s.extras.automation).unwrap();
    assert_eq!(lanes(&logical), lanes(&explicit));
    let actual = audio(&logical);
    assert!(actual.iter().any(|s| s.abs() > 0.0001));
    assert_eq!(actual, audio(&explicit));
    let static_audio = audio(&compile(&song("")).unwrap());
    assert_ne!(actual, static_audio);
}

#[test]
fn logical_and_physical_lanes_cannot_own_the_same_target() {
    for lanes in [
        r#"automation("drums.level.gain_db",curve([[0b,-6]])), automation("drums.kick.level.gain_db",curve([[0b,-3]]))"#,
        r#"automation("drums.kick.level.gain_db",curve([[0b,-3]])), automation("drums.level.gain_db",curve([[0b,-6]]))"#,
    ] {
        let session = compile(&song(lanes)).unwrap();
        let error = match AudioEngine::new(
            &session,
            AudioConfig {
                sample_rate: 8000.,
                max_frames: 64,
                offline: true,
            },
        ) {
            Ok(_) => panic!("accepted conflicting lanes"),
            Err(e) => e.to_string(),
        };
        assert!(
            error.contains("drums.kick.level.gain_db' has more than one lane"),
            "{error}"
        );
        assert!(error.contains("kit.muz:6:"), "{error}");
    }
}

#[test]
fn empty_kits_reject_lanes_and_dotted_ids_resolve_exactly() {
    let source = r#"song({tracks:[
        track("empty",drums({kick:"...."}),kit(),{chain:[fx("gain",{id:"level"})]}),
        track("other",note(60),synth("bell"))
    ],automation:[automation("empty.level.gain_db",curve([[0b,-6]]))]})"#;
    let error = compile(source).unwrap_err().to_string();
    assert!(error.contains("has no expanded kit voices"), "{error}");
    let source = song(r#"automation("drums.level.gain_db",curve([[0b,-6]]))"#)
        .replace("drums", "section.drums");
    // Keep the builtin name; only authored IDs should change.
    let source = source.replace("section.drums({", "drums({");
    let session = compile(&source).unwrap();
    assert_eq!(
        session.extras.automation[0].target,
        "section.drums.hat.level.gain_db"
    );
    audio(&session);
}

#[test]
fn physical_device_ids_take_precedence_over_shorter_logical_prefixes() {
    let source = song(r#"automation("drums.kick.level.gain_db",curve([[0b,-6]]))"#)
        .replace("fx(\"lowpass\")", "fx(\"lowpass\",{id:\"kick\"})");
    let session = compile(&source).unwrap();
    assert_eq!(session.extras.automation.len(), 1);
    assert_eq!(
        session.extras.automation[0].target,
        "drums.kick.level.gain_db"
    );
    audio(&session);
}

#[test]
fn physical_routes_are_not_captured_by_logical_insert_prefixes() {
    let source = song(r#"automation("drums.kick.send.room",curve([[0b,-6]]))"#)
        .replace("fx(\"lowpass\")", "fx(\"lowpass\",{id:\"kick\"})");
    let session = compile(&source).unwrap();
    assert_eq!(session.extras.automation.len(), 1);
    assert_eq!(session.extras.automation[0].target, "drums.kick.send.room");
    audio(&session);
}

#[test]
fn a_declared_kit_voice_keys_a_sidechain_while_the_part_rests() {
    for part in [r#"drums({snare:"..x."})"#, "rest(4b)"] {
        let session = compile(&format!(
            r#"song({{
            tracks:[
                track("drums",{part},kit("default",{{kick:synth("kick"),snare:synth("snare")}}),{{output:"hall"}}),
                track("pad",phrase("C3:w"),synth("bell"),{{output:"hall"}})
            ],
            buses:[bus("hall",[fx("compressor",{{sidechain:"drums.kick",threshold_db:-30}})])],
            tail:0
        }})"#
        ))
        .unwrap_or_else(|e| panic!("{part}: {e:#}"));
        let kick = session
            .tracks
            .iter()
            .find(|t| t.id.as_str() == "drums.kick")
            .expect("silent kick voice");
        let muz::model::TrackSource::Midi(source) = &kick.source;
        assert_eq!(source.imported.notes.len(), 0);
        assert!(audio(&session).iter().any(|s| s.abs() > 1e-4));
    }
}

#[test]
fn a_kit_choke_follows_its_notes_onto_another_midi_channel() {
    let source = |channel: &str| {
        format!(
            r#"use "std/signal" as s;
fn hit(name, decay) {{
    let e = s.adsr(0.5ms, decay, 0, 5ms, one_shot = true);
    voice_patch(name, {{velocity_track: 0, output: s.mul([s.noise(), e]), lifetime: {{envelope: e, tail: 0s}}}})
}}
let k = merge(kit("k", {{kick: hit("k", 400ms), snare: hit("s", 400ms)}}, defaults = {{}}),
              {{chokes: [["kick", "snare"]]}});
song({{tempo: 120, tail: 0, tracks: [track("n", drums({{kick: "x...", snare: "..x."}}, span = 1b){channel}, k)]}})"#
        )
    };
    let plain = audio(&compile(&source("")).unwrap());
    let drums = audio(&compile(&source(".channel(9)")).unwrap());
    let unchoked = source("").replace("{chokes: [[\"kick\", \"snare\"]]}", "{}");
    let unchoked = audio(&compile(&unchoked).unwrap());
    assert!(plain.iter().any(|x| *x != 0.));
    assert_ne!(plain, unchoked, "the choke cuts the kick");
    assert_eq!(plain, drums, "channel 9 chokes exactly as channel 0 does");
}
