use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::Path;

// calculate hashes to compare the dut outputs against during compile time
fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("reference_hashes.rs");
    let mut f = File::create(&dest).unwrap();

    let indices = [1_000u64, 10_000, 100_000, 1_000_000, 5_000_000, 10_000_000];

    writeln!(f, "pub static CHECKPOINTS: &[Checkpoint] = &[").unwrap();
    for &n in &indices {
        let (a, b) = fib_u128_at(n);
        let hash = hash_state(a, b);
        writeln!(
            f,
            "    Checkpoint {{ n: {}, expected_hash: {:?} }},",
            n, hash
        )
        .unwrap();
    }
    writeln!(f, "];").unwrap();

    println!("cargo:rerun-if-changed=src/build.rs");
}

fn fib_u128_at(n: u64) -> (u128, u128) {
    let mut a: u128 = 0;
    let mut b: u128 = 1;
    for _ in 0..n {
        let c = a.wrapping_add(b);
        a = b;
        b = c;
    }
    (a, b)
}

fn hash_state(a: u128, b: u128) -> String {
    let mut h = Sha256::new();
    h.update(a.to_le_bytes());
    h.update(b.to_le_bytes());
    hex::encode(h.finalize())
}
