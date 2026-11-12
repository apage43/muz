//! What `muz midi export` writes for a song beyond notes.
use muz::smf::{Document, Kind};

fn export(src: &str) -> Document {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.muz");
    std::fs::write(&path, src).unwrap();
    let compiled = muz::compile::compile(&path).unwrap_or_else(|e| panic!("{e:#}"));
    muz::smf::export(&compiled.session).unwrap()
}

/// Each track's meta events of `tag` as (tick, text).
fn texts(doc: &Document, tag: u8) -> Vec<Vec<(u64, String)>> {
    doc.tracks
        .iter()
        .map(|track| {
            let mut tick = 0u64;
            let mut out = vec![];
            for event in track {
                tick += u64::from(event.delta);
                if let Kind::Meta { tag: t, data } = &event.kind
                    && *t == tag
                {
                    out.push((tick, String::from_utf8(data.clone()).unwrap()));
                }
            }
            out
        })
        .collect()
}

#[test]
fn meta_events_on_an_instrument_track_reach_the_file() {
    // Loop markers derived from the song's own sections, and a lyric.
    let doc = export(
        r#"
let form = [section("intro", 2b), section("loop", 2b)];
let loop_start = form[0].duration;
let loop_end = loop_start + form[1].duration;
let markers = stack([meta(6, "loop start", at = loop_start), meta(6, "loop end", at = loop_end)]);
let lead = stack([phrase("E4:q G4:q B4:q A4:q"), markers, meta(5, "la", at = 1b)]);
song({tempo: 120, sections: form, tracks: [track("Lead", lead, synth("bell"))]})
"#,
    );
    // Markers go to the conductor track, other meta events stay on their track.
    assert_eq!(
        texts(&doc, 6),
        [vec![(1920, "loop start".into()), (3840, "loop end".into())], vec![]]
    );
    assert_eq!(texts(&doc, 5), [vec![], vec![(960, "la".into())]]);
    assert_eq!(texts(&doc, 3)[1], [(0, "Lead".to_string())]);
}

#[test]
fn a_kit_exports_as_its_logical_track_on_drum_notes() {
    let doc = export(
        r#"
let k = kit("k", {kick: synth("kick"), snare: synth("snare"), hat: synth("hat"), open_hat: synth("crash")},
            defaults = {chokes: [["hat", "open_hat"]]});
let d = drums({kick: "X...", snare: "..X.", hat: "x.x.", open_hat: "...x"}, span = 1b);
song({tempo: 120, tracks: [
    track("Noise", stack([d, meta(5, "go", at = 0b)]), k),
    track("Lead", phrase("C4:q"), synth("bell"))
]})
"#,
    );
    let names: Vec<_> = texts(&doc, 3).into_iter().map(|t| t.into_iter().map(|(_, n)| n).collect::<Vec<_>>()).collect();
    assert_eq!(names, [vec![], vec!["Noise".to_string()], vec!["Lead".into()]]);
    let mut tick = 0u64;
    let mut onsets = vec![];
    for event in &doc.tracks[1] {
        tick += u64::from(event.delta);
        if let Kind::Midi { bytes } = &event.kind {
            // A choke is a device mechanism; no CC 120 reaches the file.
            assert_ne!(bytes[0] >> 4, 0xb, "{bytes:?}");
            if bytes[0] >> 4 == 9 {
                onsets.push((tick, bytes[1]));
            }
        }
    }
    onsets.sort();
    assert_eq!(onsets, [(0, 36), (0, 42), (480, 38), (480, 42), (720, 46)]);
    // Raw events every voice carries are written once.
    assert_eq!(texts(&doc, 5)[1], [(0, "go".to_string())]);
}
