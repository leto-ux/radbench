#!/bin/sh
#
# Build both DUT binaries, deploy to both Milk-V boards, and start the monitor.
# The monitor listens on two ports simultaneously:
#   - 9000: ARM DUT
#   - 9001: RISC-V DUT
#
# Skips build+deploy for a DUT if its board is unreachable.
#

set -e

ARM_IP="192.168.43.2"
RISCV_IP="192.168.43.1"

arm_reachable=false
riscv_reachable=false

if ping -c1 -W2 "$ARM_IP" >/dev/null 2>&1; then
    arm_reachable=true
    echo "=== ARM board ($ARM_IP) reachable ==="
else
    echo "=== ARM board ($ARM_IP) unreachable, skipping ==="
fi

if ping -c1 -W2 "$RISCV_IP" >/dev/null 2>&1; then
    riscv_reachable=true
    echo "=== RISC-V board ($RISCV_IP) reachable ==="
else
    echo "=== RISC-V board ($RISCV_IP) unreachable, skipping ==="
fi

if [ "$arm_reachable" = true ]; then
    echo "=== Building ARM DUT ==="
    RUSTFLAGS="-C target-feature=+crt-static" cargo build --bin dut --target aarch64-unknown-linux-gnu --release
fi

if [ "$riscv_reachable" = true ]; then
    echo "=== Building RISC-V DUT (via Docker) ==="
    docker run --rm \
      -e LOCAL_UID=$(id -u) \
      -e LOCAL_GID=$(id -g) \
      -v $PWD:/app \
      ejortega/milkv-duo-rust:2.0 \
      cargo build --bin dut --target riscv64gc-unknown-linux-musl --release
fi

echo "=== Building monitor ==="
cargo build --bin monitor --target x86_64-unknown-linux-gnu --release

if [ "$arm_reachable" = true ]; then
    echo "=== Deploying to ARM board ==="
    scp -O ./target/aarch64-unknown-linux-gnu/release/dut ./arm_run_dut.sh root@milkv_arm:/root/
fi

if [ "$riscv_reachable" = true ]; then
    echo "=== Deploying to RISC-V board ==="
    scp -O ./target/riscv64gc-unknown-linux-musl/release/dut ./riscv_run_dut.sh root@milkv_riscv:/root/
fi

echo "=== Starting monitor (ports 9000 + 9001) ==="
MONITOR_LISTEN="0.0.0.0:9000,0.0.0.0:9001" \
./target/x86_64-unknown-linux-gnu/release/monitor
