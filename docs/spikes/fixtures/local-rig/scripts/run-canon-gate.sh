#!/usr/bin/env bash
# #43 -- M1 规范形式台架层手工门禁。必须在 arm64 mac 台架上运行，不进 CI。
set -euo pipefail
cd "$(dirname "$0")/.."

REPO_ROOT=$(cd ../../../.. && pwd)
CRATE_DIR="$PWD/spike-canon"
RUST_IMAGE="rust:1-bookworm"

echo "==> 确认台架在跑"
docker compose ps --status running --format '{{.Service}}' | grep -qx client \
  || { echo "!! client 容器没起，先跑 ./scripts/up.sh"; exit 2; }

echo "==> 单元测试（fixture 覆盖、漂移失败与 NULL bypass）"
docker run --rm --platform linux/arm64 \
  -v "$REPO_ROOT:/workspace" -w /workspace \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo test --release \
  --manifest-path docs/spikes/fixtures/local-rig/spike-canon/Cargo.toml

echo "==> 编译独立门禁程序（不属于主 Cargo workspace）"
docker run --rm --platform linux/arm64 \
  -v "$REPO_ROOT:/workspace" -w /workspace \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo build --release \
  --manifest-path docs/spikes/fixtures/local-rig/spike-canon/Cargo.toml

echo "==> 拷进 client 容器并运行"
docker cp "$CRATE_DIR/target/release/spike-canon" qbs-client:/usr/local/bin/spike-canon
docker compose exec -T client /usr/local/bin/spike-canon
