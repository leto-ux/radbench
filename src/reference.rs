use sha2::{Digest, Sha256};

pub struct Checkpoint {
    pub n: u64,
    pub expected_hash: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/reference_hashes.rs"));

pub fn checkpoints() -> &'static [Checkpoint] {
    CHECKPOINTS
}

pub fn hash_state(a: u128, b: u128) -> String {
    let mut h = Sha256::new();
    h.update(a.to_le_bytes());
    h.update(b.to_le_bytes());
    hex::encode(h.finalize())
}

pub fn fib_u128_at(n: u64) -> (u128, u128) {
    let mut a: u128 = 0;
    let mut b: u128 = 1;
    for _ in 0..n {
        let c = a.wrapping_add(b);
        a = b;
        b = c;
    }
    (a, b)
}
