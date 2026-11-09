//! Register-file / datapath workload ("regs" test).
//!
//! Keeps as many architectural registers live as possible so a flip in the
//! register file, ALU, multiplier or FPU has a chance to land in live state:
//!
//! - `INT_LANES` independent 64-bit LCGs `x = x*M + c_i`. With M odd any
//!   difference between a corrupted and a clean lane is preserved forever
//!   (multiplication by an odd number is a bijection mod 2^64), so a flip is
//!   never masked. Expected values are computed at build time (build.rs
//!   includes this file).
//! - `FP_LANES` f64 values repeatedly multiplied by 2 and by 0.5. Scaling by
//!   powers of two is exact, so every lane must still equal its seed at each
//!   checkpoint; any mantissa/exponent/sign flip survives and is caught.
//!
//! The hot loop is a tight inner loop with no calls, so the lanes stay in
//! registers instead of being spilled to the stack.
//!
//! This file must stay dependency-free: build.rs includes it via `#[path]`.

pub const INT_LANES: usize = 20;
pub const FP_LANES: usize = 24;
/// Knuth MMIX multiplier (≡ 1 mod 4 → full period with odd increments).
pub const LCG_M: u64 = 6_364_136_223_846_793_005;
/// Steps between checkpoints, and checkpoints per epoch.
pub const STEPS_PER_CHECK: u64 = 1_000_000;
pub const CHECKS_PER_EPOCH: usize = 4;
/// Inner-loop length (steps executed without leaving registers).
pub const CHUNK: u64 = 4096;

pub fn int_seeds() -> [u64; INT_LANES] {
    let mut s = [0u64; INT_LANES];
    let mut x: u64 = 0x0123_4567_89AB_CDEF;
    for v in s.iter_mut() {
        x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x6A09_E667_F3BC_C909);
        *v = x ^ (x >> 29);
    }
    s
}

/// Small odd increments: fit in an add-immediate on RISC-V and AArch64, so
/// they don't each need a register.
#[inline(always)]
pub const fn incr(lane: usize) -> u64 {
    (2 * lane as u64) + 1
}

pub fn fp_seeds() -> [f64; FP_LANES] {
    let mut s = [0f64; FP_LANES];
    for (i, v) in s.iter_mut().enumerate() {
        // Varied sign, exponent and mantissa bits.
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        *v = sign * (1.0 + (i as f64) * 0.123_456_789_012_345_6) * (2f64).powi(i as i32 - 12);
    }
    s
}

/// Advance the integer lanes by `steps` (scalar reference version used by
/// build.rs and tests).
pub fn int_advance(lanes: &mut [u64; INT_LANES], steps: u64) {
    for _ in 0..steps {
        for (i, x) in lanes.iter_mut().enumerate() {
            *x = x.wrapping_mul(LCG_M).wrapping_add(incr(i));
        }
    }
}
