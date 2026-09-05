#!/bin/sh
#
# Build both DUT binaries, deploy to both Milk-V boards, and start the monitor.
# The monitor listens on two ports simultaneously:
#   - 9000: ARM DUT
#   - 9001: RISC-V DUT
#

set -e

echo "=== Building ARM DUT ==="
RUSTFLAGS="-C target-feature=+crt-static" cargo build --bin dut --target aarch64-unknown-linux-gnu --release

echo "=== Building RISC-V DUT (via Docker) ==="
docker run --rm \
  -e LOCAL_UID=$(id -u) \
  -e LOCAL_GID=$(id -g) \
  -v $PWD:/app \
  ejortega/milkv-duo-rust:2.0 \
  cargo build --bin dut --target riscv64gc-unknown-linux-musl --release

echo "=== Building monitor ==="
cargo build --bin monitor --target x86_64-unknown-linux-gnu --release

echo "=== Deploying to ARM board ==="
scp -O ./target/aarch64-unknown-linux-gnu/release/dut ./arm_run_dut.sh root@milkv_arm:/root/

echo "=== Deploying to RISC-V board ==="
scp -O ./target/riscv64gc-unknown-linux-musl/release/dut ./riscv_run_dut.sh root@milkv_riscv:/root/

echo "=== Starting monitor (ports 9000 + 9001) ==="
MONITOR_LISTEN="0.0.0.0:9000,0.0.0.0:9001" \
./target/x86_64-unknown-linux-gnu/release/monitor
