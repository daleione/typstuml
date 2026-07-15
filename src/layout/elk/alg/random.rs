//! Faithful port of `java.util.Random` (the 48-bit LCG), which ELK
//! layered uses for tie-breaking (`InternalProperties.RANDOM`, seeded
//! from `elk.randomSeed`, default 1 — `GraphConfigurator.java:111`).
//! GWT's JRE emulation (and therefore elkjs) implements the same
//! algorithm, so reproducing the exact `nextInt` stream is required
//! for coordinate parity whenever a phase breaks a tie randomly.

const MULTIPLIER: u64 = 0x5DEECE66D;
const ADDEND: u64 = 0xB;
const MASK: u64 = (1 << 48) - 1;

#[derive(Debug, Clone)]
pub struct JavaRandom {
    seed: u64,
    draws: u64,
}

impl JavaRandom {
    pub fn new(seed: i64) -> Self {
        Self { seed: (seed as u64 ^ MULTIPLIER) & MASK, draws: 0 }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.draws += 1;
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        (self.seed >> (48 - bits)) as i64 as i32 // note: top bits, sign via i32 cast
    }

    /// Number of `next()` draws consumed so far (matches ELK's GWT
    /// `$nextInternal` call count — for debugging random alignment).
    pub fn draws(&self) -> u64 {
        self.draws
    }

    /// The 48-bit LCG state split into ELK's GWT `seedhi`/`seedlo`
    /// (each 24 bits), for comparing against an instrumented elkjs run.
    pub fn seed_hi_lo(&self) -> (u64, u64) {
        (self.seed >> 24, self.seed & 0xFF_FFFF)
    }

    /// `java.util.Random.setSeed(long)` — same scramble as construction.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ MULTIPLIER) & MASK;
    }

    /// `java.util.Random.nextLong()` = `((long) next(32) << 32) + next(32)`
    /// (both halves sign-extended; the crossing minimizer's `initialize`
    /// draws one of these to derive its per-run seed).
    pub fn next_long(&mut self) -> i64 {
        let hi = self.next(32) as i64;
        let lo = self.next(32) as i64;
        (hi << 32).wrapping_add(lo)
    }

    /// `java.util.Random.nextBoolean()` = `next(1) != 0` (each sweep in
    /// `minimizeCrossingsWithCounter` picks its direction with this).
    pub fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    /// `java.util.Random.nextFloat()` = `next(24) / (float) (1 << 24)`.
    /// `BarycenterHeuristic` perturbs each barycenter by a small
    /// `nextFloat`-derived jitter to break ties.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1i32 << 24) as f32
    }

    /// `java.util.Random.nextDouble()` =
    /// `(((long) next(26) << 27) + next(27)) / (double)(1L << 53)`.
    /// `BarycenterHeuristic.randomizeBarycenters` uses it.
    pub fn next_double(&mut self) -> f64 {
        let hi = (self.next(26) as i64) << 27;
        let lo = self.next(27) as i64;
        (hi + lo) as f64 * (1.0 / (1i64 << 53) as f64)
    }

    /// `java.util.Random.nextInt(bound)`.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        // Power-of-two fast path.
        if (bound & -bound) == bound {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference sequences generated with OpenJDK 15 (`new
    // java.util.Random(1)`) on this machine — not from memory.

    #[test]
    fn matches_java_util_random_seed_1() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<i32> = (0..10).map(|_| r.next_int(10)).collect();
        assert_eq!(seq, [5, 8, 7, 3, 4, 4, 4, 6, 8, 8]);
    }

    #[test]
    fn power_of_two_bound_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<i32> = (0..5).map(|_| r.next_int(16)).collect();
        assert_eq!(seq, [11, 1, 6, 6, 3]);
    }

    #[test]
    fn small_bound_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<i32> = (0..6).map(|_| r.next_int(3)).collect();
        assert_eq!(seq, [0, 1, 1, 0, 2, 1]);
    }

    #[test]
    fn next_long_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<i64> = (0..4).map(|_| r.next_long()).collect();
        assert_eq!(
            seq,
            [
                -4964420948893066024,
                7564655870752979346,
                3831662765844904176,
                6137546356583794141
            ]
        );
    }

    #[test]
    fn next_boolean_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<bool> = (0..8).map(|_| r.next_boolean()).collect();
        assert_eq!(seq, [true, false, false, false, false, false, false, true]);
    }

    #[test]
    fn next_float_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<f32> = (0..4).map(|_| r.next_float()).collect();
        assert_eq!(seq, [0.7308782, 0.100473166, 0.4100808, 0.40743977]);
    }

    #[test]
    fn next_double_matches_java() {
        let mut r = JavaRandom::new(1);
        let seq: Vec<f64> = (0..4).map(|_| r.next_double()).collect();
        assert_eq!(
            seq,
            [
                0.7308781907032909,
                0.41008081149220166,
                0.20771484130971707,
                0.3327170559595112
            ]
        );
    }

    /// The crossing minimizer's `initialize` does `seed = nextLong()`
    /// then `setSeed(seed)`; reproduce that exact handshake.
    #[test]
    fn set_seed_from_next_long_matches_java() {
        let mut r = JavaRandom::new(1);
        let s = r.next_long();
        assert_eq!(s, -4964420948893066024);
        r.set_seed(s);
        let seq: Vec<i32> = (0..5).map(|_| r.next_int(10)).collect();
        assert_eq!(seq, [9, 5, 4, 7, 5]);
    }
}

