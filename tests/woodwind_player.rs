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
        let line = phrase("C5:q D5:q E5:q F5:q C5:h. r:q D5:w");
        let ns = sort_by(w.play(line, w.specs.clarinet, still).notes, fn(n) => n.at);
        let last = ns[4];
        // Its attacks come later and later toward the end.
        assert(ns[0].offset == 0ms && ns[2].offset == 0ms && ns[3].offset > 0ms && last.offset > ns[3].offset, str(map(ns, fn(n) => n.offset)));
        assert(last.offset > 30ms, "final ritard " + str(last.offset));
        // The last note sounds longer than written: released into the rest,
        // past its gated end, by at most `hold_max`.
        let plain = sort_by(w.perform(line, w.specs.clarinet, {timing: {tempo: 60}, humanize: 0ms}).notes, fn(n) => n.at)[4];
        let held = last.release_offset - plain.release_offset;
        assert(last.gate == plain.gate && held > 150ms && held < 301ms, str([last.release_offset, plain.release_offset]));
        1"#,
    );
}

#[test]
fn a_held_last_note_never_crosses_the_next_entry_or_bar_line() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        let timing = {tempo: 60};
        // When each note's sound starts and ends, in seconds as played.
        fn spans(p) = map(sort_by(p.notes, fn(n) => n.at), fn(n) => {
            at: n.at, offset: n.offset / 1s, start: seconds_at(n.at, timing) / 1s + n.offset / 1s,
            end: seconds_at(n.at + n.duration * n.gate, timing) / 1s + n.offset / 1s + n.release_offset / 1s
        });
        fn check(p, bar) {
            let xs = spans(p);
            // Every note ends before the part's next entry ...
            let crossing = filter(range(len(xs) - 1), fn(i) => xs[i].end > xs[i + 1].start);
            assert(len(crossing) == 0, "crosses the next entry: " + str(xs));
            // ... and one ending at the bar line is not held past it (beyond its
            // own late attack in the final slowing).
            let over = filter(xs, fn(x) => x.at < bar * 1b && x.end > bar + max(0, x.offset) + 0.0001);
            assert(len(over) == 0, "held past the bar line: " + str(xs));
            xs
        }
        let still = {timing: timing, drift: 0ms, jitter: 0ms, ensemble: {drift: 0ms}};
        // A chorale's long last chord ends on the bar line: before another
        // entry a rest later, a short rest later, and with no entry at all.
        check(w.play(phrase("C5:h D5:h E5:w r:h G5:h"), w.specs.oboe, still), 8);
        check(w.play(phrase("C5:h D5:h E5:w r:e G5:q."), w.specs.oboe, still), 8);
        check(w.play(phrase("C5:h D5:h E5:w"), w.specs.oboe, still), 8);
        // With the ensemble's drift and the chair's own too.
        check(w.play(phrase("C5:h D5:h E5:w r:q A4:q@phrase B4:h"), w.specs.clarinet, {timing: timing, seed: 5}), 8);
        // Before a rest into a new section (the part waits past the bar line)
        // the hold is shorter than before a rest within the bar.
        let within = spans(w.play(phrase("C5:q D5:q E5:h. r:q F5:w"), w.specs.flute, still));
        let across = spans(w.play(phrase("C5:q D5:q E5:h. r:q r:w F5:w"), w.specs.flute, still));
        assert(across[2].end < within[2].end, str([within[2], across[2]]));
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

#[test]
fn staccato_spends_air_only_while_it_sounds() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        fn breaths(p) = filter(sort_by(p.notes, fn(n) => n.at), fn(n) => n.data.plan.breath != null);
        // Twelve bars of eighths at 162: 18 s written, more than a bassoon's
        // breath (16 s) when slurred or tongued legato, but half of it silence
        // when staccato.
        let jig = repeat(phrase("C3:e@staccato E3:e@staccato G3:e@staccato C4:e@staccato"), 24);
        let detached = w.play(jig, w.specs.bassoon, {timing: {tempo: 162}});
        assert(len(breaths(detached)) == 0 && len(w.unplayable(detached)) == 0, str(w.unplayable(detached)));
        let legato = w.play(jig.map_notes(fn(n) => {tags: []}), w.specs.bassoon, {timing: {tempo: 162}, strict: false});
        assert(len(breaths(legato)) >= 1, "legato breathes");
        // Where a staccato line does breathe, the silent half of the note is
        // breath time, besides what the breath takes from the sounding half
        // and the next entry's lateness (at most 70 ms).
        let long = repeat(phrase("C3:e@staccato E3:e@staccato G3:e@staccato C4:e@staccato"), 48);
        let bs = breaths(w.play(long, w.specs.bassoon, {timing: {tempo: 162}}));
        let eighth = 60 / 162 / 2;
        assert(len(bs) >= 1 && len(filter(bs, fn(n) => n.data.plan.breath.seconds <= eighth * 0.5 + 0.07)) == 0,
            str(map(bs, fn(n) => n.data.plan.breath)));
        1"#,
    );
}

#[test]
fn an_oboe_breathes_at_a_quarter_note_join_at_crotchet_72() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        // A chorale voice in quarter notes, forty seconds without a rest: every
        // place to breathe is a join of two quarter notes (0.83 s each).
        let voice = repeat(phrase("C5:q D5:q E5:q F5:q G5:q F5:q E5:q D5:q"), 6);
        let played = w.play(voice, w.specs.oboe, {timing: {tempo: 72}});
        assert(len(w.unplayable(played)) == 0, str(w.unplayable(played)));
        let bs = map(filter(sort_by(played.notes, fn(n) => n.at), fn(n) => n.data.plan.breath != null), fn(n) => [n.at / 1b, n.data.plan.breath]);
        // Quick exhale-and-sip breaths: partial, each renewing most of the stale
        // air, and often enough that the air never goes stale (15 s).
        assert(len(bs) >= 3, str(bs));
        assert(len(filter(bs, fn(b) => b[1].refill < 0.5 || b[1].refill >= 1 || b[1].why != "stale")) == 0, str(bs));
        let at = map(bs, fn(b) => b[0] * 60 / 72);
        assert(fold(range(1, len(at)), at[0], fn(m, i) => max(m, at[i] - at[i - 1])) < 15, str(at));
        1"#,
    );
}

#[test]
fn findings_name_the_limit_that_binds_with_precision_to_show_it() {
    let dir = tempfile::tempdir().unwrap();
    load(
        dir.path(),
        r#"use "std/woodwinds" as w;
        fn messages(p) = map(w.unplayable(p), fn(x) => x.message);
        // An oboe slurred for 52 s: the air would last 45 s, but held air goes
        // stale after 15 s (13.5 s planned), and that is what the finding says.
        let slurred = seq([phrase("D5:w"), repeat(phrase("E5:w F5:w"), 6).map_notes(fn(n) => {tags: n.tags + ["slur"]})]);
        let ob = messages(w.play(slurred, w.specs.oboe, {timing: {tempo: 60}, strict: false}));
        assert(len(ob) == 1 && contains(ob, "oboe, bar 1 beat 1 to bar 13 beat 1: the slur from D5 to F5 lasts 52 s and needs 4 breaths, the first after 12 s, between F5 and E5; held air goes stale after about 13.5 s"), str(ob));
        // Bassoon sixteenths at 144, too fast for a useful breath: the times are
        // given to as many decimals as tell them apart.
        let fast = repeat(phrase("C3:s D3:s E3:s F3:s"), 32);
        let bsn = messages(w.play(fast, w.specs.bassoon, {timing: {tempo: 144}, strict: false, player: {limits: {double: 0, tongue: 20}}}));
        assert(bsn == ["bassoon, bar 8 beat 2.75 to bar 8 beat 3: a breath with only 0.11 s of time, between F3 and C3 after 12.5 s of playing; a useful breath here needs 0.13 s (0.25 of a full one, which would take 0.5 s)"], str(bsn));
        1"#,
    );
}
