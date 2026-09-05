#!/bin/sh

set -e

RADBENCH_CORE=riscv \
RADBENCH_CPU=0 \
RADBENCH_MONITOR=192.168.43.100:9001 \
RADBENCH_LOG=/root/radbench-riscv.log \
./dut
