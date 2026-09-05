//! Small, explicit performance policies. Search results are model advice, not physical proofs.
use crate::music::{Pattern, real};

/// Choose a contiguous division of each attack group while respecting declared hands.
/// Register is a soft initial preference; either hand may cross middle C.
pub fn hands(p: &mut Pattern, reach: f64) {
    let mut order = (0..p.notes.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        p.notes[a]
            .at
            .cmp(&p.notes[b].at)
            .then(p.notes[a].pitch.total_cmp(&p.notes[b].pitch))
    });
    let mut centers = [48., 72.];
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
            for h in 0..2 {
                keys[h].sort_by(f64::total_cmp);
                keys[h].dedup();
                if keys[h].is_empty() {
                    continue;
                }
                let low = keys[h][0];
                let high = *keys[h].last().unwrap();
                let center = (low + high) / 2.;
                cost += (high - low - reach).max(0.) * 1000.
                    + keys[h].len().saturating_sub(5) as f64 * 10000.
                    + (center - centers[h]).abs()
                    + 0.1 * (high - low);
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
        for h in 0..2 {
            let pitches = group
                .iter()
                .filter(|&&i| usize::from(p.notes[i].hand.as_deref() == Some("right")) == h)
                .map(|&i| p.notes[i].pitch)
                .collect::<Vec<_>>();
            if !pitches.is_empty() {
                centers[h] = pitches.iter().sum::<f64>() / pitches.len() as f64;
            }
        }
        held.extend_from_slice(group);
    }
}

/// Bounded phrase search: retain eight alternative hand/finger histories. Explicit hands and
/// `finger` annotations are anchors. Held fingers cannot move to a different key.
pub fn fingers(p: &mut Pattern, reach: f64, times: &[(f64, f64)]) -> Result<(), (f64, String)> {
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
        fn recurse(
            pos: usize,
            notes: &[usize],
            h: usize,
            used: &mut [Option<usize>; 5],
            out: &mut Vec<Vec<(usize, usize, usize)>>,
            assign: &mut Vec<(usize, usize, usize)>,
            p: &Pattern,
            reach: f64,
        ) {
            if pos == notes.len() {
                out.push(assign.clone());
                return;
            }
            let i = notes[pos];
            let n = &p.notes[i];
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
                    let delta = n.pitch - p.notes[*j].pitch;
                    let orientation = if h == 1 { 1. } else { -1. };
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
                        _ => reach,
                    }
                    .min(reach);
                    delta * orientation * (f as f64 - g as f64) > 0. && delta.abs() <= limit
                });
                if valid {
                    used[f] = Some(i);
                    assign.push((i, h, f));
                    recurse(pos + 1, notes, h, used, out, assign, p, reach);
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
        recurse(0, notes, h, &mut used, &mut out, &mut Vec::new(), p, reach);
        out
    }
    for n in &p.notes {
        if let Some(f) = n.data.get("finger") {
            if f.as_f64()
                .is_none_or(|v| v.fract() != 0. || !(1.0..=5.).contains(&v))
            {
                return Err((real(n.at), "finger anchor must be an integer 1..5".into()));
            }
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
                        if transitions > 400_000 {
                            return Err((real(at),"fingering search reached its 400000-transition budget; shorten the phrase or anchor hands/fingers".into()));
                        }
                        let mut next = state.clone();
                        let assign = l.iter().chain(r).copied().collect::<Vec<_>>();
                        for &(i, h, f) in &assign {
                            let pitch = p.notes[i].pitch;
                            let target = state.last[h][f].unwrap_or(if h == 0 {
                                55. - f as f64 * 2.
                            } else {
                                64. + f as f64 * 2.
                            });
                            next.cost += (pitch - target).abs() * 0.15;
                            // Carry the hand's recent position even when another finger played it.
                            let center =
                                state.last[h].iter().flatten().copied().collect::<Vec<_>>();
                            if !center.is_empty() {
                                next.cost += (pitch
                                    - center.iter().sum::<f64>() / center.len() as f64)
                                    .abs()
                                    * 0.08;
                            }
                            if f == 0 && (pitch.round() as i32).rem_euclid(12) % 12 == 1 {
                                next.cost += 0.1;
                            }
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
