use num_bigint::BigUint;
use num_traits::{One, Zero};
use sha2::{Digest, Sha256};

pub struct Checkpoint {
    pub n: u64,
    pub expected_hash: String,
}

// bottom of the barrel generated idea, not sure what the complexity of this algorithm is
pub fn checkpoints() -> Vec<Checkpoint> {
    let indices = [1_000, 10_000, 100_000, 1_000_000, 5_000_000, 10_000_000];
    // why is map syntax so gross
    indices
        .iter()
        .map(|&n| Checkpoint {
            n,
            expected_hash: hash_fib(n),
        })
        .collect()
}

pub fn hash_fib(n: u64) -> String {
    let f = fib(n);
    let mut h = Sha256::new();
    h.update(f.to_bytes_be());
    hex::encode(h.finalize())
}

pub fn fib(n: u64) -> BigUint {
    if n == 0 {
        return BigUint::zero();
    }
    // still don't know if fibonnaci starts with two 1's or 0 and 1
    let mut a = BigUint::zero();
    let mut b = BigUint::one();
    let highest = 63 - n.leading_zeros();
    for bit in (0..=highest).rev() {
        let c = &a * (&b * 2u32 - &a);
        let d = &a * &a + &b * &b;
        if n & (1u64 << bit) != 0 {
            a = d;
            b = c + d;
        } else {
            a = c;
            b = d;
        }
    }
    // i love how the return keyword isn't required
    a
}
