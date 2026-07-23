/// Fault injection framework for radiation testing simulation.
///
/// Two modes model the two primary radiation effects:
/// - **SEE**: Random transient bit flips at a constant rate (models SEU/SET)
/// - **TID**: Progressive degradation with error rate increasing each epoch

/// Simple xorshift64 PRNG — no external dependency needed.
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

pub struct FaultInjector {
    rng: u64,
    mode: FaultMode,
    epoch_multiplier: f64,
    pub total_injected: u64,
}

#[derive(Debug, Clone)]
pub enum FaultMode {
    /// Single Event Effects: constant probability per step.
    See { rate: f64 },
    /// Total Ionizing Dose: rate = initial_rate * acceleration^epoch.
    Tid {
        initial_rate: f64,
        acceleration: f64,
    },
}

#[derive(Debug, Clone)]
pub struct FaultEvent {
    pub bit: u8,
    pub target: FaultTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultTarget {
    FibA,
    FibB,
}

impl FaultInjector {
    pub fn new(seed: u64, mode: FaultMode) -> Self {
        Self {
            rng: seed.max(1), // xorshift can't be seeded with 0
            mode,
            epoch_multiplier: 1.0,
            total_injected: 0,
        }
    }

    /// Current fault probability per step.
    pub fn current_rate(&self) -> f64 {
        match &self.mode {
            FaultMode::See { rate } => *rate,
            FaultMode::Tid { initial_rate, .. } => initial_rate * self.epoch_multiplier,
        }
    }

    /// Call at the end of each epoch to ramp TID rate.
    pub fn on_epoch_end(&mut self) {
        if let FaultMode::Tid { acceleration, .. } = &self.mode {
            self.epoch_multiplier *= acceleration;
        }
    }

    /// Returns Some(FaultEvent) if a fault should be injected this step.
    pub fn maybe_fault(&mut self) -> Option<FaultEvent> {
        let rate = self.current_rate();
        let r = xorshift64(&mut self.rng);
        let p = (r as f64) / (u64::MAX as f64);
        if p < rate {
            let bit = (xorshift64(&mut self.rng) % 128) as u8;
            let target = if xorshift64(&mut self.rng) % 2 == 0 {
                FaultTarget::FibA
            } else {
                FaultTarget::FibB
            };
            self.total_injected += 1;
            Some(FaultEvent { bit, target })
        } else {
            None
        }
    }
}
