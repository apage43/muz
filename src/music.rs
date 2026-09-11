//! Musical values: exact score time, reusable patterns, and lightweight annotations.
use crate::lang::Diagnostic;
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
/// Score operations must never wrap rational intermediates in release builds.
pub(crate) fn checked_time(value: Option<Beat>) -> Result<Beat> {
    value.ok_or_else(|| anyhow::anyhow!("exact score time overflow"))
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
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
    pub fn shifted(&self, offset: Beat, key: &str) -> Result<Self> {
        let mut p = self.clone();
        p.span = checked_time(p.span.checked_add(&offset))?;
        for n in &mut p.notes {
            n.at = checked_time(n.at.checked_add(&offset))?;
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
            bail!("pattern exceeds 200000 notes");
        }
        if self.controls.len() > 200_000 || self.raw.len() > 200_000 {
            bail!("pattern exceeds 200000 control or raw events");
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
        rest.parse::<i32>()
            .map_err(|_| invalid_pitch(s))?
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
        let (body, tag) = t
            .split_once('@')
            .map_or((t.as_str(), None), |(a, b)| (a, Some(b)));
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
                if let Some(t) = tag {
                    n.tags.insert(t.to_owned());
                }
                out.notes.push(n);
            }
        }
        out.span = checked_time(out.span.checked_add(&dur))?;
    }
    out.validate()?;
    Ok(out)
}
pub fn chord(symbol: &str, octave: i32) -> Result<Vec<f64>> {
    let chars: Vec<char> = symbol.chars().collect();
    if chars.is_empty() {
        return Err(Diagnostic::new("empty chord")
            .help("chord symbols look like 'C', 'Dm7' or 'F#m7b5/A'")
            .err());
    }
    let split = if chars.get(1).is_some_and(|c| *c == '#' || *c == 'b') {
        if chars.get(2) == chars.get(1) { 3 } else { 2 }
    } else {
        1
    };
    let root_str: String = chars[..split].iter().collect();
    let mut quality: String = chars[split..].iter().collect();
    let bass = quality.split_once('/').map(|(_, s)| s.to_owned());
    if bass.is_some() {
        quality = quality.split('/').next().unwrap().into();
    }
    let root = pitch(&format!("{root_str}{octave}"))?;
    let mut ints: Vec<i32> = if quality.starts_with("dim") || quality.starts_with('°') {
        vec![0, 3, 6]
    } else if quality.starts_with("aug") || quality.starts_with('+') {
        vec![0, 4, 8]
    } else if quality.starts_with("sus2") {
        vec![0, 2, 7]
    } else if quality.starts_with("sus4") || quality == "sus" {
        vec![0, 5, 7]
    } else if quality == "5" {
        vec![0, 7]
    } else if quality.starts_with('m') && !quality.starts_with("maj") {
        vec![0, 3, 7]
    } else {
        vec![0, 4, 7]
    };
    if quality.contains('7')
        || quality.contains('9')
        || quality.contains("11")
        || quality.contains("13")
    {
        if !quality.contains("add") {
            ints.push(if quality.contains("maj") {
                11
            } else if quality.contains("dim") {
                9
            } else {
                10
            });
        }
    }
    if quality.contains('6') && !quality.contains("16") {
        ints.push(9);
    }
    if quality.contains('9') {
        ints.push(14);
    }
    if quality.contains("11") {
        ints.extend([14, 17]);
    }
    if quality.contains("13") {
        ints.extend([14, 21]);
    }
    if quality.contains("b5") {
        if let Some(x) = ints.iter_mut().find(|x| **x == 7) {
            *x = 6;
        }
    }
    if quality.contains("b9") {
        for x in &mut ints {
            if *x == 14 {
                *x = 13;
            }
        }
    }
    let mut result: Vec<f64> = ints.into_iter().map(|i| root + i as f64).collect();
    if let Some(bass) = bass {
        let mut p = pitch(&format!("{bass}{octave}"))?;
        while p >= result[0] {
            p -= 12.0;
        }
        result.insert(0, p);
    }
    Ok(result)
}
