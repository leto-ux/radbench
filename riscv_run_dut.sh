#!/bin/sh

set -e

RADBENCH_CORE=riscv \
RADBENCH_CPU=0 \
#RADBENCH_UART=/dev/ttyS0 \
RADBENCH_MONITOR=192.168.42.221:9000 \
RADBENCH_LOG=/root/radbench-riscv.log \
./dut_riscv64
