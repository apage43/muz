//! Packs: audio-rate steps of the same kind that do not depend on each other,
//! computed together in the lanes of one vector, and delay writes likewise.
//!
//! A physical model repeats its sections: the junctions of a tone-hole
//! lattice read two lines each, filter, scale and sum the same way and write
//! two lines. Each lane of a pack runs the scalar step's floating-point
//! operations in the same order on its own operands, and a vector operation
//! rounds each lane as the scalar one does, so a pack computes what its steps
//! compute alone, bit for bit. Only the bounded copy of the audio-rate steps
//! packs (see `Gen::audio`); the guarded copy runs the steps one by one.
//!
//! Packs grow from the delay writes, the end of every chain of a physical
//! model: writes whose input steps have the same shape to some depth, and
//! whose chains share no step, are packed in schedule order, at most `LANES`
//! to a pack; then the steps that a pack's lanes read at one operand, when
//! they are distinct steps of one kind that do not depend on each other. An
//! operand whose lanes are not a pack is gathered from its scalars, and a
//! scalar reader of a packed step takes its lane. A pack of fewer than
//! `LANES` lanes fills the rest with copies of its first lane, which store
//! nothing and count towards nothing.
use super::*;
use cranelift_codegen::ir::types::{F32X4, I8X16, I32X4};

/// The lanes of a vector of f32.
pub(super) const LANES: usize = 4;
/// How deep the shapes of two writes' chains must agree to pack them.
const DEPTH: usize = 8;
/// How near a write (in steps) two writes must share a step to be packed
/// apart, each with its kind.
const NEAR: usize = 4;

/// What a packed step computes, and so what code its lanes share. Lanes
/// differ only in their constants, state and lines.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Kind {
    /// A sum or product, with which of its inputs are constants.
    Fold { product: bool, constants: Vec<bool> },
    /// A filter whose coefficients are constants, or whose memo another
    /// tier or cluster refills (see `Gen::refresh`).
    Filter { mode: u8, constant: bool },
    /// An undamped cubic delay of fixed time with a mirrored line.
    Delay { feedback: bool },
}

/// One pack's lanes: steps (indices of the schedule) or delay writes
/// (indices of the patch's delays).
#[derive(Clone, Debug)]
pub(super) enum Lanes {
    Steps(Vec<usize>),
    Writes(Vec<usize>),
}

impl Lanes {
    pub(super) fn len(&self) -> usize {
        match self {
            Lanes::Steps(l) | Lanes::Writes(l) => l.len(),
        }
    }
}

/// The packs of a patch's audio-rate steps (see the module's documentation).
pub(super) struct Packs {
    pub(super) packs: Vec<Lanes>,
    /// For each step of the schedule, its pack and lane.
    pub(super) of_step: Vec<Option<(usize, usize)>>,
    /// For each delay write, its pack and lane.
    pub(super) of_write: Vec<Option<(usize, usize)>>,
    /// The step of the schedule that computes each node.
    step_of: Vec<usize>,
}

/// The kind of step `k`, if it can be packed, given which nodes run in the
/// clusters (`quiet`) and which memos are refilled elsewhere (`hoisted`).
fn kind(p: &VoicePatch, k: usize, nodes: usize, quiet: &[Quiet], hoisted: &[u8]) -> Option<Kind> {
    let s = &p.steps[k];
    let node = s.node as usize;
    if k < p.settling || quiet[node] != Quiet::No {
        return None;
    }
    let constant = |slot: &Slot| *slot as usize >= nodes;
    match &s.op {
        Op::Add2(..) | Op::Add3(..) | Op::Sum { .. } | Op::Copy(_) => Some(Kind::Fold {
            product: false,
            constants: inputs(&s.op, &p.args).iter().map(constant).collect(),
        }),
        Op::Mul2(..) | Op::Mul3(..) | Op::Product { .. } => Some(Kind::Fold {
            product: true,
            constants: inputs(&s.op, &p.args).iter().map(constant).collect(),
        }),
        Op::Filter {
            coefficients, mode, ..
        } if coefficients.is_some() || hoisted[node] != CONSTANT => Some(Kind::Filter {
            mode: *mode,
            constant: coefficients.is_some(),
        }),
        Op::Delay {
            cubic: true,
            fixed_cubic: Some(_),
            damping: None,
            feedback,
            length,
            ..
        } if line_tail(*length) == *length => Some(Kind::Delay {
            feedback: constant(feedback),
        }),
        _ => None,
    }
}

/// The inputs a packed step's code reads: a filter reads its coefficients
/// from constants or its memo, and a fixed delay its time from neither.
fn operands(p: &VoicePatch, k: usize) -> Vec<Slot> {
    match &p.steps[k].op {
        Op::Filter { input, .. } => vec![*input],
        Op::Delay { feedback, .. } => vec![*feedback],
        op => inputs(op, &p.args),
    }
}

/// Group `p`'s audio-rate steps and delay writes into packs.
pub(super) fn packs(p: &VoicePatch, quiet: &[Quiet], hoisted: &[u8], nodes: usize) -> Packs {
    let count = p.steps.len();
    let kinds: Vec<Option<Kind>> = (0..count)
        .map(|k| kind(p, k, nodes, quiet, hoisted))
        .collect();
    let mut step_of = vec![usize::MAX; nodes];
    for (k, s) in p.steps.iter().enumerate() {
        step_of[s.node as usize] = k;
    }
    // The audio-rate step that computes a slot, if it runs in `rest`.
    let producer = |slot: Slot| -> Option<usize> {
        let k = *step_of.get(slot as usize)?;
        (k != usize::MAX && k >= p.settling && quiet[slot as usize] == Quiet::No).then_some(k)
    };
    // Which steps each step depends on this frame, as bit sets; schedule
    // order is topological among them (a delay reads its line, written
    // after every step).
    let words = count.div_ceil(64);
    let mut below = vec![vec![0u64; words]; count];
    for k in p.settling..count {
        for slot in inputs(&p.steps[k].op, &p.args) {
            if let Some(j) = producer(slot).filter(|&j| j < k) {
                let (low, high) = below.split_at_mut(k);
                let set = &mut high[0];
                for (a, b) in set.iter_mut().zip(&low[j]) {
                    *a |= b;
                }
                set[j / 64] |= 1 << (j % 64);
            }
        }
    }
    let depends = |a: usize, b: usize| below[a][b / 64] >> (b % 64) & 1 != 0;
    // The shape of each step's chain, to `DEPTH`.
    let mut shapes: std::collections::HashMap<(usize, usize), u64> = Default::default();
    fn shape(
        p: &VoicePatch,
        k: usize,
        depth: usize,
        kinds: &[Option<Kind>],
        producer: &dyn Fn(Slot) -> Option<usize>,
        nodes: usize,
        memo: &mut std::collections::HashMap<(usize, usize), u64>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        if let Some(&h) = memo.get(&(k, depth)) {
            return h;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        kinds[k].hash(&mut hasher);
        if depth > 0 {
            for slot in operands(p, k) {
                let h = match producer(slot) {
                    _ if slot as usize >= nodes => 0,
                    Some(j) if kinds[j].is_some() => {
                        shape(p, j, depth - 1, kinds, producer, nodes, memo)
                    }
                    _ => 1,
                };
                h.hash(&mut hasher);
            }
        }
        let h = hasher.finish();
        memo.insert((k, depth), h);
        h
    }
    // Packable writes, of mirrored lines: each one's shape (its input's;
    // a write handles any line alike) and the packable steps near it, to
    // `NEAR`.
    let mut shape_of = vec![None; p.delays.len()];
    let mut near: Vec<Vec<usize>> = vec![Vec::new(); p.delays.len()];
    for (w, &(_, input, _, length)) in p.delays.iter().enumerate() {
        if line_tail(length) != length {
            continue;
        }
        shape_of[w] = Some(match producer(input) {
            Some(j) if kinds[j].is_some() => {
                shape(p, j, DEPTH, &kinds, &producer, nodes, &mut shapes)
            }
            _ if (input as usize) >= nodes => 0,
            _ => 1,
        });
        let mut frontier: Vec<usize> = (producer(input).into_iter())
            .filter(|&j| kinds[j].is_some())
            .collect();
        for _ in 0..NEAR {
            let mut next = Vec::new();
            for j in frontier {
                // A line ends a section, and sections read one another's
                // lines: lines are near no write.
                if matches!(kinds[j], Some(Kind::Delay { .. })) {
                    continue;
                }
                if !near[w].contains(&j) {
                    near[w].push(j);
                    next.extend(
                        (operands(p, j).into_iter())
                            .filter_map(producer)
                            .filter(|&i| kinds[i].is_some()),
                    );
                }
            }
            frontier = next;
        }
    }
    // Writes that share steps near them form a group, as a junction's two
    // outgoing waves share its scattering; a group's writes in the order
    // of their inputs in the schedule. A step near more than two writes and
    // half of them, such as a source that every section reads, groups none.
    let writes: Vec<usize> = (0..p.delays.len())
        .filter(|&w| shape_of[w].is_some())
        .collect();
    let mut reach = vec![0usize; count];
    for &w in &writes {
        near[w].iter().for_each(|&j| reach[j] += 1);
    }
    for &w in &writes {
        near[w].retain(|&j| reach[j] <= 2 || 2 * reach[j] < writes.len());
    }
    let mut group = (0..p.delays.len()).collect::<Vec<_>>();
    fn root(group: &mut [usize], w: usize) -> usize {
        let mut r = w;
        while group[r] != r {
            r = group[r];
        }
        group[w] = r;
        r
    }
    for (i, &a) in writes.iter().enumerate() {
        for &b in &writes[..i] {
            if near[a].iter().any(|j| near[b].contains(j)) {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let position = |w: usize| (step_of.get(p.delays[w].1 as usize).copied()).unwrap_or(usize::MAX);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &w in &writes {
        let r = root(&mut group, w);
        match groups.iter_mut().find(|g| root(&mut group, g[0]) == r) {
            Some(g) => g.push(w),
            None => groups.push(vec![w]),
        }
    }
    for g in &mut groups {
        g.sort_by_key(|&w| (position(w), w));
    }
    groups.sort_by_key(|g| (position(g[0]), g[0]));
    let mut packs = Packs {
        packs: Vec::new(),
        of_step: vec![None; count],
        of_write: vec![None; p.delays.len()],
        step_of: step_of.clone(),
    };
    // Groups of one form, in order, `LANES` at a time: a pack for each of
    // their writes in turn, its lanes in the groups' order.
    let mut forms: Vec<(Vec<u64>, Vec<usize>)> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let form: Vec<u64> = g.iter().map(|&w| shape_of[w].expect("packable")).collect();
        match (forms.iter_mut()).find(|(f, members)| *f == form && members.len() < LANES) {
            Some((_, members)) => members.push(i),
            None => forms.push((form, vec![i])),
        }
    }
    for (form, members) in forms {
        if members.len() < 2 {
            continue;
        }
        for (role, _) in form.iter().enumerate() {
            let ws: Vec<usize> = members.iter().map(|&i| groups[i][role]).collect();
            for (lane, &w) in ws.iter().enumerate() {
                packs.of_write[w] = Some((packs.packs.len(), lane));
            }
            packs.packs.push(Lanes::Writes(ws));
        }
    }
    // Then the steps their lanes read at one operand: values first, the
    // lines of write packs last, so that the values' operands decide lanes.
    let grow = |packs: &mut Packs, lines: bool| {
        let mut at = 0;
        while at < packs.packs.len() {
            let operand_lanes: Vec<Vec<Slot>> = match &packs.packs[at] {
                Lanes::Writes(ws) if lines => {
                    vec![ws.iter().map(|&w| p.delays[w].0 as Slot).collect()]
                }
                Lanes::Writes(ws) => vec![ws.iter().map(|&w| p.delays[w].1).collect()],
                Lanes::Steps(_) if lines => vec![],
                Lanes::Steps(ks) => {
                    let ops: Vec<Vec<Slot>> = ks.iter().map(|&k| operands(p, k)).collect();
                    (0..ops[0].len())
                        .map(|j| ops.iter().map(|o| o[j]).collect())
                        .collect()
                }
            };
            at += 1;
            for slots in operand_lanes {
                let ks: Option<Vec<usize>> = slots.iter().map(|&s| producer(s)).collect();
                let Some(ks) = ks else { continue };
                let fits = kinds[ks[0]].is_some()
                    && ks
                        .iter()
                        .all(|&k| kinds[k] == kinds[ks[0]] && packs.of_step[k].is_none())
                    && ks.iter().enumerate().all(|(i, &a)| {
                        ks[..i]
                            .iter()
                            .all(|&b| a != b && !depends(a, b) && !depends(b, a))
                    });
                if fits {
                    for (lane, &k) in ks.iter().enumerate() {
                        packs.of_step[k] = Some((packs.packs.len(), lane));
                    }
                    packs.packs.push(Lanes::Steps(ks));
                }
            }
        }
    };
    grow(&mut packs, false);
    grow(&mut packs, true);
    grow(&mut packs, false);
    packs
}

impl Packs {
    /// Undo pack `id`: its lanes run alone. The pack stays, without lanes.
    pub(super) fn dissolve(&mut self, id: usize) {
        match std::mem::replace(&mut self.packs[id], Lanes::Steps(Vec::new())) {
            Lanes::Steps(ks) => ks.into_iter().for_each(|k| self.of_step[k] = None),
            Lanes::Writes(ws) => ws.into_iter().for_each(|w| self.of_write[w] = None),
        }
    }
}

#[cfg(test)]
thread_local! {
    static OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The lanes of the step packs and of the write packs that this
    /// thread's compilations emitted, and how many of those packs had fewer
    /// than `LANES` lanes.
    static PACKED: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}
/// Make this thread's compilations run every step alone.
#[cfg(test)]
pub(crate) fn set_off(off: bool) {
    OFF.with(|o| o.set(off));
}
/// See `PACKED`.
#[cfg(test)]
pub(crate) fn packed() -> [usize; 3] {
    PACKED.with(std::cell::Cell::get)
}
#[cfg(test)]
fn count(which: usize, lanes: usize) {
    PACKED.with(|p| {
        let mut n = p.get();
        n[which] += lanes;
        n[2] += usize::from(lanes < LANES);
        p.set(n);
    });
}

/// Whether compiled code packs steps: where vectors of four f32 have
/// instructions that insert and extract lanes.
pub(super) fn packing() -> bool {
    #[cfg(test)]
    if OFF.with(std::cell::Cell::get) {
        return false;
    }
    #[cfg(target_arch = "x86_64")]
    return std::is_x86_feature_detected!("sse4.1");
    #[cfg(not(target_arch = "x86_64"))]
    return cfg!(target_arch = "aarch64");
}

impl Packs {
    /// The pack and lane of the step that computes `slot`, if packed.
    fn lane_of(&self, slot: Slot) -> Option<(usize, usize)> {
        let k = *self.step_of.get(slot as usize)?;
        self.of_step.get(k).copied().flatten()
    }

    /// Whether `slots` are the lanes of pack `pack`, in order.
    fn holds(&self, pack: usize, slots: &[Slot]) -> bool {
        self.packs[pack].len() == slots.len()
            && (slots.iter().enumerate()).all(|(i, &s)| self.lane_of(s) == Some((pack, i)))
    }
}

impl Gen<'_, '_> {
    /// A vector of `lanes`, the rest copies of the first.
    fn vconst(&mut self, lanes: &[f32]) -> Value {
        let v = self.vbits(std::array::from_fn(|i| {
            lanes.get(i).unwrap_or(&lanes[0]).to_bits()
        }));
        let bound = lanes.iter().fold(0f64, |m, c| match c {
            c if c.is_finite() => m.max(f64::from(c.abs())),
            _ => f64::INFINITY,
        });
        self.bounds.insert(v, bound);
        v
    }

    /// A vector of f32 lanes of these bits.
    fn vbits(&mut self, bits: [u32; LANES]) -> Value {
        let bytes: Vec<u8> = bits.iter().flat_map(|b| b.to_le_bytes()).collect();
        let constant = self.b.func.dfg.constants.insert(bytes.into());
        self.b.ins().vconst(F32X4, constant)
    }

    /// A vector of scalars, the rest copies of the first.
    /// Each lane is made right before it is inserted, so that a load can
    /// be read straight into the vector.
    fn gather(&mut self, lanes: usize, mut lane: impl FnMut(&mut Self, usize) -> Value) -> Value {
        let x = lane(self, 0);
        let mut bound = self.bound(x);
        let mut v = self.b.ins().splat(F32X4, x);
        for i in 1..lanes {
            let x = lane(self, i);
            bound = bound.max(self.bound(x));
            v = self.b.ins().insertlane(v, x, i as u8);
        }
        self.bounds.insert(v, bound);
        v
    }

    /// The vector of a packed step's operand: constants, a pack emitted
    /// with these lanes in this order, or the lanes gathered.
    fn operand(&mut self, packs: &Packs, slots: &[Slot]) -> Value {
        if slots.iter().all(|&s| s as usize >= self.nodes) {
            let lanes: Vec<f32> = (slots.iter())
                .map(|&s| self.constants[s as usize - self.nodes])
                .collect();
            return self.vconst(&lanes);
        }
        if let Some((pack, 0)) = packs.lane_of(slots[0])
            && packs.holds(pack, slots)
            && let Some(&v) = self.vectors.get(&pack)
        {
            return v;
        }
        self.gather(slots.len(), |g, i| g.get(slots[i]))
    }

    /// The columns of four rows of four lanes.
    fn transpose(&mut self, rows: [Value; 4]) -> [Value; 4] {
        let little = MemFlagsData::new().with_endianness(ir::Endianness::Little);
        let bytes = rows.map(|r| self.b.ins().bitcast(I8X16, little, r));
        // Lanes of a and b by 32-bit index (b's from 4).
        let mut shuffle = |a: Value, b: Value, lanes: [u8; 4]| {
            let mask: Vec<u8> = lanes
                .iter()
                .flat_map(|&l| (0..4).map(move |i| 4 * l + i))
                .collect();
            let mask = self.b.func.dfg.immediates.push(mask.into());
            self.b.ins().shuffle(a, b, mask)
        };
        let low = [0, 4, 1, 5];
        let high = [2, 6, 3, 7];
        let t0 = shuffle(bytes[0], bytes[1], low);
        let t1 = shuffle(bytes[2], bytes[3], low);
        let t2 = shuffle(bytes[0], bytes[1], high);
        let t3 = shuffle(bytes[2], bytes[3], high);
        let columns = [
            shuffle(t0, t1, [0, 1, 4, 5]),
            shuffle(t0, t1, [2, 3, 6, 7]),
            shuffle(t2, t3, [0, 1, 4, 5]),
            shuffle(t2, t3, [2, 3, 6, 7]),
        ];
        columns.map(|c| self.b.ins().bitcast(F32X4, little, c))
    }

    /// A lane mask as a vector of f32.
    fn mask(&mut self, mask: Value) -> Value {
        self.b.ins().bitcast(F32X4, MemFlagsData::new(), mask)
    }

    /// Each lane of `x` if finite, else zero, as `finite` does a scalar.
    fn vfinite(&mut self, x: Value) -> Value {
        let d = self.b.ins().fsub(x, x);
        let finite = self.b.ins().fcmp(FloatCC::Ordered, d, d);
        let finite = self.mask(finite);
        let zero = self.vconst(&[0.]);
        let y = self.b.ins().bitselect(finite, x, zero);
        self.bounds.insert(y, FINITE);
        y
    }

    /// `guard` for each lane of `x`, given a bound on every lane.
    fn vguard(&mut self, x: Value, bound: f64) -> Value {
        if bound <= FINITE {
            self.bounds.insert(x, bound);
            x
        } else {
            self.vfinite(x)
        }
    }

    /// `x.clamp(low, high)` in each lane, as `clamp` has it.
    fn vclamp(&mut self, x: Value, low: Value, high: Value) -> Value {
        let below = self.b.ins().fcmp(FloatCC::LessThan, x, low);
        let below = self.mask(below);
        let x = self.b.ins().bitselect(below, low, x);
        let above = self.b.ins().fcmp(FloatCC::GreaterThan, x, high);
        let above = self.mask(above);
        self.b.ins().bitselect(above, high, x)
    }

    /// Lane `lane` of `v`, with `v`'s bound.
    fn lane(&mut self, v: Value, lane: usize) -> Value {
        let x = self.b.ins().extractlane(v, lane as u8);
        let bound = self.bound(v);
        self.bounds.insert(x, bound);
        x
    }

    /// Store lane `lane` of `v` at `base + offset`.
    fn store_lane(&mut self, v: Value, lane: usize, base: Value, offset: i32) {
        let x = if lane == 0 {
            self.b.ins().extractlane(v, 0)
        } else {
            // An integer lane is stored straight from the vector.
            let bits = self.b.ins().bitcast(I32X4, MemFlagsData::new(), v);
            self.b.ins().extractlane(bits, lane as u8)
        };
        self.store(x, base, offset);
    }

    /// The f32 at `offset` from each of `bases`, as a vector.
    fn gather_loads(&mut self, bases: &[Value], offset: i32) -> Value {
        self.gather(bases.len(), |g, i| g.load_f32(bases[i], offset))
    }

    /// `account` for the first `lanes` lanes of `x`.
    fn account_lanes(&mut self, x: Value, lanes: usize) {
        if self.squares.is_none() {
            return;
        }
        let x = if lanes < LANES {
            let mask = self.vbits(std::array::from_fn(|i| if i < lanes { !0 } else { 0 }));
            self.b.ins().band(x, mask)
        } else {
            x
        };
        let sum = match self.vsquares {
            Some(sum) => sum,
            None => self.vconst(&[0.]),
        };
        let sum = if fused() {
            self.b.ins().fma(x, x, sum)
        } else {
            let square = self.b.ins().fmul(x, x);
            self.b.ins().fadd(sum, square)
        };
        self.vsquares = Some(sum);
        // Pinned, as `account` pins its sum.
        let slot = *self.vanchor.get_or_insert_with(|| {
            let data = ir::StackSlotData::new(ir::StackSlotKind::ExplicitSlot, 16, 4);
            self.b.create_sized_stack_slot(data)
        });
        self.b.ins().stack_store(I64, sum, slot, 0);
    }

    /// Add the lanes that `account_lanes` counted to the sum of squares.
    pub(super) fn reduce_squares(&mut self) {
        if let (Some(lanes), Some(mut sum)) = (self.vsquares.take(), self.squares) {
            for lane in 0..LANES {
                let x = self.b.ins().extractlane(lanes, lane as u8);
                sum = self.b.ins().fadd(sum, x);
            }
            self.squares = Some(sum);
        }
    }

    /// Pack `id`: its steps' outputs, each lane left for its scalar
    /// readers, or its writes.
    pub(super) fn pack(&mut self, p: &VoicePatch, packs: &Packs, id: usize, store: &[bool]) {
        let ks = match &packs.packs[id] {
            Lanes::Writes(ws) => return self.pack_writes(p, packs, ws),
            Lanes::Steps(ks) => ks,
        };
        #[cfg(test)]
        count(0, ks.len());
        let x = match &p.steps[ks[0]].op {
            Op::Filter { .. } => self.pack_filter(p, packs, ks),
            Op::Delay { .. } => self.pack_delay(p, packs, id, ks),
            _ => self.pack_fold(p, packs, ks),
        };
        self.vectors.insert(id, x);
        for (lane, &k) in ks.iter().enumerate() {
            let node = p.steps[k].node as usize;
            let y = self.lane(x, lane);
            self.computed[node] = Some(y);
            if store[node] {
                self.store_lane(x, lane, self.values, 4 * node as i32);
                self.stored[node] = true;
            }
        }
    }

    /// Sums and products, as `fold` computes each.
    fn pack_fold(&mut self, p: &VoicePatch, packs: &Packs, ks: &[usize]) -> Value {
        let product = matches!(
            p.steps[ks[0]].op,
            Op::Mul2(..) | Op::Mul3(..) | Op::Product { .. }
        );
        let slots: Vec<Vec<Slot>> = (ks.iter())
            .map(|&k| inputs(&p.steps[k].op, self.args))
            .collect();
        let column = |j: usize| slots.iter().map(|s| s[j]).collect::<Vec<_>>();
        let mut x = self.operand(packs, &column(0));
        let mut bound = self.bound(x);
        let mut worst = bound;
        for j in 1..slots[0].len() {
            let y = self.operand(packs, &column(j));
            let b = self.bound(y);
            (x, bound) = if product {
                (self.b.ins().fmul(x, y), bound * b)
            } else {
                (self.b.ins().fadd(x, y), bound + b)
            };
            worst = worst.max(bound);
        }
        if worst <= FINITE {
            self.vguard(x, bound)
        } else {
            self.vfinite(x)
        }
    }

    /// Filters, as `inline` computes each.
    fn pack_filter(&mut self, p: &VoicePatch, packs: &Packs, ks: &[usize]) -> Value {
        let filter = |k: usize| match &p.steps[k].op {
            Op::Filter {
                input,
                coefficients,
                mode,
                ..
            } => (*input, *coefficients, *mode),
            _ => unreachable!("a pack's steps are of one kind"),
        };
        let mode = filter(ks[0]).2;
        let inputs: Vec<Slot> = ks.iter().map(|&k| filter(k).0).collect();
        let x = self.operand(packs, &inputs);
        let states: Vec<Value> = (ks.iter())
            .map(|&k| self.state_of(p.steps[k].node as usize))
            .collect();
        let constant: Option<Vec<(f32, f32, f32)>> = ks.iter().map(|&k| filter(k).1).collect();
        // Bounds on the coefficients' magnitudes, as `inline` has them.
        let [gb, kb, ab] = match &constant {
            Some(lanes) => lanes.iter().fold([0f64; 3], |m, &(g, k, a)| {
                [
                    m[0].max(f64::from(g.abs())),
                    m[1].max(f64::from(k.abs())),
                    m[2].max(f64::from(a.abs())),
                ]
            }),
            None => [6.32, margin(1. / f64::from(FILTER_Q[0])), 1.0001],
        };
        let [g, k, a, more] = match &constant {
            Some(lanes) => {
                let lanes: Vec<[f32; 4]> = (lanes.iter())
                    .map(|&(g, k, a)| [g, k, a, if mode >= 4 { g + 1. } else { g * a }])
                    .collect();
                [0, 1, 2, 3].map(|i| {
                    let column: Vec<f32> = lanes.iter().map(|l| l[i]).collect();
                    self.vconst(&column)
                })
            }
            None => {
                let at = |g: &mut Self, i: i32| g.gather_loads(&states, MEMO + 4 * i);
                if mode >= 4 {
                    // k and a are unused.
                    let zero = self.vconst(&[0.]);
                    [at(self, 0), zero, zero, at(self, 1)]
                } else {
                    [0, 1, 2, 3].map(|i| at(self, i))
                }
            }
        };
        let low = self.gather_loads(&states, LOW);
        let low = self.source(low);
        let (bx, bl) = (self.bound(x), self.bound(low));
        let (y, bound) = if mode >= 4 {
            let d = self.b.ins().fsub(x, low);
            let d = self.b.ins().fmul(d, g);
            let step = self.b.ins().fdiv(d, more);
            let low = self.b.ins().fadd(low, step);
            let next = self.b.ins().fadd(low, step);
            for (lane, &s) in states.iter().enumerate() {
                self.store_lane(next, lane, s, LOW);
            }
            self.account_lanes(next, ks.len());
            let bound = margin(bl + (bx + bl) * gb);
            match mode {
                4 => (low, bound),
                5 => (self.b.ins().fsub(x, low), margin(bx + bound)),
                _ => {
                    let two = self.vconst(&[2.]);
                    let twice = self.b.ins().fmul(low, two);
                    (self.b.ins().fsub(twice, x), margin(2. * bound + bx))
                }
            }
        } else {
            let band = self.gather_loads(&states, BAND);
            let band = self.source(band);
            let band_bound = margin(ab * self.bound(band) + gb * ab * (bx + bl));
            let low_bound = margin(bl + gb * band_bound);
            let v3 = self.b.ins().fsub(x, low);
            let ab = self.b.ins().fmul(a, band);
            let gav = self.b.ins().fmul(more, v3);
            let next_band = self.b.ins().fadd(ab, gav);
            let gb = self.b.ins().fmul(g, next_band);
            let next_low = self.b.ins().fadd(low, gb);
            let two = self.vconst(&[2.]);
            let t = self.b.ins().fmul(next_band, two);
            let stored_band = self.b.ins().fsub(t, band);
            let t = self.b.ins().fmul(next_low, two);
            let stored_low = self.b.ins().fsub(t, low);
            for (lane, &s) in states.iter().enumerate() {
                self.store_lane(stored_band, lane, s, BAND);
                self.store_lane(stored_low, lane, s, LOW);
            }
            self.account_lanes(stored_band, ks.len());
            self.account_lanes(stored_low, ks.len());
            match mode {
                1 => {
                    let k_band = self.b.ins().fmul(k, next_band);
                    let t = self.b.ins().fsub(x, k_band);
                    let y = self.b.ins().fsub(t, next_low);
                    (y, margin(bx + kb * band_bound + low_bound))
                }
                2 => (next_band, band_bound),
                3 => {
                    let k_band = self.b.ins().fmul(k, next_band);
                    (self.b.ins().fsub(x, k_band), margin(bx + kb * band_bound))
                }
                _ => (next_low, low_bound),
            }
        };
        self.vguard(y, bound)
    }

    /// Undamped cubic delays of fixed times, as `inline` and `cubic`
    /// compute each.
    fn pack_delay(&mut self, p: &VoicePatch, packs: &Packs, id: usize, ks: &[usize]) -> Value {
        struct Line {
            node: usize,
            feedback: Slot,
            max_feedback: f32,
            whole: usize,
            weights: [f32; 4],
            base: usize,
            length: usize,
        }
        let lines: Vec<Line> = (ks.iter())
            .map(|&k| match &p.steps[k].op {
                Op::Delay {
                    feedback,
                    max_feedback,
                    fixed_cubic: Some((whole, weights)),
                    base,
                    length,
                    ..
                } => Line {
                    node: p.steps[k].node as usize,
                    feedback: *feedback,
                    max_feedback: *max_feedback,
                    whole: *whole,
                    weights: *weights,
                    base: *base,
                    length: *length,
                },
                _ => unreachable!("a pack's steps are of one kind"),
            })
            .collect();
        // Each line's state and write index, and the address of its taps.
        let mut at = Vec::new();
        let mut taps = Vec::new();
        for l in &lines {
            let s = self.state_of(l.node);
            let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
            let line = self.b.ins().iadd_imm_s(self.delay, (l.base * 4) as i64);
            let base = (self.b.ins()).iadd_imm_s(index, l.length as i64 - l.whole as i64 - 2);
            let offset = self.b.ins().ishl_imm_u(base, 2);
            taps.push(self.b.ins().iadd(line, offset));
            at.push((s, index));
        }
        // A line's four taps are consecutive: a row each, turned into a
        // vector of each tap.
        let rows: Vec<Value> = (0..LANES)
            .map(|lane| {
                let flags = MemFlagsData::new().with_notrap();
                self.b
                    .ins()
                    .load(F32X4, flags, taps[lane.min(taps.len() - 1)], 0)
            })
            .collect();
        let columns = self.transpose([rows[0], rows[1], rows[2], rows[3]]);
        let mut x = self.vconst(&[0.]);
        for i in 0..4 {
            // Tap i is at 4 (3 - i) bytes.
            let sample = columns[3 - i];
            self.bounds
                .insert(sample, self.limit.unwrap_or(f64::INFINITY));
            let weights: Vec<f32> = lines.iter().map(|l| l.weights[i]).collect();
            let weight = self.vconst(&weights);
            let term = self.b.ins().fmul(sample, weight);
            x = self.b.ins().fadd(x, term);
        }
        let weight = (lines.iter())
            .map(|l| l.weights.iter().map(|w| f64::from(w.abs())).sum::<f64>())
            .fold(0., f64::max);
        let line_bound = self.limit.unwrap_or(f64::INFINITY);
        self.bounds.insert(x, margin(weight * line_bound));
        let slots: Vec<Slot> = lines.iter().map(|l| l.feedback).collect();
        let feedback = if slots.iter().all(|&s| s as usize >= self.nodes) {
            // Clamped here, as `clamp_const` clamps a constant.
            let lanes: Vec<f32> = (lines.iter())
                .map(|l| {
                    let c = self.constants[l.feedback as usize - self.nodes];
                    let c = if c < -l.max_feedback {
                        -l.max_feedback
                    } else {
                        c
                    };
                    if c > l.max_feedback {
                        l.max_feedback
                    } else {
                        c
                    }
                })
                .collect();
            self.vconst(&lanes)
        } else {
            let feedback = self.operand(packs, &slots);
            let low: Vec<f32> = lines.iter().map(|l| -l.max_feedback).collect();
            let high: Vec<f32> = lines.iter().map(|l| l.max_feedback).collect();
            let (low, high) = (self.vconst(&low), self.vconst(&high));
            self.vclamp(feedback, low, high)
        };
        let env = self.b.ins().fmul(x, feedback);
        let most = (lines.iter())
            .map(|l| f64::from(l.max_feedback.abs()))
            .fold(0., f64::max);
        let bound = margin(self.bound(x) * most);
        self.bounds.insert(env, bound);
        for (lane, l) in lines.iter().enumerate() {
            self.store_lane(env, lane, at[lane].0, ENV);
            self.returns[l.node] = bound;
        }
        self.vlines.insert(id, (at, env));
        let bound = self.bound(x);
        self.vguard(x, bound)
    }

    /// Delay writes, as `write_delay` writes each.
    fn pack_writes(&mut self, p: &VoicePatch, packs: &Packs, ws: &[usize]) {
        #[cfg(test)]
        count(1, ws.len());
        let lines: Vec<(usize, Slot, usize, usize)> = ws.iter().map(|&w| p.delays[w]).collect();
        let inputs: Vec<Slot> = lines.iter().map(|l| l.1).collect();
        let x = self.operand(packs, &inputs);
        // The lines' delays as their pack left them, else read back.
        let nodes: Vec<Slot> = lines.iter().map(|l| l.0 as Slot).collect();
        let kept = match packs.lane_of(nodes[0]) {
            Some((pack, 0)) if packs.holds(pack, &nodes) => self.vlines.get(&pack).cloned(),
            _ => None,
        };
        let (at, env) = match kept {
            Some(kept) => kept,
            None => {
                let at: Vec<(Value, Value)> = (lines.iter())
                    .map(|&(line, ..)| {
                        let s = self.state_of(line);
                        let index = self.b.ins().load(I64, MemFlagsData::trusted(), s, INDEX);
                        (s, index)
                    })
                    .collect();
                let states: Vec<Value> = at.iter().map(|&(s, _)| s).collect();
                let env = self.gather_loads(&states, ENV);
                // As the delays' steps left them this frame.
                let bound = (lines.iter())
                    .map(|&(line, ..)| self.returns[line])
                    .fold(0., f64::max);
                self.bounds.insert(env, bound);
                (at, env)
            }
        };
        let bound = margin(self.bound(x) + self.bound(env));
        let x = self.b.ins().fadd(x, env);
        let x = self.vguard(x, bound);
        for (lane, &(_, _, base, length)) in lines.iter().enumerate() {
            let (s, index) = at[lane];
            let line = self.b.ins().iadd_imm_s(self.delay, (base * 4) as i64);
            let offset = self.b.ins().ishl_imm_u(index, 2);
            let address = self.b.ins().iadd(line, offset);
            // Every sample again in the line's copy (see `LINE_MIRROR`).
            self.store_lane(x, lane, address, 0);
            self.store_lane(x, lane, address, 4 * length as i32);
            let next = self.b.ins().iadd_imm_s(index, 1);
            let end = self.b.ins().icmp_imm_s(IntCC::Equal, next, length as i64);
            let zero = self.b.ins().iconst(I64, 0);
            let next = self.b.ins().select(end, zero, next);
            self.b.ins().store(MemFlagsData::trusted(), next, s, INDEX);
        }
        self.account_lanes(x, ws.len());
    }
}
