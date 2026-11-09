use radbench::lanes::{self, CHECKS_PER_EPOCH, FP_LANES, INT_LANES, STEPS_PER_CHECK};
use radbench::regs::RegWorkload;

fn run_epoch(w: &mut RegWorkload) -> Vec<bool> {
    (0..CHECKS_PER_EPOCH)
        .map(|i| {
            w.advance(STEPS_PER_CHECK);
            w.check(i).passed()
        })
        .collect()
}

#[test]
fn clean_epochs_pass_every_checkpoint() {
    let mut w = RegWorkload::new();
    for _ in 0..2 {
        assert!(run_epoch(&mut w).iter().all(|&p| p));
        w.reset_epoch();
    }
}

#[test]
fn unrolled_loop_matches_scalar_reference() {
    let mut w = RegWorkload::new();
    w.advance(12_345);
    let mut x = lanes::int_seeds();
    lanes::int_advance(&mut x, 12_345);
    assert_eq!(w.int, x);
    assert_eq!(w.n, 12_345);
}

#[test]
fn every_int_lane_and_bit_is_detected_and_never_masked() {
    for lane in 0..INT_LANES {
        for bit in [0u32, 1, 31, 62, 63] {
            let mut w = RegWorkload::new();
            w.advance(500);
            w.flip_int(lane, bit);
            let results = run_epoch_from(&mut w, 500);
            assert!(results.iter().all(|&p| !p), "lane {lane} bit {bit}");
        }
    }
}

#[test]
fn every_fp_lane_and_bit_is_detected() {
    for lane in 0..FP_LANES {
        for bit in 0..64 {
            let mut w = RegWorkload::new();
            w.flip_fp(lane, bit);
            w.advance(STEPS_PER_CHECK);
            let c = w.check(0);
            assert!(!c.passed(), "fp lane {lane} bit {bit}");
            assert_eq!(c.fp_bad.len(), 1);
        }
    }
}

#[test]
fn check_reports_lane_and_xor() {
    let mut w = RegWorkload::new();
    w.advance(STEPS_PER_CHECK);
    w.flip_int(7, 3);
    let c = w.check(0);
    assert_eq!(c.int_bad, vec![(7, 1 << 3)]);
    assert_eq!(c.flipped_bits(), 1);
}

/// Finish an epoch whose first `done` steps were already taken.
fn run_epoch_from(w: &mut RegWorkload, done: u64) -> Vec<bool> {
    let mut out = vec![];
    w.advance(STEPS_PER_CHECK - done);
    out.push(w.check(0).passed());
    for i in 1..CHECKS_PER_EPOCH {
        w.advance(STEPS_PER_CHECK);
        out.push(w.check(i).passed());
    }
    out
}
