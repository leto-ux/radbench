//! Runtime side of the register-file workload; see `lanes.rs` for the design.

use crate::lanes::{self, CHUNK, FP_LANES, INT_LANES, LCG_M};
use std::hint::black_box;

include!(concat!(env!("OUT_DIR"), "/lane_reference.rs"));

/// Expected integer lane values at checkpoint `idx` (0-based) of an epoch.
pub fn expected_int(idx: usize) -> &'static [u64; INT_LANES] {
    &LANE_CHECKPOINTS[idx]
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegCheck {
    /// (lane, expected ^ actual) for each corrupted integer lane.
    pub int_bad: Vec<(usize, u64)>,
    /// (lane, expected_bits ^ actual_bits) for each corrupted FP lane.
    pub fp_bad: Vec<(usize, u64)>,
}

impl RegCheck {
    pub fn passed(&self) -> bool {
        self.int_bad.is_empty() && self.fp_bad.is_empty()
    }
    pub fn flipped_bits(&self) -> u32 {
        self.int_bad
            .iter()
            .chain(&self.fp_bad)
            .map(|(_, x)| x.count_ones())
            .sum()
    }
}

pub struct RegWorkload {
    pub int: [u64; INT_LANES],
    pub fp: [f64; FP_LANES],
    /// Steps taken in the current epoch.
    pub n: u64,
    pub epoch: u64,
}

macro_rules! unroll {
    ($m:ident; $($i:literal)*) => { $( $m!($i); )* };
}

impl RegWorkload {
    pub fn new() -> Self {
        Self {
            int: lanes::int_seeds(),
            fp: lanes::fp_seeds(),
            n: 0,
            epoch: 0,
        }
    }

    pub fn reset_epoch(&mut self) {
        self.int = lanes::int_seeds();
        self.fp = lanes::fp_seeds();
        self.n = 0;
        self.epoch += 1;
    }

    /// Advance all lanes by `steps` (a multiple of CHUNK is fastest).
    #[inline(never)]
    pub fn advance(&mut self, steps: u64) {
        let m = black_box(LCG_M);
        // Runtime scale factors so the compiler can't reason about x*2*0.5.
        let two = black_box(2.0f64);
        let half = black_box(0.5f64);

        let mut x = self.int;
        let mut f = self.fp;
        let mut left = steps;
        while left > 0 {
            let chunk = left.min(CHUNK);
            for _ in 0..chunk {
                macro_rules! int_step {
                    ($i:literal) => {
                        x[$i] = x[$i].wrapping_mul(m).wrapping_add(lanes::incr($i));
                    };
                }
                macro_rules! fp_step {
                    ($i:literal) => {
                        f[$i] = (f[$i] * two) * half;
                    };
                }
                unroll!(int_step; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19);
                unroll!(fp_step; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23);
            }
            left -= chunk;
        }
        self.int = x;
        self.fp = f;
        self.n += steps;
    }

    /// Compare against the reference for checkpoint `idx`.
    pub fn check(&self, idx: usize) -> RegCheck {
        let exp = expected_int(idx);
        let fp_seeds = lanes::fp_seeds();
        RegCheck {
            int_bad: (0..INT_LANES)
                .filter(|&i| self.int[i] != exp[i])
                .map(|i| (i, self.int[i] ^ exp[i]))
                .collect(),
            fp_bad: (0..FP_LANES)
                .filter(|&i| self.fp[i].to_bits() != fp_seeds[i].to_bits())
                .map(|i| (i, self.fp[i].to_bits() ^ fp_seeds[i].to_bits()))
                .collect(),
        }
    }

    pub fn flip_int(&mut self, lane: usize, bit: u32) {
        self.int[lane % INT_LANES] ^= 1u64 << (bit % 64);
    }

    pub fn flip_fp(&mut self, lane: usize, bit: u32) {
        let l = lane % FP_LANES;
        self.fp[l] = f64::from_bits(self.fp[l].to_bits() ^ (1u64 << (bit % 64)));
    }
}

impl Default for RegWorkload {
    fn default() -> Self {
        Self::new()
    }
}
