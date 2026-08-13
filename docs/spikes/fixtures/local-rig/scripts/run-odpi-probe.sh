#!/usr/bin/env bash
# #3 —— 在台架上跑 ODPI-C 类型保真度探针。幂等，重复跑安全。
#
# 为什么不把 Rust 塞进 Dockerfile.client：
#   ODPI-C 是**运行时** dlopen `libclntsh.so` 的，编译期不需要 Instant Client。
#   所以「rust 镜像里编译 → 拷进 client 容器里跑」比给 client 镜像装一套工具链更省事，
#   也不动已经验证过的台架镜像。
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE_DIR="$PWD/spike-odpi"
RUST_IMAGE="rust:1-bookworm"

echo "==> 确认台架在跑"
docker compose ps --status running --format '{{.Service}}' | grep -qx client \
  || { echo "!! client 容器没起，先跑 ./scripts/up.sh"; exit 1; }

echo "==> 单元测试（规范形式转换，纯字符串逻辑，不连库）"
docker run --rm --platform linux/arm64 \
  -v "$CRATE_DIR:/src" -w /src \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo test --release

echo "==> 编译探针（arm64 原生）"
docker run --rm --platform linux/arm64 \
  -v "$CRATE_DIR:/src" -w /src \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo build --release

echo "==> 拷进 client 容器并运行（Instant Client 19.32 在那里）"
docker cp "$CRATE_DIR/target/release/spike-odpi" qbs-client:/usr/local/bin/spike-odpi
docker compose exec -T client /usr/local/bin/spike-odpi
