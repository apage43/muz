//! std/woodwinds `play`: a player with lookahead plans breaths and phrases and
//! writes ordinary note data. Synthetic lines check its properties.
use std::io::Read;
use std::path::Path;

fn load(dir: &Path, source: &str) {
    let path = dir.join("player.muz");
    std::fs::write(&path, source).unwrap();
    muz::lang::load(&path).unwrap();
}

fn compile(dir: &Path, source: &str) -> anyhow::Result<muz::compile::Compiled> {
    let path = dir.join("song.muz");
    std::fs::write(&path, source).unwrap();
    muz::compile::compile(&path)
}

#[test]
fn material_no_player_could_play_is_rejected_unless_accepted() {
    let dir = tempfile::tempdir().unwrap();
    // Ten seconds slurred on one flute breath, sixteenths faster than a
    // clarinet can single-tongue, and a note under the oboe's range.
    let song = |options: &str| {
        let checks = if options.contains("strict: false") {
            ", {checks: [w.playable()]}"
        } else {
            ""
        };
        format!(
            r#"use "std/woodwinds" as w;
            let timing = {{tempo: 120}};
            let slurred = seq([phrase("D5:w"), phrase("E5:w F5:w G5:w A5:w").map_notes(fn(n) => {{tags: n.tags + ["slur"]}})]);
            let tongued = repeat(phrase("C5:s D5:s"), 8);
            song({{tempo: 120, tracks: [
                track("fl", w.play(slurred, w.specs.flute, merge({{timing: timing}}, {options})), w.flute(){checks}),
                track("cl", w.play(tongued, w.specs.clarinet, merge({{timing: {{tempo: 140}}}}, {options})), w.clarinet()),
                track("ob", w.play(phrase("A3:q C5:q"), w.specs.oboe, merge({{timing: timing}}, {options})), w.oboe())
            ], tail: 0}})"#
        )
    };
    // By default the first part that cannot be played stops evaluation, and
    // says where, which notes, which limit and what was asked.
    let error = format!("{:#}", compile(dir.path(), &song("{}")).unwrap_err());
    assert!(error.contains("unplayable as written"), "{error}");
    assert!(
        error.contains("flute, bar 1 beat 1 to bar 5 beat 1: the slur from D5 to A5 lasts 10 s and needs a breath")
            && error.contains("a breath lasts about 6.4 s at this dynamic"),
        "{error}"
    );
    let error = format!(
        "{:#}",
        compile(dir.path(), &song(r#"{accept: ["slur_breath"]}"#)).unwrap_err()
    );
    assert!(
        error.contains("clarinet, bar 1 beat 1 to bar 1 beat 4.75: 16 tongued notes at 9.3 a second; single tonguing reaches about 8"),
        "{error}"
    );
    // Accepting every finding plays all three: the composer's call.
    compile(dir.path(), &song("{accept: true}")).unwrap();
    // Not strict: the best plan, and the findings as warnings of a track check.
    let c = compile(dir.path(), &song("{strict: false}")).unwrap();
    let found: Vec<_> = c
        .diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.track.as_str(), d.beat))
        .collect();
    assert_eq!(found, [("check.woodwinds.slur_breath", "fl", 0.0)]);
    // Per note: a `superhuman` tag (or `data.play.accept`) accepts what it touches.
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        let fast = repeat(phrase("C5:s D5:s"), 8);
        let tagged = fast.map_notes(fn(n) => {tags: n.tags + ["superhuman"]});
        let found = w.unplayable(w.play(tagged, w.specs.clarinet, {timing: {tempo: 140}}));
        assert(len(found) == 1 && found[0].accepted, str(found));
        let low = note("A3", 1b).map_notes(fn(n) => {data: {play: {accept: ["range"]}}});
        let under = w.unplayable(w.play(low, w.specs.oboe, {}));
        assert(len(under) == 1 && under[0].accepted && under[0].code == "range", str(under));
        // Limits are the player's, open to change.
        w.play(fast, w.specs.clarinet, {timing: {tempo: 140}, player: {limits: {tongue: 10}}});
        1"#,
    );
}

#[test]
fn a_breath_lands_before_the_air_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        fn ordered(p) = sort_by(p.notes, fn(n) => n.at);
        fn breathed(ns) = filter(ns, fn(n) => n.data.plan.breath != null);
        // Forty seconds of quarter notes without a rest: a flute (9 s of air)
        // must breathe several times, never overdrawing a breath.
        let line = repeat(phrase("D5:q E5:q F5:q G5:q A5:q G5:q F5:q E5:q"), 5);
        let ns = ordered(w.play(line, w.specs.flute, {timing: {tempo: 60}}));
        let bs = breathed(ns);
        assert(len(bs) >= 4, "breaths: " + str(len(bs)));
        assert(len(filter(ns, fn(n) => n.data.plan.air < 0)) == 0, "overdrawn");
        // The first breath comes before nine seconds of mezzo-forte air are spent.
        assert(bs[0].at < 9b, "first breath at " + str(bs[0].at));
        // A breath in a running line takes its time from the note before it.
        assert(bs[0].release_offset < 0ms - 200ms, "breath time " + str(bs[0].release_offset));
        // Each phrase starts after a breath.
        assert(ns[len(ns) - 1].data.plan.phrase == len(bs), "phrases follow breaths");
        // A phrase mark within reach is where the breath goes; a slur is not broken.
        let long = phrase("C5:q D5:q E5:q F5:q G5:q A5:q G5:q F5:q@phrase E5:q D5:q@slur C5:q@slur D5:q@slur E5:q@slur F5:q@slur E5:q@slur D5:w");
        let at = map(breathed(ordered(w.play(long, w.specs.clarinet, {timing: {tempo: 60}}))), fn(n) => n.at);
        assert(at == [7b], "breaths at " + str(at));
        // A written breath is always taken; `no_breath` and `data.play.breath` override.
        let written = breathed(ordered(w.play(phrase("C5:q D5:q@breath E5:q F5:q"), w.specs.clarinet, {})));
        assert(len(written) == 1 && written[0].at == 1b, "breath mark");
        // An oboe hardly spends its air but renews it before it goes stale (15 s).
        let held = ordered(w.play(repeat(phrase("C5:h D5:h E5:h F5:h"), 6), w.specs.oboe, {timing: {tempo: 60}}));
        let ob = map(breathed(held), fn(n) => n.at);
        assert(len(ob) >= 3, "oboe breaths " + str(ob));
        assert(fold(range(1, len(ob)), ob[0], fn(m, i) => max(m, ob[i] - ob[i - 1])) <= 15b, "stale air " + str(ob));
        assert(len(filter(held, fn(n) => n.data.plan.air < 0)) == 0, "oboe overdrawn");
        1"#,
    );
}

#[test]
fn timing_drifts_rather_than_jitters() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        // Lag-one autocorrelation of the attack offsets.
        fn r1(xs) {
            let n = len(xs);
            let mean = fold(xs, 0, fn(s, x) => s + x) / n;
            let d = map(xs, fn(x) => x - mean);
            fold(range(1, n), 0, fn(s, i) => s + d[i] * d[i - 1]) / fold(d, 0, fn(s, x) => s + x * x)
        }
        fn offsets(p) = map(sort_by(p.notes, fn(n) => n.at), fn(n) => n.offset / 1ms);
        // Twelve seconds of even eighths: no breath needed, phrase shaping off.
        let line = repeat(phrase("C4:e D4:e E4:e F4:e G4:e F4:e E4:e D4:e"), 6);
        let flat = {timing: {tempo: 120}, final: 0, hold: 0};
        let ours = map(range(4), fn(seed) => r1(offsets(w.play(line, w.specs.clarinet, merge(flat, {seed: seed, ensemble: {seed: seed}})))));
        assert(len(filter(ours, fn(r) => r < 0.6)) == 0, "player autocorrelation " + str(ours));
        // `perform`'s humanize is independent per attack.
        let white = map(range(4), fn(seed) => r1(offsets(w.perform(line, w.specs.clarinet, {timing: {tempo: 120}, seed: seed}))));
        assert(len(filter(white, fn(r) => abs(r) > 0.45)) == 0, "humanize autocorrelation " + str(white));
        // The drift stays a few milliseconds.
        let xs = offsets(w.play(line, w.specs.clarinet, flat));
        assert(len(filter(xs, fn(x) => abs(x) > 40)) == 0, "drift range " + str(xs));
        1"#,
    );
}

#[test]
fn a_phrase_slows_into_its_end_and_holds_its_last_note() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        let still = {timing: {tempo: 60}, drift: 0ms, jitter: 0ms, ensemble: {drift: 0ms}};
        let line = phrase("C5:q D5:q E5:q F5:q E5:q D5:q C5:h r:h");
        let ns = sort_by(w.play(line, w.specs.clarinet, still).notes, fn(n) => n.at);
        let last = ns[6];
        // Its attacks come later and later toward the end.
        assert(ns[0].offset == 0ms && ns[4].offset == 0ms && ns[5].offset > 0ms && last.offset > ns[5].offset, str(map(ns, fn(n) => n.offset)));
        assert(last.offset > 30ms, "final ritard " + str(last.offset));
        // The last note sounds longer than written: release past its gated end.
        let plain = sort_by(w.perform(line, w.specs.clarinet, {timing: {tempo: 60}, humanize: 0ms}).notes, fn(n) => n.at)[6];
        assert(last.gate == plain.gate && last.release_offset > plain.release_offset + 150ms, str([last.release_offset, plain.release_offset]));
        1"#,
    );
}

#[test]
fn a_performance_is_seed_stable_and_parts_share_the_ensemble_drift() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        let line = phrase("D4:q E4:e@slur F4:e@slur G4:h A4:q@staccato G4:q@accent F4:h@niente");
        fn notes(p) = map(sort_by(p.notes, fn(n) => n.at), fn(n) => [n.offset, n.release_offset, n.gate, n.data]);
        let a = notes(w.play(line, w.specs.oboe, {seed: 3}));
        assert(a == notes(w.play(line, w.specs.oboe, {seed: 3})), "same seed, same performance");
        assert(a != notes(w.play(line, w.specs.oboe, {seed: 4})), "another seed varies");
        // Two chairs with no drift of their own move together with the ensemble.
        let own = {drift: 0ms, jitter: 0ms};
        fn at(p) = map(sort_by(p.notes, fn(n) => n.at), fn(n) => n.offset);
        let fl = at(w.play(line, w.specs.flute, merge(own, {seed: 1})));
        let cl = at(w.play(line, w.specs.clarinet, merge(own, {seed: 2})));
        assert(fl == cl, "shared drift");
        assert(fl != at(w.play(line, w.specs.flute, merge(own, {seed: 1, ensemble: {seed: 9}}))), "another ensemble");
        // A chair's lead moves every attack.
        let led = at(w.play(line, w.specs.flute, merge(own, {seed: 1, lead: 0ms - 5ms})));
        assert(len(filter(range(len(fl)), fn(i) => abs((fl[i] - led[i]) / 1ms - 5) > 0.001)) == 0, "lead");
        1"#,
    );
}

#[test]
fn the_player_writes_expression_that_exports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.muz");
    std::fs::write(
        &path,
        r#"use "std/woodwinds" as w;
        let line = phrase("C5:h A5:h@slur G5:q F5:q E5:w");
        let played = w.play(line, w.specs.flute, {timing: {tempo: 60}});
        let ns = sort_by(played.notes, fn(n) => n.at);
        fn x(n) = n.data.expression;
        // Every note: breath as `pressure`, a tongue, tuning with the lip's correction.
        assert(len(filter(ns, fn(n) => x(n).velocity_breath != 0 || kind(x(n).pressure) != "list")) == 0, "pressure");
        assert(x(ns[0]).tongue > 0 && x(ns[1]).tongue == 0, "tongue");
        let t = x(ns[0]).tuning;
        assert(len(t) == 3 && abs(t[2][1]) < abs(t[1][1]), "lip correction " + str(t));
        // The leap up across the flute's break: voicing support, and vibrato on the long notes.
        assert(kind(x(ns[1]).brightness) == "list", "voicing");
        assert(kind(x(ns[4]).vibrato) == "list" && kind(x(ns[4]).vibrato_rate) == "list", "vibrato");
        // A note's own expression wins.
        let mine = sort_by(w.play(note("D5", 2b).express({tuning: 0.1}), w.specs.flute, {}).notes, fn(n) => n.at)[0];
        assert(x(mine).tuning == 0.1, "written expression");
        song({tempo: 60, tracks: [track("fl", played, w.flute())]})"#,
    )
    .unwrap();
    let compiled = muz::compile::compile(&path).unwrap();
    let plan = muz::dawproject::Plan::new(&compiled, "bitwig-linux").unwrap();
    let output = dir.path().join("song.dawproject");
    plan.write(&output, false, None).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
    let mut xml = String::new();
    archive
        .by_name("project.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    for target in ["transpose", "pressure", "timbre"] {
        assert!(
            xml.contains(&format!("<Target expression=\"{target}\"/>")),
            "{target} lane missing"
        );
    }
}
