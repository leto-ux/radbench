use crate::reference::{self, Checkpoint};

// core loop functions, probably need to state what they actually do
pub struct FibState {
    pub a: u128,
    pub b: u128,
    pub n: u64,
    pub epoch: u64,
}

#[derive(Debug, Clone)]
pub struct CheckpointResult {
    pub epoch: u64,
    pub n: u64,
    pub hash: String,
    pub expected: &'static str,
    pub passed: bool,
}

impl FibState {
    pub fn new() -> Self {
        Self {
            a: 0,
            b: 1,
            n: 0,
            epoch: 0,
        }
    }

    // fibonacci iter
    #[inline(always)]
    pub fn step(&mut self) {
        let c = self.a.wrapping_add(self.b);
        self.a = self.b;
        self.b = c;
        self.n += 1;
    }

    // increment epoch
    pub fn reset_epoch(&mut self) {
        self.a = 0;
        self.b = 1;
        self.n = 0;
        self.epoch += 1;
    }

    // sha256 of current (a, b)
    pub fn hash(&self) -> String {
        reference::hash_state(self.a, self.b)
    }

    /// verify current state against checkpoint
    pub fn check(&self, checkpoint: &Checkpoint) -> CheckpointResult {
        let hash = self.hash();
        let passed = hash == checkpoint.expected_hash;
        CheckpointResult {
            epoch: self.epoch,
            n: self.n,
            hash,
            expected: checkpoint.expected_hash,
            passed,
        }
    }

    /// bit flip in register a
    pub fn flip_bit_a(&mut self, bit: u8) {
        self.a ^= 1u128 << (bit % 128);
    }

    /// same for register b
    pub fn flip_bit_b(&mut self, bit: u8) {
        self.b ^= 1u128 << (bit % 128);
    }
}
