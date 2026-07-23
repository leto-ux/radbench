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
