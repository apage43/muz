//! The tangent of a filter's prewarped cutoff, correctly rounded.
//!
//! A filter's gain is `tan(pi cutoff / rate)` in f32, which the interpreter
//! and compiled patches must compute alike, and which every platform should
//! compute alike for a render to be the same everywhere. A platform's `tanf`
//! need not be correctly rounded, and calling it costs a call. This tangent
//! is correctly rounded (the f32 nearest the exact tangent, ties to even) for
//! every `|x| <= DOMAIN`, and it is a short, branch-free sequence of f64
//! operations, which compiled code emits inline.
//!
//! It evaluates Lambert's continued fraction for the tangent, truncated at
//! depth 10: `x P(z) / Q(z)` with `z = x^2` and integer coefficients, each
//! polynomial by Estrin's scheme. Its error on the domain is at most 12 f64
//! units in the last place, against the 2^29 units of an f32's, and rounding
//! it to f32 gives the correctly rounded tangent of every f32 in the domain:
//! the test checks each one against an independent reference. `x * x` is
//! exact in f64 for an f32 `x`, and Rust and compiled code round each f64
//! operation alike, so the result is the same everywhere.

/// The largest argument with a correctly rounded result, 1.5 (a filter's
/// argument is at most 0.45 pi, about 1.4137).
pub(super) const DOMAIN: f32 = 1.5;
/// The numerator's coefficients, constant first, in `z = x^2`.
pub(super) const P: [f64; 6] = [13749310575., -1964187225., 64324260., -675675., 2145., -1.];
/// The denominator's coefficients, constant first.
pub(super) const Q: [f64; 6] = [
    13749310575.,
    -6547290750.,
    413513100.,
    -7567560.,
    45045.,
    -66.,
];

/// tan(x), correctly rounded for `|x| <= DOMAIN`; libm's beyond it.
pub(crate) fn tan(x: f32) -> f32 {
    if x.abs() <= DOMAIN {
        approximate(x) as f32
    } else {
        x.tan()
    }
}

/// `x P(z) / Q(z)` in f64. A compiled patch emits these operations in this
/// order.
#[inline(always)]
pub(super) fn approximate(x: f32) -> f64 {
    let x = x as f64;
    let z = x * x;
    let z2 = z * z;
    let p = (P[0] + P[1] * z) + z2 * ((P[2] + P[3] * z) + z2 * (P[4] - z));
    let q = (Q[0] + Q[1] * z) + z2 * ((Q[2] + Q[3] * z) + z2 * (Q[4] + Q[5] * z));
    x * p / q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Double-double arithmetic: an unevaluated sum `hi + lo` with `|lo|`
    /// at most half a unit of `hi`'s last place.
    type Dd = (f64, f64);
    fn quick_sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        (s, b - (s - a))
    }
    fn sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        let v = s - a;
        (s, (a - (s - v)) + (b - v))
    }
    fn add(a: Dd, b: Dd) -> Dd {
        let (s, e) = sum(a.0, b.0);
        let (t, f) = sum(a.1, b.1);
        let (s, e) = quick_sum(s, e + t);
        quick_sum(s, e + f)
    }
    fn mul_f64(a: Dd, b: f64) -> Dd {
        let p = a.0 * b;
        quick_sum(p, a.0.mul_add(b, -p) + a.1 * b)
    }
    fn divide(a: Dd, b: Dd) -> Dd {
        let q1 = a.0 / b.0;
        let r = add(a, mul_f64((-b.0, -b.1), q1));
        let q2 = r.0 / b.0;
        let r = add(r, mul_f64((-b.0, -b.1), q2));
        let q3 = r.0 / b.0;
        add(quick_sum(q1, q2), (q3, 0.))
    }

    /// The f64 bits below an f32's last bit.
    const BELOW: u64 = (1 << 29) - 1;

    /// The f32 nearest tan(x), from an independent double-double tangent:
    /// Lambert's continued fraction to depth 40, evaluated bottom up (its
    /// error is far below 2^-90 on the domain).
    fn reference(x: f32) -> f32 {
        let x = x as f64;
        let z = x * x;
        let mut d: Dd = (81., 0.);
        for k in (0..40).rev() {
            d = add((f64::from(2 * k + 1), 0.), divide((-z, 0.), d));
        }
        let (hi, lo) = divide((x, 0.), d);
        // Only a sum exactly halfway between two f32 rounds wrongly through
        // `hi` alone; then `lo` decides.
        let halfway = hi.to_bits() & BELOW == 1 << 28;
        let hi = match lo.partial_cmp(&0.) {
            Some(std::cmp::Ordering::Greater) if halfway => hi.next_up(),
            Some(std::cmp::Ordering::Less) if halfway => hi.next_down(),
            _ => hi,
        };
        hi as f32
    }

    /// Whether `y`, within a few units in the last place of the tangent,
    /// lies so near a midpoint between two f32 that its rounding is in
    /// doubt.
    fn ambiguous(y: f64) -> bool {
        (y.to_bits() & BELOW).abs_diff(1 << 28) <= 4
    }

    #[test]
    fn small_signed_and_outside_arguments() {
        for x in [0f32, -0.] {
            assert_eq!(tan(x).to_bits(), x.to_bits());
        }
        for x in [f32::from_bits(1), 1e-30, 1e-4, 0.5, 1.0, 1.4137167, 1.5] {
            assert_eq!(tan(x).to_bits(), reference(x).to_bits(), "tan({x})");
            assert_eq!(tan(-x).to_bits(), (-tan(x)).to_bits(), "tan(-{x})");
        }
        for x in [1.5000001f32, 3., 100., f32::INFINITY, f32::NAN] {
            assert_eq!(tan(x).to_bits(), x.tan().to_bits(), "libm's tan({x})");
        }
    }

    /// Every f32 in [0, DOMAIN] (negatives mirror them, `x P / Q` being odd
    /// in `x`): the result is the f32 nearest f64's tangent, or where that
    /// is in doubt, the reference's. The approximation's own error stays
    /// far from what could round wrongly. Run in release builds; it takes
    /// seconds across cores.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "exhaustive; run with --release")]
    fn correctly_rounded_everywhere() {
        let end = DOMAIN.to_bits() + 1;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u32;
        let chunk = end.div_ceil(threads);
        // From here the tangent's bits below x's spread over an f32's last
        // place; below, the result is x.
        let spread = 2f32.powi(-12);
        // Per thread: (x, got, expected) bits where they differ, and the
        // approximation's largest error.
        type Seen = (Vec<(u32, u32, u32)>, u64);
        let seen: Vec<Seen> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    s.spawn(move || {
                        let (mut wrong, mut error) = (Vec::new(), 0);
                        for bits in t * chunk..((t + 1) * chunk).min(end) {
                            let x = f32::from_bits(bits);
                            let exact = (x as f64).tan();
                            let expected = if ambiguous(exact) {
                                reference(x)
                            } else {
                                exact as f32
                            };
                            let got = tan(x);
                            if got.to_bits() != expected.to_bits() && wrong.len() < 4 {
                                wrong.push((bits, got.to_bits(), expected.to_bits()));
                            }
                            if x >= spread {
                                let y = approximate(x).to_bits();
                                error = error.max(y.abs_diff(exact.to_bits()));
                            }
                        }
                        (wrong, error)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let wrong: Vec<_> = seen.iter().flat_map(|s| &s.0).collect();
        assert!(
            wrong.is_empty(),
            "tan differs (x, got, expected bits): {wrong:x?}"
        );
        let error = seen.iter().map(|s| s.1).max().unwrap();
        assert!(error <= 16, "the approximation errs by {error} units");
    }
}
