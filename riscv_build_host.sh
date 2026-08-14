#!/bin/sh

docker run --rm \
  -e LOCAL_UID=$(id -u) \
  -e LOCAL_GID=$(id -g) \
  -v $PWD:/app \
  ejortega/milkv-duo-rust:2.0 \
  cargo build --bin dut --target riscv64gc-unknown-linux-musl --release

cargo build --bin monitor --target x86_64-unknown-linux-gnu --release

scp -O ./target/riscv64gc-unknown-linux-musl/release/dut ./riscv_run_dut.sh root@milkv_riscv:/root/

./target/x86_64-unknown-linux-gnu/release/monitor
