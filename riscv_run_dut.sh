#!/bin/sh

# Restart the DUT whenever it dies: a crash under beam is a data point, not
# the end of the run. The new process reports why the previous one died
# (crash marker / reboot / pstore) and the monitor logs a RESTART alarm.
export RADBENCH_CORE=riscv \
RADBENCH_CPU=0 \
RADBENCH_MONITOR=192.168.43.100:9001 \
RADBENCH_LOG=/root/radbench-riscv.log

while true; do
    ./dut
    echo "[run_dut] dut exited with status $?, restarting in 1 s" >&2
    sleep 1
done
