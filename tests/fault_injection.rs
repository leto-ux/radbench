use radbench::fault::{FaultInjector, FaultMode, FaultTarget};
use radbench::reference;
use radbench::workload::FibState;

// checkpoints
#[test]
fn clean_run_first_checkpoint() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    while state.n < checkpoints[0].n {
        state.step();
    }

    let result = state.check(&checkpoints[0]);
    assert!(
        result.passed,
        "First checkpoint (n={}) should pass in clean run",
        checkpoints[0].n
    );
}

#[test]
fn clean_run_all_checkpoints() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();
    let mut cp_idx = 0;

    while cp_idx < checkpoints.len() {
        state.step();
        if state.n == checkpoints[cp_idx].n {
            let result = state.check(&checkpoints[cp_idx]);
            assert!(
                result.passed,
                "Checkpoint at n={} failed in clean run (epoch={})",
                state.n, state.epoch
            );
            cp_idx += 1;
        }
    }
    assert_eq!(cp_idx, checkpoints.len());
}

#[test]
fn clean_run_two_epochs() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    for expected_epoch in 0..2u64 {
        let mut cp_idx = 0;
        while cp_idx < checkpoints.len() {
            state.step();
            if state.n == checkpoints[cp_idx].n {
                let result = state.check(&checkpoints[cp_idx]);
                assert!(
                    result.passed,
                    "Checkpoint n={} epoch={} failed in clean run",
                    state.n, state.epoch
                );
                cp_idx += 1;
            }
        }
        assert_eq!(state.epoch, expected_epoch);
        state.reset_epoch();
    }
    assert_eq!(state.epoch, 2);
}

// compile hash consistency
#[test]
fn reference_hashes_match_runtime_computation() {
    let checkpoints = reference::checkpoints();
    for cp in checkpoints {
        let (a, b) = reference::fib_u128_at(cp.n);
        let hash = reference::hash_state(a, b);
        assert_eq!(
            hash, cp.expected_hash,
            "compiletime hash for n={} doesn't match runtime",
            cp.n
        );
    }
}

// bit flip in register a
#[test]
fn flip_in_a_detected_at_checkpoint() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    while state.n < checkpoints[0].n - 1 {
        state.step();
    }

    state.flip_bit_a(42);

    state.step();
    assert_eq!(state.n, checkpoints[0].n);

    let result = state.check(&checkpoints[0]);
    assert!(!result.passed, "Bit flip in a should be detected");
}

// bit flip in register b
#[test]
fn flip_in_b_detected_at_checkpoint() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    while state.n < checkpoints[0].n - 1 {
        state.step();
    }

    state.flip_bit_b(0);
    state.step();

    let result = state.check(&checkpoints[0]);
    assert!(!result.passed, "Bit flip in b should be detected");
}

#[test]
fn all_128_bit_positions_detected_in_a() {
    let checkpoints = reference::checkpoints();

    for bit in 0..128u8 {
        let mut state = FibState::new();

        // Flip at midpoint (n=500) so it propagates to checkpoint
        while state.n < 500 {
            state.step();
        }
        state.flip_bit_a(bit);

        while state.n < checkpoints[0].n {
            state.step();
        }

        let result = state.check(&checkpoints[0]);
        assert!(
            !result.passed,
            "Bit flip at position {} in a not detected",
            bit
        );
    }
}

#[test]
fn all_128_bit_positions_detected_in_b() {
    let checkpoints = reference::checkpoints();

    for bit in 0..128u8 {
        let mut state = FibState::new();

        while state.n < 500 {
            state.step();
        }
        state.flip_bit_b(bit);

        while state.n < checkpoints[0].n {
            state.step();
        }

        let result = state.check(&checkpoints[0]);
        assert!(
            !result.passed,
            "Bit flip at position {} in b not detected",
            bit
        );
    }
}

// corruption

#[test]
fn corruption_propagates_to_all_subsequent_checkpoints() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    state.step();
    state.flip_bit_a(64);

    let mut cp_idx = 0;
    while cp_idx < checkpoints.len() && cp_idx < 3 {
        while state.n < checkpoints[cp_idx].n {
            state.step();
        }
        let result = state.check(&checkpoints[cp_idx]);
        assert!(
            !result.passed,
            "Corruption should propagate to checkpoint n={}",
            checkpoints[cp_idx].n
        );
        cp_idx += 1;
    }
}

#[test]
fn epoch_reset_recovers_from_corruption() {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();

    state.flip_bit_a(0);
    state.flip_bit_b(63);
    state.flip_bit_a(127);

    state.reset_epoch();
    assert_eq!(state.epoch, 1);
    assert_eq!(state.n, 0);

    while state.n < checkpoints[0].n {
        state.step();
    }

    let result = state.check(&checkpoints[0]);
    assert!(
        result.passed,
        "Epoch reset should fully recover from corruption"
    );
}

#[test]
fn fault_injector_see_produces_faults() {
    let mut fi = FaultInjector::new(42, FaultMode::See { rate: 0.5 });
    let mut count = 0;
    for _ in 0..1000 {
        if fi.maybe_fault().is_some() {
            count += 1;
        }
    }
    // with rate=0.5, I expect around 500 faults
    assert!(count > 200, "Too few faults: {}", count);
    assert!(count < 800, "Too many faults: {}", count);
    assert_eq!(fi.total_injected, count);
}

#[test]
fn fault_injector_see_rate_stays_constant() {
    let mut fi = FaultInjector::new(42, FaultMode::See { rate: 0.01 });
    assert!((fi.current_rate() - 0.01).abs() < 1e-10);
    fi.on_epoch_end();
    assert!((fi.current_rate() - 0.01).abs() < 1e-10);
    fi.on_epoch_end();
    assert!((fi.current_rate() - 0.01).abs() < 1e-10);
}

#[test]
fn fault_injector_tid_rate_increases() {
    let mut fi = FaultInjector::new(
        42,
        FaultMode::Tid {
            initial_rate: 0.001,
            acceleration: 2.0,
        },
    );

    assert!((fi.current_rate() - 0.001).abs() < 1e-10);
    fi.on_epoch_end();
    assert!((fi.current_rate() - 0.002).abs() < 1e-10);
    fi.on_epoch_end();
    assert!((fi.current_rate() - 0.004).abs() < 1e-10);
    fi.on_epoch_end();
    assert!((fi.current_rate() - 0.008).abs() < 1e-10);
}

// see

#[test]
fn see_simulation_all_faults_detected() {
    let checkpoints = reference::checkpoints();
    let mut fi = FaultInjector::new(12345, FaultMode::See { rate: 1e-4 });
    let mut epochs_clean = 0u64;
    let mut epochs_faulted = 0u64;
    let mut epochs_detected = 0u64;

    for _ in 0..20 {
        let mut state = FibState::new();
        let mut cp_idx = 0;
        let mut fault_in_epoch = false;
        let mut detected = false;

        while cp_idx < checkpoints.len() {
            state.step();

            if let Some(fault) = fi.maybe_fault() {
                match fault.target {
                    FaultTarget::FibA => state.flip_bit_a(fault.bit),
                    FaultTarget::FibB => state.flip_bit_b(fault.bit),
                }
                fault_in_epoch = true;
            }

            if state.n == checkpoints[cp_idx].n {
                let result = state.check(&checkpoints[cp_idx]);
                if !result.passed {
                    detected = true;
                    break;
                }
                cp_idx += 1;
            }
        }

        if fault_in_epoch {
            epochs_faulted += 1;
            if detected {
                epochs_detected += 1;
            }
        } else {
            epochs_clean += 1;
            assert!(
                !detected,
                "Clean epoch should not produce false positive detections"
            );
        }
    }

    eprintln!(
        "SEE sim: {} clean, {} faulted, {} detected, {} injected total",
        epochs_clean, epochs_faulted, epochs_detected, fi.total_injected
    );
    assert_eq!(
        epochs_faulted, epochs_detected,
        "Every faulted epoch must be detected"
    );
}

// tid, i.e. just increasing the rate at wthich stuff fails

#[test]
fn tid_simulation_increasing_failure_rate() {
    let checkpoints = reference::checkpoints();
    let short_checkpoints = &checkpoints[..2];

    let mut fi = FaultInjector::new(
        9999,
        FaultMode::Tid {
            initial_rate: 1e-5,
            acceleration: 3.0,
        },
    );

    let mut failure_counts = Vec::new();

    for epoch_num in 0..8 {
        let mut failures_this_epoch = 0u64;

        for _ in 0..10 {
            let mut state = FibState::new();
            let mut cp_idx = 0;

            while cp_idx < short_checkpoints.len() {
                state.step();

                if let Some(fault) = fi.maybe_fault() {
                    match fault.target {
                        FaultTarget::FibA => state.flip_bit_a(fault.bit),
                        FaultTarget::FibB => state.flip_bit_b(fault.bit),
                    }
                }

                if state.n == short_checkpoints[cp_idx].n {
                    let result = state.check(&short_checkpoints[cp_idx]);
                    if !result.passed {
                        failures_this_epoch += 1;
                        break;
                    }
                    cp_idx += 1;
                }
            }
        }

        failure_counts.push(failures_this_epoch);
        fi.on_epoch_end();

        eprintln!(
            "TID epoch {}: rate={:.2e}, failures={}/10",
            epoch_num,
            fi.current_rate(),
            failures_this_epoch
        );
    }

    let early_failures: u64 = failure_counts[..3].iter().sum();
    let late_failures: u64 = failure_counts[5..].iter().sum();
    eprintln!(
        "TID trend: early(0-2)={}, late(5-7)={}",
        early_failures, late_failures
    );
    assert!(
        late_failures >= early_failures,
        "TID should show increasing failure rate: early={} late={}",
        early_failures,
        late_failures
    );
}

#[test]
fn test_fault_injector_seed_warmup_no_step1_fault() {
    // With small seeds like 42 and rate = 5e-9, un-warmed xorshift64 had a cold-start
    // anomaly where step 1 generated p = 2.46e-9 < 5e-9, causing an immediate fault on step 1.
    // Ensure that with splitmix64 + warmup, step 1 does not deterministically fault.
    let mut fi = FaultInjector::new(42, FaultMode::See { rate: 5e-9 });
    assert!(
        fi.maybe_fault().is_none(),
        "Step 1 should not fault under rate 5e-9"
    );
}
