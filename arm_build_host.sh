#!/bin/sh

# cargo build --bin dut --target aarch64-unknown-linux-gnu --release
RUSTFLAGS="-C target-feature=+crt-static" cargo build --bin dut --target aarch64-unknown-linux-gnu --release

cargo build --bin monitor --target x86_64-unknown-linux-gnu --release

scp -O ./target/aarch64-unknown-linux-gnu/release/dut ./arm_run_dut.sh root@milkv_arm:/root/

MONITOR_LISTEN="0.0.0.0:9000,0.0.0.0:9001" \
./target/x86_64-unknown-linux-gnu/release/monitor
