//! Cubic / quadratic / linear root finders. Direct port of
//! `pathplan/solvers.c::{solve1, solve2, solve3}`. Coefficients are
//! provided low-to-high (`coeff[0]` is the constant term).
//!
//! `solve1` returns the sentinel `RootCount::Infinite` when the polynomial
//! is identically zero — `splineintersectsline` keys off this to signal a
//! degenerate cubic-vs-line check, where the line lives on a coordinate
//! axis and one of the cubic component polynomials is constant.

const ROOT_EPS: f64 = 1e-7;
const PI: f64 = std::f64::consts::PI;

/// How `solveN` reports its result. The C original packs this as an `int`
/// (count, with `4` meaning "infinitely many"); the enum makes the sentinel
/// explicit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum RootCount {
    None,
    Finite(usize),
    Infinite,
}

/// Solve `c[0] + c[1]·t + c[2]·t² + c[3]·t³ = 0`. Real roots only; up to
/// three. Falls through to `solve2` when the cubic coefficient vanishes.
pub(super) fn solve3(coeff: [f64; 4], roots: &mut [f64; 3]) -> RootCount {
    let a = coeff[3];
    let b = coeff[2];
    let c = coeff[1];
    let d = coeff[0];
    if a.abs() < ROOT_EPS {
        return solve2([d, c, b], roots);
    }

    let b_over_3a = b / (3.0 * a);
    let c_over_a = c / a;
    let d_over_a = d / a;
    let p_init = b_over_3a * b_over_3a;
    let q = 2.0 * b_over_3a * p_init - b_over_3a * c_over_a + d_over_a;
    let p = c_over_a / 3.0 - p_init;
    let disc = q * q + 4.0 * p * p * p;

    let count;
    if disc < 0.0 {
        let r = 0.5 * (-disc + q * q).sqrt();
        let theta = (-disc).sqrt().atan2(-q);
        let temp = 2.0 * cuberoot(r);
        roots[0] = temp * (theta / 3.0).cos();
        roots[1] = temp * ((theta + 2.0 * PI) / 3.0).cos();
        roots[2] = temp * ((theta - 2.0 * PI) / 3.0).cos();
        count = 3;
    } else {
        let alpha = 0.5 * (disc.sqrt() - q);
        let beta = -q - alpha;
        roots[0] = cuberoot(alpha) + cuberoot(beta);
        if disc > 0.0 {
            count = 1;
        } else {
            roots[1] = -0.5 * roots[0];
            roots[2] = roots[1];
            count = 3;
        }
    }
    for r in roots.iter_mut().take(count) {
        *r -= b_over_3a;
    }
    RootCount::Finite(count)
}

fn solve2(coeff: [f64; 3], roots: &mut [f64; 3]) -> RootCount {
    let a = coeff[2];
    let b = coeff[1];
    let c = coeff[0];
    if a.abs() < ROOT_EPS {
        return solve1([c, b], roots);
    }
    let b_over_2a = b / (2.0 * a);
    let c_over_a = c / a;
    let disc = b_over_2a * b_over_2a - c_over_a;
    if disc < 0.0 {
        RootCount::None
    } else if disc == 0.0 {
        roots[0] = -b_over_2a;
        RootCount::Finite(1)
    } else {
        roots[0] = -b_over_2a + disc.sqrt();
        roots[1] = -2.0 * b_over_2a - roots[0];
        RootCount::Finite(2)
    }
}

fn solve1(coeff: [f64; 2], roots: &mut [f64; 3]) -> RootCount {
    let a = coeff[1];
    let b = coeff[0];
    if a.abs() < ROOT_EPS {
        if b.abs() < ROOT_EPS {
            RootCount::Infinite
        } else {
            RootCount::None
        }
    } else {
        roots[0] = -b / a;
        RootCount::Finite(1)
    }
}

/// Real cube root, signed. `f64::powf` would mis-handle negative bases.
fn cuberoot(x: f64) -> f64 {
    if x < 0.0 {
        -(-x).powf(1.0 / 3.0)
    } else {
        x.powf(1.0 / 3.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn solve3_known_roots() {
        // (t-1)(t-2)(t-3) = t³ - 6t² + 11t - 6
        let mut r = [0.0; 3];
        let count = solve3([-6.0, 11.0, -6.0, 1.0], &mut r);
        assert_eq!(count, RootCount::Finite(3));
        let mut sorted = r;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(near(sorted[0], 1.0), "{:?}", sorted);
        assert!(near(sorted[1], 2.0), "{:?}", sorted);
        assert!(near(sorted[2], 3.0), "{:?}", sorted);
    }

    #[test]
    fn solve3_falls_through_to_solve2() {
        // 2t² - 8 = 0 → t = ±2. Pass with cubic coeff = 0.
        let mut r = [0.0; 3];
        let count = solve3([-8.0, 0.0, 2.0, 0.0], &mut r);
        assert_eq!(count, RootCount::Finite(2));
        let mut sorted = [r[0], r[1]];
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(near(sorted[0], -2.0));
        assert!(near(sorted[1], 2.0));
    }

    #[test]
    fn solve3_falls_through_to_solve1() {
        // 3t + 6 = 0 → t = -2. Pass with cubic & quadratic coeffs = 0.
        let mut r = [0.0; 3];
        let count = solve3([6.0, 3.0, 0.0, 0.0], &mut r);
        assert_eq!(count, RootCount::Finite(1));
        assert!(near(r[0], -2.0));
    }

    #[test]
    fn solve3_constant_zero_is_infinite() {
        let mut r = [0.0; 3];
        let count = solve3([0.0, 0.0, 0.0, 0.0], &mut r);
        assert_eq!(count, RootCount::Infinite);
    }

    #[test]
    fn solve3_one_real_root() {
        // t³ + t = 0 has one real root (t=0).
        let mut r = [0.0; 3];
        let count = solve3([0.0, 1.0, 0.0, 1.0], &mut r);
        assert_eq!(count, RootCount::Finite(1));
        assert!(near(r[0], 0.0));
    }
}
