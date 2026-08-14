#!/bin/sh

set -e

RADBENCH_CORE=arm \
RADBENCH_CPU=0 \
RADBENCH_UART=/dev/ttyS0 \
RADBENCH_MONITOR=192.168.42.237:9000 \
RADBENCH_LOG=/root/radbench-arm.log \
./dut
