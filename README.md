my current compilation script

```bash
docker run --rm \
  -e LOCAL_UID=$(id -u) \
  -e LOCAL_GID=$(id -g) \
  -v "$PWD:/app" \
  ejortega/milkv-duo-rust:2.0 \
  cargo build --target riscv64gc-unknown-linux-musl --release
```
