#!/bin/sh

# cargo build --bin dut --target aarch64-unknown-linux-gnu --release
RUSTFLAGS="-C target-feature=+crt-static" cargo build --bin dut --target aarch64-unknown-linux-gnu --release

cargo build --bin monitor --target x86_64-unknown-linux-gnu --release

scp -O ./target/aarch64-unknown-linux-gnu/release/dut ./arm_run_dut.sh root@milkv_arm:/root/

./target/x86_64-unknown-linux-gnu/release/monitor
