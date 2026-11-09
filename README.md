my current compilation script

```bash
docker run --rm \
  -e LOCAL_UID=$(id -u) \
  -e LOCAL_GID=$(id -g) \
  -v "$PWD:/app" \
  ejortega/milkv-duo-rust:2.0 \
  cargo build --target riscv64gc-unknown-linux-musl --release
```

you need to have your firewall allow tcp 9000 traffic to go through the
interface, be it the usb cdc ncm, or via ethernet.

```bash
sudo firewall-cmd --zone=trusted --change-interface=enp56s0f4u1
```
*here the interface is my usbc port*

## Faults

The fault rate is a **per-step** probability. One epoch is 10 million steps
(last checkpoint at n=10M), so `expected faults/epoch ≈ rate × 10,000,000`.

SEE mode: constant random bit-flips (~1 fault every 2 epochs on average)
```sh
RADBENCH_FAULT_RATE=5e-8 ./radbench
 ```

TID mode: starts near-silent, doubles each epoch (first faults around epoch 5–7)
```sh
RADBENCH_FAULT_RATE=5e-9 RADBENCH_FAULT_MODE=tid RADBENCH_FAULT_ACCEL=2.0 ./radbench
```

> **Choosing a rate:** `rate × 10,000,000` = expected faults in epoch 0.
> For TID, epoch *k* multiplies by `accel^k`, so faults grow as
> `rate × 10M × accel^k`.

Bit flips

```sh
echo "flip" | nc -u 192.168.42.1 9001
```

## Test surface

A fault is only observable if the hit bit holds known data and is read back
afterwards, so the DUT keeps as much state under test as possible:

| Test | What it covers | Config (env) |
|------|----------------|--------------|
| `fib` | Integer datapath + a few registers, checked against build-time SHA-256 checkpoints | — |
| `regs` | 20 integer + 24 FP lanes kept live in registers in a tight loop (verified spill-free on aarch64/riscv64). Integer lanes are odd-multiplier LCGs (a flip is never masked), FP lanes are scaled ×2/×0.5 (exact). Checked every 1M steps against build-time references; mismatches name the lane and flipped bits. Inject with `echo flipreg \| nc -u <ip> 9001`. | `RADBENCH_REGS=0` disables |
| `mem` | Large DRAM-resident pattern held for the whole run and scrubbed periodically. Address-unique, ~50% ones, polarity inverted every 6 scrubs. Upsets are corrected and reported per word; a word failing two scrubs in a row is flagged `persistent` (stuck/TID). | `RADBENCH_MEM_MB` (default: ½ of MemAvailable), `RADBENCH_MEM_SCRUB_S` (default 10) |
| `cache` | Small region re-read often so it stays resident in L1/L2 SRAM | `RADBENCH_CACHE_KB` (default 16, 0 = off), `RADBENCH_CACHE_SCRUB_MS` (default 20) |

`scrub` summary packets carry `bytes` and `passes`, giving the bit·seconds of
exposure needed to compute a per-bit cross-section (upsets / fluence / bits).

## Liveness alarms (monitor)

Crashes and hangs produce no error packet, so the monitor raises alarms
(written to `logs/alarms_*.log`) from the heartbeat stream:

| Alarm | Meaning |
|-------|---------|
| `SILENT` | no packet for `MONITOR_TIMEOUT_MS` (default 5000) — crash, hang, link loss |
| `RECOVERED` | packets resumed; reports the outage length |
| `STALLED` | heartbeats arrive but the workload counter has not moved for `MONITOR_STALL_MS` |
| `SLOW` | iteration rate below 50% of the running baseline |
| `RESTART` | same DUT reappeared with a new `run_id` (reboot / process crash) |
| `DISCONNECT` | TCP connection closed or timed out |

## Crash, reboot and kernel fault capture (DUT)

- **Kernel log**: `/dev/kmsg` is followed and fault-looking messages (panic,
  oops, lockup/RCU stall, segfault, illegal instruction, EDAC/ECC, MMC/I/O and
  filesystem errors, anything at err priority or worse) are sent as `error`
  events with `test` = `kernel:<category>`; rate-limited to 30/min.
- **Process crashes**: panics and SIGSEGV/SIGBUS/SIGILL/SIGFPE/SIGABRT are
  written to `<RADBENCH_LOG>.crash`; the next run reports them (`test=crash`).
- **Reboots**: the kernel boot id is compared to the previous run's
  (`test=reboot`). If `/sys/fs/pstore` holds panic records from a previous
  boot (enable `CONFIG_PSTORE_RAM`/ramoops), they are reported (`test=pstore`).
- The `*_run_dut.sh` scripts restart the DUT whenever it exits.
