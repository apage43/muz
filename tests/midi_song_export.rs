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
