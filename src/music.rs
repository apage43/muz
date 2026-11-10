//! Musical values: exact score time, reusable patterns, and lightweight annotations.
use crate::diagnostic::Diagnostic;
use anyhow::{Result, bail};
use num_rational::Ratio;
use num_traits::{CheckedAdd, CheckedMul, ToPrimitive};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub type Beat = Ratio<i64>;
pub fn b(n: i64) -> Beat {
    Ratio::from_integer(n)
}
pub fn real(x: Beat) -> f64 {
    x.to_f64().unwrap_or(0.0)
}
pub fn rational(x: f64) -> Result<Beat> {
    Ratio::approximate_float(x)
        .ok_or_else(|| anyhow::anyhow!("number is not finite or is too large"))
}
/// Denominator of the grid that inexact beat values are rounded to:
/// lcm(`PPQ`, 1..=10) = 2^9 * 3^2 * 5^4 * 7. Every tick and every tuplet
/// subdivision up to 10 (and 12, 16, 32, 64, ...) is a grid point.
pub const TIME_GRID: i64 = 20_160_000;
const _: () = assert!(TIME_GRID % crate::compile::PPQ as i64 == 0 && TIME_GRID % 2520 == 0);
/// Converts an inexact (floating-point) beat value to exact score time.
///
/// The value is rounded to the nearest multiple of `1/TIME_GRID` beat, so the
/// error is at most `1/(2*TIME_GRID)` beat (about 2.5e-8 beat, 12 ns at
/// 120 BPM). Floats that already lie on the grid (0.25, 0.5, 0.375, ...) are
/// unchanged, and floats near a simple tuplet fraction (a computed 1/3) become
/// that fraction. The bounded denominator keeps later exact arithmetic with
/// tuplet positions from overflowing, unlike a best rational approximation of
/// every float digit.
pub fn time(x: f64) -> Result<Beat> {
    let ticks = (x * TIME_GRID as f64).round();
    // 2^63 is exactly representable; anything at or beyond it cannot be an i64.
    if !x.is_finite() || ticks.abs() >= 9_223_372_036_854_775_808.0 {
        bail!("beat value {x} is not finite or is too large for exact score time");
    }
    Ok(Ratio::new(ticks as i64, TIME_GRID))
}
/// Score operations must never wrap rational intermediates in release builds.
pub(crate) fn checked_time(value: Option<Beat>) -> Result<Beat> {
    value.ok_or_else(|| anyhow::anyhow!("exact score time overflow"))
}
/// Like `checked_time`, naming the operation and both exact operands.
pub(crate) fn checked_time_op(op: &str, a: Beat, b: Beat, value: Option<Beat>) -> Result<Beat> {
    value.ok_or_else(|| {
        anyhow::anyhow!(
            "exact score time overflow: {a} {op} {b} does not fit a 64-bit rational \
             (exact rationals with large, unrelated denominators, such as long decimal \
             literals; inexact beat values are rounded to 1/{TIME_GRID} beat and cannot cause this alone)"
        )
    })
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    /// Optional compile-time explanation; never enters playback or semantic equality.
    #[serde(skip)]
    pub provenance: crate::provenance::NoteProvenance,
    /// Engine-owned clock timing; annotation keys never activate this payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockPlacement>,
    pub at: Beat,
    pub dur: Beat,
    pub pitch: f64,
    pub velocity: f64,
    pub release: f64,
    pub gate: f64,
    pub key: String,
    pub tags: BTreeSet<String>,
    pub data: BTreeMap<String, serde_json::Value>,
    pub voice: String,
    pub hand: Option<String>,
    pub offset_ms: f64,
    #[serde(default)]
    pub release_offset_ms: f64,
}
impl Note {
    pub fn new(at: Beat, dur: Beat, pitch: f64, key: String) -> Self {
        Self {
            provenance: Default::default(),
            clock: None,
            at,
            dur,
            pitch,
            velocity: 0.72,
            release: 0.3,
            gate: 0.9,
            key,
            tags: BTreeSet::new(),
            data: BTreeMap::new(),
            voice: String::new(),
            hand: None,
            offset_ms: 0.0,
            release_offset_ms: 0.0,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClockPlacement {
    offset_seconds: f64,
    duration_seconds: f64,
    span_seconds: f64,
}
impl ClockPlacement {
    pub fn new(offset_seconds: f64, duration_seconds: f64, span_seconds: f64) -> Result<Self> {
        let value = Self {
            offset_seconds,
            duration_seconds,
            span_seconds,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if !self.offset_seconds.is_finite()
            || self.offset_seconds < 0.
            || !self.duration_seconds.is_finite()
            || self.duration_seconds <= 0.
            || !self.span_seconds.is_finite()
            || self.span_seconds < self.duration_seconds
            || !(self.offset_seconds + self.span_seconds).is_finite()
        {
            bail!("clock timing requires finite nonnegative offset and positive duration <= span");
        }
        Ok(())
    }
    pub fn seconds(self) -> (f64, f64, f64) {
        (
            self.offset_seconds,
            self.duration_seconds,
            self.span_seconds,
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Control {
    #[serde(default)]
    pub offset_ms: f64,
    pub at: Beat,
    pub cc: u8,
    pub value: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    #[serde(default)]
    pub offset_ms: f64,
    pub at: Beat,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    pub span: Beat,
    pub notes: Vec<Note>,
    pub controls: Vec<Control>,
    pub raw: Vec<RawEvent>,
}
impl Default for Pattern {
    fn default() -> Self {
        Self {
            span: b(0),
            notes: vec![],
            controls: vec![],
            raw: vec![],
        }
    }
}
impl Pattern {
    pub fn require_score_time(&self, operation: &str) -> Result<()> {
        if self.notes.iter().any(|n| n.clock.is_some()) {
            bail!("{operation} cannot transform clock clip score duration; use clip trim options");
        }
        Ok(())
    }
    pub fn shifted(&self, offset: Beat, key: &str) -> Result<Self> {
        let mut p = self.clone();
        p.span = checked_time_op("+", p.span, offset, p.span.checked_add(&offset))?;
        for n in &mut p.notes {
            n.at = checked_time_op("+", n.at, offset, n.at.checked_add(&offset))?;
            n.key = format!("{key}/{}", n.key);
        }
        for c in &mut p.controls {
            c.at = checked_time(c.at.checked_add(&offset))?;
        }
        for r in &mut p.raw {
            r.at = checked_time(r.at.checked_add(&offset))?;
        }
        Ok(p)
    }
    pub fn overlay(&mut self, p: Self) {
        self.span = self.span.max(p.span);
        self.notes.extend(p.notes);
        self.controls.extend(p.controls);
        self.raw.extend(p.raw);
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_local()?;
        if self.span < b(0)
            || self.notes.iter().any(|n| n.at < b(0))
            || self.controls.iter().any(|c| c.at < b(0))
            || self.raw.iter().any(|r| r.at < b(0))
        {
            bail!("final pattern contains negative score time");
        }
        Ok(())
    }
    /// Reusable fragments may contain pickups before zero; final score validation is stricter.
    pub fn validate_local(&self) -> Result<()> {
        if self.notes.len() > 200_000 {
            return Err(crate::diagnostic::Diagnostic::new("pattern exceeds 200000 notes").limit().err());
        }
        if self.controls.len() > 200_000 || self.raw.len() > 200_000 {
            return Err(crate::diagnostic::Diagnostic::new("pattern exceeds 200000 control or raw events").limit().err());
        }
        if self
            .controls
            .iter()
            .any(|c| !c.offset_ms.is_finite() || c.cc > 127 || c.value > 127)
            || self.raw.iter().any(|r| !r.offset_ms.is_finite())
        {
            bail!("invalid control/raw event time or MIDI value");
        }
        for n in &self.notes {
            if let Some(clock) = n.clock {
                clock.validate()?;
            }
            if n.dur <= b(0)
                || !n.offset_ms.is_finite()
                || !n.release_offset_ms.is_finite()
                || !n.release.is_finite()
                || !(0.0..=1.0).contains(&n.release)
                || !n.pitch.is_finite()
                || !(0.0..=127.0).contains(&n.pitch)
                || !n.velocity.is_finite()
                || !(0.0..=1.0).contains(&n.velocity)
                || !n.gate.is_finite()
                || n.gate <= 0.0
                || n.hand
                    .as_deref()
                    .is_some_and(|h| !matches!(h, "left" | "right"))
            {
                bail!(
                    "invalid note {}: positive duration/gate, MIDI pitch 0..127 and velocity 0..1 required",
                    n.key
                );
            }
        }
        Ok(())
    }
}
/// Pitch spellings are one small vocabulary; both `pitch` failures point at it.
/// A letter outside A-G is one edit from all seven, so a "did you mean" for the
/// letter itself would always name 'A'; the accepted forms carry the information.
fn invalid_pitch(text: &str) -> anyhow::Error {
    Diagnostic::new(format!("invalid pitch '{text}'"))
        .help(concat!(
            "pitch names look like 'C4', 'F#3' or 'Bb2' ",
            "(letters A-G, optional #/##/b/bb, octave -1..9; MIDI 0..127)"
        ))
        .err()
}
pub fn pitch(text: &str) -> Result<f64> {
    let s = text.trim();
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }
    let mut cs = s.chars();
    let letter = cs
        .next()
        .ok_or_else(|| {
            Diagnostic::new("empty pitch")
                .help("write a pitch like 'C4', 'F#3' or a MIDI number like 60")
                .err()
        })?
        .to_ascii_uppercase();
    let mut pc = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(invalid_pitch(s)),
    };
    let rest = cs.as_str();
    let (rest, alter) = if let Some(r) = rest.strip_prefix("##") {
        (r, 2)
    } else if let Some(r) = rest.strip_prefix("bb") {
        (r, -2)
    } else if let Some(r) = rest.strip_prefix('#') {
        (r, 1)
    } else if let Some(r) = rest.strip_prefix('b') {
        (r, -1)
    } else {
        (rest, 0)
    };
    pc += alter;
    let octave = if rest.is_empty() {
        4
    } else {
        rest.parse::<i32>().map_err(|_| invalid_pitch(s))?
    };
    Ok(((i64::from(octave) + 1) * 12 + pc) as f64)
}
pub fn pitch_name(p: f64) -> String {
    let n = p.round() as i32;
    format!(
        "{}{}",
        [
            "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"
        ][n.rem_euclid(12) as usize],
        n.div_euclid(12) - 1
    )
}
pub fn duration(s: &str) -> Result<Beat> {
    let mut t = s.trim();
    let mut dotted = false;
    if let Some(x) = t.strip_suffix('.') {
        t = x;
        dotted = true;
    }
    let d = match t {
        "w" => b(4),
        "h" => b(2),
        "q" => b(1),
        "e" => Ratio::new(1, 2),
        "s" => Ratio::new(1, 4),
        "t" => Ratio::new(1, 8),
        _ => decimal(t.trim_end_matches('b'))?,
    };
    if dotted {
        checked_time(d.checked_mul(&Ratio::new(3, 2)))
    } else {
        Ok(d)
    }
}
pub fn decimal(s: &str) -> Result<Beat> {
    if let Some((n, d)) = s.split_once('/') {
        let n = n.parse::<i64>()?;
        let d = d.parse::<i64>()?;
        if d == 0 {
            bail!("division by zero");
        }
        return Ok(Ratio::new(n, d));
    }
    if let Some((whole, frac)) = s.split_once('.') {
        let den = 10i64
            .checked_pow(frac.len() as u32)
            .ok_or_else(|| anyhow::anyhow!("decimal too precise"))?;
        let v = whole.parse::<i64>().unwrap_or(0);
        let f = frac.parse::<i64>().unwrap_or(0);
        let num = v
            .checked_mul(den)
            .and_then(|v| v.checked_add(if whole.starts_with('-') { -f } else { f }))
            .ok_or_else(|| anyhow::anyhow!("number overflow"))?;
        Ok(Ratio::new(num, den))
    } else {
        Ok(b(s.parse()?))
    }
}
pub fn phrase(text: &str) -> Result<Pattern> {
    let mut tokens = vec![];
    let mut token = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        if (c.is_whitespace() || c == '|') && depth == 0 {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(c);
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    if depth != 0 {
        return Err(Diagnostic::new("unclosed chord in phrase")
            .help("close every '[' with ']', for example '[C4 E4]:q'")
            .err());
    }
    let mut out = Pattern::default();
    for (i, t) in tokens.iter().enumerate() {
        let mut parts = t.split('@');
        let body = parts.next().unwrap_or_default();
        let tags: Vec<&str> = parts.filter(|tag| !tag.is_empty()).collect();
        let (notes, ds) = body.rsplit_once(':').unwrap_or((body, "q"));
        let dur = duration(ds)?;
        if dur <= b(0) {
            return Err(Diagnostic::new("note duration must be positive")
                .help("write a positive length such as 'q', '1/2' or '2.'")
                .err());
        }
        if notes != "r" && notes != "_" {
            let ps = notes.trim_matches(['[', ']']);
            for (j, p) in ps
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .enumerate()
            {
                let mut n = Note::new(out.span, dur, pitch(p)?, format!("n{i}.{j}"));
                n.tags.extend(tags.iter().map(|tag| (*tag).to_owned()));
                out.notes.push(n);
            }
        }
        out.span = checked_time(out.span.checked_add(&dur))?;
    }
    out.validate()?;
    Ok(out)
}
/// Split a note name (letter plus optional `#`, `##`, `b` or `bb`) off `text`.
fn split_note_name(text: &str) -> Option<(&str, &str)> {
    let first = text.chars().next()?;
    if !matches!(first.to_ascii_uppercase(), 'A'..='G') {
        return None;
    }
    let rest = &text[1..];
    let n = if rest.starts_with("##") || rest.starts_with("bb") {
        2
    } else if rest.starts_with('#') || rest.starts_with('b') {
        1
    } else {
        0
    };
    Some(text.split_at(1 + n))
}
#[derive(Clone, Copy, PartialEq)]
enum ChordFamily {
    Major,
    Minor,
    Diminished,
    HalfDiminished,
    Augmented,
}
const CHORD_HELP: &str = concat!(
    "chord symbols are a root, a quality, alterations and an optional /bass, ",
    "e.g. 'C', 'Dm7', 'F#m7b5/A', 'B7sus4', 'E7#9', 'Cmaj7(#11)', 'C6/9', 'Cadd9/E'; ",
    "see 'Chord symbols' in docs/language.md"
);
/// Parse a chord symbol completely and return its pitches in ascending order:
/// the root in `octave` (or, for a slash chord, the bass strictly below it)
/// first, then the remaining tones upward from the root. The grammar is
/// documented under "Chord symbols" in docs/language.md.
pub fn chord(symbol: &str, octave: i32) -> Result<Vec<f64>> {
    if symbol.is_empty() {
        return Err(Diagnostic::new("empty chord").help(CHORD_HELP).err());
    }
    let unread = |rest: &str| {
        Diagnostic::new(format!("chord '{symbol}': cannot read '{rest}'"))
            .help(CHORD_HELP)
            .err()
    };
    let (root_name, mut q) = split_note_name(symbol).ok_or_else(|| unread(symbol))?;
    let root = pitch(&format!("{root_name}{octave}"))?;
    let eat = |q: &mut &str, prefixes: &[&str]| -> bool {
        for p in prefixes {
            if let Some(r) = q.strip_prefix(p) {
                *q = r;
                return true;
            }
        }
        false
    };
    // Triad family. `maj`/`M`/`Δ` mark the major seventh and are read below.
    let family = if q.starts_with("maj") {
        ChordFamily::Major
    } else if eat(&mut q, &["min", "m", "-"]) {
        ChordFamily::Minor
    } else if eat(&mut q, &["dim", "°", "o"]) {
        ChordFamily::Diminished
    } else if eat(&mut q, &["ø"]) {
        ChordFamily::HalfDiminished
    } else if eat(&mut q, &["aug", "+"]) {
        ChordFamily::Augmented
    } else {
        ChordFamily::Major
    };
    // Sixth, seventh or extension, optionally after a major-seventh marker.
    let marker_at = q;
    let triangle = eat(&mut q, &["Δ", "∆"]);
    let major_seventh = triangle || eat(&mut q, &["maj", "Maj", "M"]);
    let degree_at = q;
    let degree = ["6/9", "69", "13", "11", "9", "7", "6", "5"]
        .into_iter()
        .find(|d| q.starts_with(d));
    if let Some(d) = degree {
        q = &q[d.len()..];
    }
    if major_seventh && family == ChordFamily::HalfDiminished {
        return Err(unread(marker_at));
    }
    let degree = match degree {
        None if triangle || family == ChordFamily::HalfDiminished => Some("7"),
        None if major_seventh && family != ChordFamily::Major => return Err(unread(marker_at)),
        Some("5") if family != ChordFamily::Major || major_seventh => {
            return Err(unread(degree_at));
        }
        Some("6" | "69" | "6/9")
            if family == ChordFamily::HalfDiminished
                || (major_seventh && family != ChordFamily::Major) =>
        {
            return Err(unread(degree_at));
        }
        d => d,
    };
    let mut ints: BTreeSet<i32> = BTreeSet::from([0]);
    let (third, fifth) = match family {
        ChordFamily::Major => (4, 7),
        ChordFamily::Minor => (3, 7),
        ChordFamily::Diminished | ChordFamily::HalfDiminished => (3, 6),
        ChordFamily::Augmented => (4, 8),
    };
    if degree != Some("5") {
        ints.insert(third);
    }
    ints.insert(fifth);
    let seventh = if major_seventh {
        11
    } else if family == ChordFamily::Diminished {
        9
    } else {
        10
    };
    // `maj6`/`M6` read as a plain sixth chord; `13` omits the 11.
    let tones: &[i32] = match degree {
        Some("7") => &[seventh],
        Some("9") => &[seventh, 14],
        Some("11") => &[seventh, 14, 17],
        Some("13") => &[seventh, 14, 21],
        Some("6") => &[9],
        Some("69" | "6/9") => &[9, 14],
        _ => &[],
    };
    ints.extend(tones);
    // Suspensions, alterations and added tones, in any order, with
    // alterations and added tones optionally grouped in parentheses.
    let mut sus = None;
    let mut altered = BTreeSet::new();
    let mut added = BTreeSet::new();
    let alteration = |q: &mut &str| -> Option<i32> {
        [
            ("b5", 6),
            ("#5", 8),
            ("b9", 13),
            ("#9", 15),
            ("#11", 18),
            ("b13", 20),
        ]
        .into_iter()
        .find_map(|(text, i)| q.strip_prefix(text).map(|r| (r, i)))
        .map(|(r, i)| {
            *q = r;
            i
        })
    };
    let added_tone = |q: &mut &str, bare: bool| -> Option<i32> {
        let mut r = *q;
        if !bare {
            r = r.strip_prefix("add")?;
        }
        let shift = if let Some(x) = r.strip_prefix('#') {
            r = x;
            1
        } else if let Some(x) = r.strip_prefix('b') {
            r = x;
            -1
        } else {
            0
        };
        let (text, i) = [
            ("13", 21),
            ("11", 17),
            ("9", 14),
            ("6", 9),
            ("4", 5),
            ("2", 2),
        ]
        .into_iter()
        .find(|(text, _)| r.starts_with(text))?;
        if bare && !matches!(text, "9" | "11" | "13") {
            return None;
        }
        *q = &r[text.len()..];
        Some(i + shift)
    };
    while !q.is_empty() && !q.starts_with('/') {
        let at = q;
        if eat(&mut q, &["sus4", "sus2", "sus"]) {
            let tone = if at.starts_with("sus2") { 2 } else { 5 };
            if sus.is_some() || family != ChordFamily::Major || degree == Some("5") {
                return Err(unread(at));
            }
            sus = Some(tone);
        } else if let Some(i) = alteration(&mut q) {
            altered.insert(i);
        } else if let Some(i) = added_tone(&mut q, false) {
            added.insert(i);
        } else if let Some(group) = q.strip_prefix('(') {
            q = group;
            let unclosed = || {
                Diagnostic::new(format!("chord '{symbol}': unclosed '('"))
                    .help(CHORD_HELP)
                    .err()
            };
            loop {
                if q.is_empty() {
                    return Err(unclosed());
                }
                let item = q;
                if let Some(i) = alteration(&mut q) {
                    altered.insert(i);
                } else if let Some(i) =
                    added_tone(&mut q, false).or_else(|| added_tone(&mut q, true))
                {
                    added.insert(i);
                } else {
                    return Err(unread(item));
                }
                if eat(&mut q, &[")"]) {
                    break;
                }
                if q.is_empty() {
                    return Err(unclosed());
                }
                if !eat(&mut q, &[","]) {
                    return Err(unread(q));
                }
            }
        } else {
            return Err(unread(at));
        }
    }
    if let Some(tone) = sus {
        ints.remove(&4);
        ints.insert(tone);
    }
    for i in &altered {
        let natural = match i {
            6 | 8 => 7,
            13 | 15 => 14,
            18 => 17,
            _ => 21,
        };
        ints.remove(&natural);
    }
    ints.extend(altered);
    ints.extend(added);
    let mut result: Vec<f64> = ints.into_iter().map(|i| root + f64::from(i)).collect();
    if let Some(bass_text) = q.strip_prefix('/') {
        if bass_text.is_empty() {
            return Err(
                Diagnostic::new(format!("chord '{symbol}': missing bass note after '/'"))
                    .help(CHORD_HELP)
                    .err(),
            );
        }
        let (bass_name, rest) = split_note_name(bass_text).ok_or_else(|| unread(bass_text))?;
        if !rest.is_empty() {
            return Err(unread(rest));
        }
        let mut p = pitch(&format!("{bass_name}{octave}"))?;
        while p >= root {
            p -= 12.0;
        }
        result.insert(0, p);
    }
    Ok(result)
}
