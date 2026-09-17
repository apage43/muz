//! Small, explicit performance policies. Search results are model advice, not physical proofs.
use crate::lang::Diagnostic;
use crate::music::{Pattern, real};

/// Search preferences are supplied by source; this type only validates their shape.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub hand_centers: [f64; 2],
    pub finger_positions: [[f64; 5]; 2],
    pub reach_cost: f64,
    pub capacity_cost: f64,
    pub hand_motion_cost: f64,
    pub hand_span_cost: f64,
    pub finger_motion_cost: f64,
    pub finger_center_cost: f64,
    pub finger_pitch_costs: [[f64; 12]; 5],
}
impl Preferences {
    pub fn parse(value: &crate::lang::Value) -> anyhow::Result<Self> {
        fn scalar_numbers(value: &crate::lang::Value) -> bool {
            use crate::lang::{Unit, Value};
            match value {
                Value::Num(n) => n.unit == Unit::Scalar,
                Value::Array(values) => values.iter().all(scalar_numbers),
                Value::Record(values) => values.values().all(scalar_numbers),
                _ => true,
            }
        }
        anyhow::ensure!(
            scalar_numbers(value),
            "piano preferences require scalar numbers"
        );
        let preferences: Self = serde_json::from_value(value.json()).map_err(|e| {
            Diagnostic::new(format!("invalid piano playing preferences: {e}"))
                .help(concat!(
                    "preference fields: hand_centers, finger_positions, reach_cost, ",
                    "capacity_cost, hand_motion_cost, hand_span_cost, finger_motion_cost, ",
                    "finger_center_cost, finger_pitch_costs"
                ))
                .err()
        })?;
        anyhow::ensure!(
            preferences
                .hand_centers
                .iter()
                .chain(preferences.finger_positions.iter().flatten())
                .all(|x| x.is_finite() && (0.0..=127.).contains(x)),
            "piano initial positions must be finite MIDI pitches 0..127"
        );
        let weights = [
            preferences.reach_cost,
            preferences.capacity_cost,
            preferences.hand_motion_cost,
            preferences.hand_span_cost,
            preferences.finger_motion_cost,
            preferences.finger_center_cost,
        ];
        anyhow::ensure!(
            weights
                .iter()
                .chain(preferences.finger_pitch_costs.iter().flatten())
                .all(|x| x.is_finite() && (0.0..=1_000_000.).contains(x)),
            "piano preference costs must be finite values 0..1000000"
        );
        Ok(preferences)
    }
}

/// Choose a contiguous division of each attack group while respecting declared hands.
/// Register is a soft initial preference; either hand may cross middle C.
pub fn hands(p: &mut Pattern, reach: f64, preferences: &Preferences) {
    let mut order = (0..p.notes.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        p.notes[a]
            .at
            .cmp(&p.notes[b].at)
            .then(p.notes[a].pitch.total_cmp(&p.notes[b].pitch))
    });
    let mut centers = preferences.hand_centers;
    let mut held: Vec<usize> = Vec::new();
    let mut cursor = 0;
    while cursor < order.len() {
        let at = p.notes[order[cursor]].at;
        let start = cursor;
        while cursor < order.len() && p.notes[order[cursor]].at == at {
            cursor += 1;
        }
        held.retain(|&i| {
            real(p.notes[i].at) + real(p.notes[i].dur) * p.notes[i].gate > real(at) + 1e-8
        });
        let group = &order[start..cursor];
        let free = group
            .iter()
            .copied()
            .filter(|&i| p.notes[i].hand.is_none())
            .collect::<Vec<_>>();
        let mut best = (f64::INFINITY, 0);
        for split in 0..=free.len() {
            let mut keys = [Vec::new(), Vec::new()];
            for &i in held
                .iter()
                .chain(group.iter().filter(|&&i| p.notes[i].hand.is_some()))
            {
                let h = usize::from(p.notes[i].hand.as_deref() == Some("right"));
                keys[h].push(p.notes[i].pitch);
            }
            for (j, &i) in free.iter().enumerate() {
                keys[usize::from(j >= split)].push(p.notes[i].pitch);
            }
            let mut cost = 0.;
            for (h, center) in centers.iter().enumerate() {
                keys[h].sort_by(f64::total_cmp);
                keys[h].dedup();
                if keys[h].is_empty() {
                    continue;
                }
                let low = keys[h][0];
                let high = *keys[h].last().unwrap();
                let mid = (low + high) / 2.;
                cost += (high - low - reach).max(0.) * preferences.reach_cost
                    + keys[h].len().saturating_sub(5) as f64 * preferences.capacity_cost
                    + (mid - *center).abs() * preferences.hand_motion_cost
                    + preferences.hand_span_cost * (high - low);
            }
            if cost < best.0 {
                best = (cost, split);
            }
        }
        for (j, &i) in free.iter().enumerate() {
            p.notes[i]
                .data
                .insert("hand_auto".into(), serde_json::json!(true));
            p.notes[i].hand = Some(if j < best.1 { "left" } else { "right" }.into());
        }
        for (h, center) in centers.iter_mut().enumerate() {
            let pitches = group
                .iter()
                .filter(|&&i| usize::from(p.notes[i].hand.as_deref() == Some("right")) == h)
                .map(|&i| p.notes[i].pitch)
                .collect::<Vec<_>>();
            if !pitches.is_empty() {
                *center = pitches.iter().sum::<f64>() / pitches.len() as f64;
            }
        }
        held.extend_from_slice(group);
    }
}

/// Bounded phrase search: retain eight alternative hand/finger histories. Explicit hands and
/// `finger` annotations are anchors. Held fingers cannot move to a different key.
pub fn fingers(
    p: &mut Pattern,
    reach: f64,
    times: &[(f64, f64)],
    preferences: &Preferences,
) -> Result<(), (f64, String)> {
    #[derive(Clone)]
    struct State {
        held: [[Option<usize>; 5]; 2],
        last: [[Option<f64>; 5]; 2],
        cost: f64,
        path: Option<usize>,
    }
    struct Link {
        parent: Option<usize>,
        assign: Vec<(usize, usize, usize)>,
    }
    fn choices(
        notes: &[usize],
        h: usize,
        state: &State,
        p: &Pattern,
        reach: f64,
    ) -> Vec<Vec<(usize, usize, usize)>> {
        #[derive(Clone, Copy)]
        struct Hand<'a> {
            notes: &'a [usize],
            hand: usize,
            pattern: &'a Pattern,
            reach: f64,
        }
        fn recurse(
            pos: usize,
            hand: &Hand<'_>,
            used: &mut [Option<usize>; 5],
            out: &mut Vec<Vec<(usize, usize, usize)>>,
            assign: &mut Vec<(usize, usize, usize)>,
        ) {
            if pos == hand.notes.len() {
                out.push(assign.clone());
                return;
            }
            let i = hand.notes[pos];
            let n = &hand.pattern.notes[i];
            let fixed = n
                .data
                .get("finger")
                .and_then(|v| v.as_f64())
                .map(|v| v as u64);
            for f in 0..5 {
                if used[f].is_some() || fixed.is_some_and(|v| v != f as u64 + 1) {
                    continue;
                }
                let valid = used.iter().enumerate().all(|(g, other)| {
                    let Some(j) = other else { return true };
                    let delta = n.pitch - hand.pattern.notes[*j].pitch;
                    let orientation = if hand.hand == 1 { 1. } else { -1. };
                    let gap = f.abs_diff(g);
                    let limit = match gap {
                        1 => {
                            if f == 0 || g == 0 {
                                7.
                            } else {
                                5.
                            }
                        }
                        2 => 9.,
                        3 => 11.,
                        _ => hand.reach,
                    }
                    .min(hand.reach);
                    delta * orientation * (f as f64 - g as f64) > 0. && delta.abs() <= limit
                });
                if valid {
                    used[f] = Some(i);
                    assign.push((i, hand.hand, f));
                    recurse(pos + 1, hand, used, out, assign);
                    assign.pop();
                    used[f] = None;
                }
            }
        }
        let mut out = Vec::new();
        let mut used = state.held[h];
        if notes.len() + used.iter().flatten().count() > 5 {
            return out;
        }
        let hand = Hand {
            notes,
            hand: h,
            pattern: p,
            reach,
        };
        recurse(0, &hand, &mut used, &mut out, &mut Vec::new());
        out
    }
    for n in &p.notes {
        if let Some(f) = n.data.get("finger")
            && f.as_f64()
                .is_none_or(|v| v.fract() != 0. || !(1.0..=5.).contains(&v))
        {
            return Err((real(n.at), "finger anchor must be an integer 1..5".into()));
        }
    }
    let mut order = (0..p.notes.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        p.notes[a]
            .at
            .cmp(&p.notes[b].at)
            .then(p.notes[a].pitch.total_cmp(&p.notes[b].pitch))
    });
    let mut beam = vec![State {
        held: [[None; 5]; 2],
        last: [[None; 5]; 2],
        cost: 0.,
        path: None,
    }];
    let mut links = Vec::<Link>::new();
    let mut cursor = 0;
    let mut transitions = 0;
    while cursor < order.len() {
        let at = p.notes[order[cursor]].at;
        let begin = cursor;
        while cursor < order.len() && p.notes[order[cursor]].at == at {
            cursor += 1;
        }
        let group = &order[begin..cursor];
        let now = group
            .iter()
            .map(|&i| times[i].0)
            .fold(f64::INFINITY, f64::min);
        let free = group
            .iter()
            .copied()
            .filter(|&i| {
                p.notes[i].hand.is_none()
                    || p.notes[i].data.get("hand_auto") == Some(&serde_json::Value::Bool(true))
            })
            .collect::<Vec<_>>();
        let mut candidates = Vec::<(State, Vec<(usize, usize, usize)>)>::new();
        for prev in &beam {
            let mut state = prev.clone();
            for hand in &mut state.held {
                for finger in hand {
                    if finger.is_some_and(|i| times[i].1 <= now + 1e-6) {
                        *finger = None;
                    }
                }
            }
            for split in 0..=free.len() {
                let mut notes = [Vec::new(), Vec::new()];
                for &i in group {
                    let h = if let Some(j) = free.iter().position(|x| *x == i) {
                        usize::from(j >= split)
                    } else {
                        usize::from(p.notes[i].hand.as_deref() == Some("right"))
                    };
                    notes[h].push(i);
                }
                let left = choices(&notes[0], 0, &state, p, reach);
                let right = choices(&notes[1], 1, &state, p, reach);
                for l in &left {
                    for r in &right {
                        transitions += 1;
                        if transitions % 1024 == 0 {
                            crate::host::check_cancelled()
                                .map_err(|e| (real(at), e.to_string()))?;
                        }
                        if transitions > 400_000 {
                            return Err((real(at),"fingering search reached its 400000-transition budget; shorten the phrase or anchor hands/fingers".into()));
                        }
                        let mut next = state.clone();
                        let assign = l.iter().chain(r).copied().collect::<Vec<_>>();
                        for &(i, h, f) in &assign {
                            let pitch = p.notes[i].pitch;
                            let target =
                                state.last[h][f].unwrap_or(preferences.finger_positions[h][f]);
                            next.cost += (pitch - target).abs() * preferences.finger_motion_cost;
                            // Carry the hand's recent position even when another finger played it.
                            let center =
                                state.last[h].iter().flatten().copied().collect::<Vec<_>>();
                            if !center.is_empty() {
                                next.cost += (pitch
                                    - center.iter().sum::<f64>() / center.len() as f64)
                                    .abs()
                                    * preferences.finger_center_cost;
                            }
                            next.cost += preferences.finger_pitch_costs[f]
                                [(pitch.round() as i32).rem_euclid(12) as usize];
                            next.held[h][f] = Some(i);
                            next.last[h][f] = Some(pitch);
                        }
                        candidates.push((next, assign));
                    }
                }
            }
        }
        if candidates.is_empty() {
            return Err((real(at),"bounded fingering search found no allocation with the held keys, hand/finger anchors and configured reach; redistribute, release earlier, roll the chord or loosen the model".into()));
        }
        candidates.sort_by(|a, b| a.0.cost.total_cmp(&b.0.cost));
        // Cheap alternatives for a short accompaniment attack must not crowd out a different
        // finger on a long held melody. Preserve those distinct obligations in the beam.
        let next = order.get(cursor).map_or(f64::INFINITY, |&i| times[i].0);
        let mut seen = std::collections::BTreeSet::new();
        let mut selected = std::collections::BTreeSet::new();
        for (i, (s, _)) in candidates.iter().enumerate() {
            let signature = s
                .held
                .iter()
                .enumerate()
                .flat_map(|(h, fs)| {
                    fs.iter().enumerate().filter_map(move |(f, n)| {
                        n.filter(|n| times[*n].1 > next + 1e-6).map(|n| (h, f, n))
                    })
                })
                .collect::<Vec<_>>();
            if seen.insert(signature) {
                selected.insert(i);
                if selected.len() == 8 {
                    break;
                }
            }
        }
        for i in 0..candidates.len() {
            if selected.len() == 8 {
                break;
            }
            selected.insert(i);
        }
        beam.clear();
        for (i, (mut state, assign)) in candidates.into_iter().enumerate() {
            if !selected.contains(&i) {
                continue;
            }
            let path = links.len();
            links.push(Link {
                parent: state.path,
                assign,
            });
            state.path = Some(path);
            beam.push(state);
        }
    }
    let Some(best) = beam.first() else {
        return Ok(());
    };
    let mut link = best.path;
    while let Some(i) = link {
        for &(note, h, f) in &links[i].assign {
            let n = &mut p.notes[note];
            if n.hand.is_none() {
                n.data.insert("hand_auto".into(), serde_json::json!(true));
            }
            n.hand = Some(if h == 0 { "left" } else { "right" }.into());
            n.data.insert("finger".into(), serde_json::json!(f + 1));
        }
        link = links[i].parent;
    }
    Ok(())
}
