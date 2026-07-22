use num_bigint::BigUint;
use num_traits::{One, Zero};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("reference_hashes.rs");
    let mut f = File::create(&dest).unwrap();

    let indices = [1_000u64, 10_000, 100_000, 1_000_000, 5_000_000, 10_000_000];

    writeln!(f, "pub static CHECKPOINTS: &[Checkpoint] = &[").unwrap();
    for &n in &indices {
        let hash = hash_fib(n);
        writeln!(
            f,
            "    Checkpoint {{ n: {}, expected_hash: {:?} }},",
            n, hash
        )
        .unwrap();
    }
    writeln!(f, "];").unwrap();

    println!("cargo:rerun-if-changed=build.rs");
}

fn hash_fib(n: u64) -> String {
    let f = fib(n);
    let mut h = Sha256::new();
    h.update(f.to_bytes_be());
    hex::encode(h.finalize())
}

fn fib(n: u64) -> BigUint {
    if n == 0 {
        return BigUint::zero();
    }
    let mut a = BigUint::zero();
    let mut b = BigUint::one();
    let highest = 63 - n.leading_zeros();
    for bit in (0..=highest).rev() {
        let c = &a * (&b * 2u32 - &a);
        let d = &a * &a + &b * &b;
        if n & (1u64 << bit) != 0 {
            a = d.clone();
            b = c + d;
        } else {
            a = c;
            b = d;
        }
    }
    a
}
