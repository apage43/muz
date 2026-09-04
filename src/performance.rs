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
