//! Bounded tonal assistance. Candidate generation is separate from phrase-wide path choice.
use crate::lang::{Diagnostic, suggest_vocabulary};
use crate::music::{self, Beat, Pattern, real};
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
/// Every scoring preference is required; the list doubles as the accepted vocabulary.
const SCORING_KEYS: [&str; 13] = [
    "common_tone",
    "parallel",
    "candidate_center",
    "candidate_spread",
    "center",
    "strong_beat",
    "weak_beat",
    "repeat",
    "fifth",
    "motion",
    "alternative_movement",
    "metrical_period",
    "metrical_tolerance",
];
/// Numeric search preferences are supplied by source; budgets remain engine invariants.
pub struct Scoring(pub BTreeMap<String, f64>);
impl Scoring {
    pub fn new(values: BTreeMap<String, f64>) -> Result<Self> {
        for key in SCORING_KEYS {
            let value = values.get(key).ok_or_else(|| {
                Diagnostic::new(format!("tonal scoring requires '{key}'"))
                    .helps(suggest_vocabulary("scoring keys", key, SCORING_KEYS))
                    .err()
            })?;
            ensure!(
                value.is_finite() && value.abs() <= 1e6,
                "tonal scoring '{key}' must be finite and bounded"
            );
        }
        ensure!(
            values["metrical_period"] > 0. && values["metrical_tolerance"] >= 0.,
            "metrical period must be positive and tolerance nonnegative"
        );
        Ok(Self(values))
    }
    fn get(&self, key: &str) -> f64 {
        self.0[key]
    }
}
pub fn degree(scale: &[f64], degree: i64) -> Result<f64> {
    ensure!(!scale.is_empty(), "scale is empty");
    let n = scale.len() as i64;
    let i = degree - 1;
    Ok(scale[i.rem_euclid(n) as usize] + 12. * i.div_euclid(n) as f64)
}
fn groups(p: &Pattern) -> Vec<(Beat, Vec<usize>)> {
    let mut g = BTreeMap::<Beat, Vec<usize>>::new();
    for (i, n) in p.notes.iter().enumerate() {
        g.entry(n.at).or_default().push(i);
    }
    g.into_iter().collect()
}
fn movement(a: &[f64], b: &[f64], scoring: &Scoring) -> f64 {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort_by(f64::total_cmp);
    b.sort_by(f64::total_cmp);
    let mut cost = 0.;
    for (i, x) in b.iter().enumerate() {
        let y = a[i.min(a.len() - 1)];
        cost += (x - y).abs();
        if a.iter().any(|z| (*z - *x).abs() < 1e-6) {
            cost += scoring.get("common_tone");
        }
    }
    for i in 0..b.len().min(a.len()) {
        for j in i + 1..b.len().min(a.len()) {
            let before = (a[j] - a[i]).rem_euclid(12.);
            let after = (b[j] - b[i]).rem_euclid(12.);
            if (before - after).abs() < 1e-6
                && (after == 0. || after == 7.)
                && (b[i] - a[i]) * (b[j] - a[j]) > 0.
            {
                cost += scoring.get("parallel");
            }
        }
    }
    cost
}
/// Enumerate octave placements/inversions, then find the least-cost path across the whole phrase.
pub fn voicelead(
    p: &Pattern,
    low: f64,
    high: f64,
    center: f64,
    scoring: &Scoring,
) -> Result<Pattern> {
    ensure!(
        low >= 0. && high <= 127. && high - low >= 12. && high - low <= 48.,
        "voice-leading register must span 12..48 semitones within 0..127"
    );
    let groups = groups(p);
    ensure!(
        groups.len() <= 512,
        "voicelead accepts at most 512 harmony attacks per call"
    );
    if groups.is_empty() {
        return Ok(p.clone());
    }
    let mut candidates = Vec::<Vec<Vec<f64>>>::new();
    for (at, indices) in &groups {
        ensure!(
            indices.len() <= 8,
            "voicelead supports at most eight voices per harmony attack"
        );
        let mut rows = vec![Vec::new()];
        for &i in indices {
            let n = &p.notes[i];
            let pitches = if n.tags.contains("fixed")
                || n.data.get("anchor") == Some(&serde_json::json!(true))
            {
                vec![n.pitch]
            } else {
                (0..11)
                    .map(|o| n.pitch.rem_euclid(12.) + 12. * o as f64)
                    .filter(|x| *x >= low && *x <= high)
                    .collect::<Vec<_>>()
            };
            let mut next = Vec::new();
            for row in &rows {
                for &pitch in &pitches {
                    if row.contains(&pitch) {
                        continue;
                    }
                    let mut v = row.clone();
                    v.push(pitch);
                    next.push(v);
                }
            }
            ensure!(
                next.len() <= 65536,
                "voice-leading candidate budget exceeded; narrow the register or anchor voices"
            );
            rows = next;
        }
        let local = |v: &Vec<f64>| {
            scoring.get("candidate_center")
                * (v.iter().sum::<f64>() / v.len() as f64 - center).abs()
                + scoring.get("candidate_spread")
                    * (v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                        - v.iter().copied().fold(f64::INFINITY, f64::min))
        };
        rows.sort_by(|a, b| local(a).total_cmp(&local(b)));
        // Deduplicate equivalent permutations; preserve the note-to-pitch assignment in the winner.
        let mut seen = std::collections::BTreeSet::new();
        rows.retain(|v| {
            let mut key = v
                .iter()
                .map(|x| (*x * 1000.).round() as i32)
                .collect::<Vec<_>>();
            key.sort();
            seen.insert(key)
        });
        rows.truncate(64);
        ensure!(
            !rows.is_empty(),
            "no voicing at beat {} in requested register",
            real(*at)
        );
        candidates.push(rows);
    }
    let mut costs = vec![0.; candidates[0].len()];
    let mut parents = Vec::<Vec<usize>>::new();
    for (g, rows) in candidates.iter().enumerate() {
        let local = |v: &Vec<f64>| {
            scoring.get("center") * (v.iter().sum::<f64>() / v.len() as f64 - center).abs()
        };
        if g == 0 {
            costs = rows.iter().map(local).collect();
            continue;
        }
        let mut next = Vec::new();
        let mut prev = Vec::new();
        for row in rows {
            let (index, cost) = candidates[g - 1]
                .iter()
                .enumerate()
                .map(|(i, p)| (i, costs[i] + movement(p, row, scoring)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            next.push(cost + local(row));
            prev.push(index);
        }
        costs = next;
        parents.push(prev);
    }
    let mut index = costs
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let mut out = p.clone();
    for g in (0..groups.len()).rev() {
        for (&i, &pitch) in groups[g].1.iter().zip(&candidates[g][index]) {
            out.notes[i].pitch = pitch;
        }
        if g > 0 {
            index = parents[g - 1][index];
        }
    }
    Ok(out)
}
/// Choose harmony against sustained/metrical melody notes, with continuity across chord slots.
pub fn reharmonize(
    h: &Pattern,
    melody: &Pattern,
    palette: &[String],
    octave: i32,
    scoring: &Scoring,
) -> Result<Pattern> {
    ensure!(
        !palette.is_empty() && palette.len() <= 32,
        "reharmonize needs 1..32 candidate chord symbols"
    );
    let groups = groups(h);
    ensure!(
        !groups.is_empty() && groups.len() <= 512,
        "reharmonize needs 1..512 harmony slots"
    );
    let mut symbols = palette.to_vec();
    for (_, g) in &groups {
        if let Some(s) = h.notes[g[0]].data.get("chord").and_then(|v| v.as_str())
            && !symbols.iter().any(|x| x == s)
        {
            symbols.push(s.into());
        }
    }
    let mut chords = symbols
        .iter()
        .map(|s| music::chord(s, octave))
        .collect::<Result<Vec<_>>>()?;
    let mut literal = BTreeMap::new();
    for (g, (_, indices)) in groups.iter().enumerate() {
        if h.notes[indices[0]]
            .data
            .get("chord")
            .and_then(|v| v.as_str())
            .is_none()
        {
            let pitches = indices
                .iter()
                .map(|&i| h.notes[i].pitch)
                .collect::<Vec<_>>();
            let index = chords
                .iter()
                .position(|v| *v == pitches)
                .unwrap_or_else(|| {
                    let index = chords.len();
                    chords.push(pitches);
                    symbols.push(String::new());
                    index
                });
            literal.insert(g, index);
        }
    }
    ensure!(
        chords.len() <= 64,
        "reharmonization accepts at most 64 distinct candidate/original harmonies"
    );
    let mut costs = vec![0.; chords.len()];
    let mut parents = Vec::new();
    for (g, (at, indices)) in groups.iter().enumerate() {
        let end = groups.get(g + 1).map_or(h.span, |v| v.0);
        let anchor = indices.iter().any(|&i| {
            h.notes[i].tags.contains("fixed")
                || h.notes[i].data.get("anchor") == Some(&serde_json::json!(true))
        });
        let mut next = Vec::new();
        let mut prev = Vec::new();
        for (j, chord) in chords.iter().enumerate() {
            let mut cost = 0.;
            if anchor
                && if let Some(symbol) = h.notes[indices[0]]
                    .data
                    .get("chord")
                    .and_then(|v| v.as_str())
                {
                    symbol != symbols[j]
                } else {
                    literal.get(&g) != Some(&j)
                }
            {
                cost = f64::INFINITY;
            }
            for n in &melody.notes {
                let overlap = real((n.at + n.dur).min(end) - n.at.max(*at)).max(0.);
                if overlap == 0. {
                    continue;
                }
                let distance = chord
                    .iter()
                    .map(|x| {
                        let d = (n.pitch - x).rem_euclid(12.);
                        d.min(12. - d)
                    })
                    .fold(f64::INFINITY, f64::min);
                let metrical = if real(n.at).rem_euclid(scoring.get("metrical_period"))
                    < scoring.get("metrical_tolerance")
                {
                    scoring.get("strong_beat")
                } else {
                    scoring.get("weak_beat")
                };
                cost += distance * overlap * metrical * n.velocity;
            }
            let (index, transition) = if g == 0 {
                (0, 0.)
            } else {
                chords
                    .iter()
                    .enumerate()
                    .map(|(i, previous)| {
                        let d = (chord[0] - previous[0]).rem_euclid(12.);
                        let motion = d.min(12. - d);
                        (
                            i,
                            costs[i]
                                + if i == j {
                                    scoring.get("repeat")
                                } else if motion == 5. {
                                    scoring.get("fifth")
                                } else {
                                    motion * scoring.get("motion")
                                },
                        )
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap()
            };
            next.push(cost + transition);
            prev.push(index);
        }
        costs = next;
        parents.push(prev);
    }
    let mut index = costs
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    ensure!(
        costs[index].is_finite(),
        "reharmonization anchors leave no candidate path"
    );
    let mut chosen = vec![0; groups.len()];
    for g in (0..groups.len()).rev() {
        chosen[g] = index;
        index = parents[g][index];
    }
    let mut out = h.clone();
    out.notes.clear();
    for (g, (at, indices)) in groups.iter().enumerate() {
        let end = groups.get(g + 1).map_or(h.span, |v| v.0);
        for (j, pitch) in chords[chosen[g]].iter().enumerate() {
            let mut n = h.notes[indices[j.min(indices.len() - 1)]].clone();
            n.pitch = *pitch;
            n.at = *at;
            n.dur = end - *at;
            n.key = format!("reharm{g}.{j}");
            if symbols[chosen[g]].is_empty() {
                n.data.remove("chord");
            } else {
                n.data
                    .insert("chord".into(), serde_json::json!(symbols[chosen[g]]));
            }
            out.notes.push(n);
        }
    }
    Ok(out)
}

/// Rank distinct alternatives by varying the first unanchored slot. This is a small audition
/// set, not an exhaustive k-best solver. Every alternative still optimizes the remaining phrase.
pub fn alternatives(
    h: &Pattern,
    melody: &Pattern,
    palette: &[String],
    octave: i32,
    count: usize,
    scoring: &Scoring,
) -> Result<Vec<(Pattern, f64)>> {
    ensure!(
        (1..=5).contains(&count),
        "request 1..5 reharmonization alternatives"
    );
    let slots = groups(h);
    let slot = slots.iter().find(|(_, g)| {
        !g.iter().any(|&i| {
            h.notes[i].tags.contains("fixed")
                || h.notes[i].data.get("anchor") == Some(&serde_json::json!(true))
        })
    });
    let Some((at, indices)) = slot else {
        return Ok(vec![(
            reharmonize(h, melody, palette, octave, scoring)?,
            0.,
        )]);
    };
    let mut options = Vec::new();
    for symbol in palette {
        let mut source = h.clone();
        for &i in indices {
            source.notes[i].tags.insert("fixed".into());
            source.notes[i]
                .data
                .insert("chord".into(), serde_json::json!(symbol));
        }
        let mut p = reharmonize(&source, melody, palette, octave, scoring)?;
        for n in &mut p.notes {
            if n.at == *at {
                n.tags.remove("fixed");
            }
        }
        let mut score = 0.;
        let mut previous: Option<Vec<f64>> = None;
        for (at, g) in groups(&p) {
            let notes = g.iter().map(|&i| p.notes[i].pitch).collect::<Vec<_>>();
            let end = at + p.notes[g[0]].dur;
            for n in &melody.notes {
                let duration = real((n.at + n.dur).min(end) - n.at.max(at)).max(0.);
                let distance = notes
                    .iter()
                    .map(|x| {
                        let d = (n.pitch - x).rem_euclid(12.);
                        d.min(12. - d)
                    })
                    .fold(f64::INFINITY, f64::min);
                score += distance * duration * n.velocity;
            }
            if let Some(prev) = &previous {
                score += scoring.get("alternative_movement") * movement(prev, &notes, scoring)
            }
            previous = Some(notes);
        }
        options.push((p, score));
    }
    options.sort_by(|a, b| a.1.total_cmp(&b.1));
    options.truncate(count);
    Ok(options)
}
