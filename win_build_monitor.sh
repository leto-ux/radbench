#!/bin/sh
#
# Cross-compile the monitor for Windows (x86_64).
# Requires: mingw-w64-gcc (pacman -S mingw-w64-gcc)
#
# Only builds the monitor — the DUT binary is Linux-only (runs on the boards).
# Output: target/x86_64-pc-windows-gnu/release/monitor.exe
#

set -e

echo "=== Building monitor for Windows ==="
cargo build --bin monitor --target x86_64-pc-windows-gnu --release

echo ""
echo "Done: target/x86_64-pc-windows-gnu/release/monitor.exe"
echo ""
echo "Transfer monitor.exe to your Windows host and run:"
echo "  set MONITOR_LISTEN=0.0.0.0:9000,0.0.0.0:9001"
echo "  monitor.exe"
