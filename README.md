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
