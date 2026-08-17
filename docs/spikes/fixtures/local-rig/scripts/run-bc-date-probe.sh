#!/usr/bin/env bash
# #98 -- Oracle 驱动对公元前日期给正年还是负年，canon_date 丢不丢纪元。
# 只有源端一段：在 client 容器里走生产同款 oracle 驱动取数路径。
# 目标端那一半 #35 已测完（probes/mysql-datetime-domain.sql），本探针不碰 MySQL。
# 必须在 arm64 mac 台架上运行，不进 CI。幂等，重复跑安全。
set -euo pipefail
cd "$(dirname "$0")/.."

REPO_ROOT=$(cd ../../../.. && pwd)
CRATE_DIR="$PWD/spike-bc-date"
RUST_IMAGE="rust:1-bookworm"

echo "==> 起台架（oracle + mysql + client）"
./scripts/up.sh

echo "==> 编译探针（不属于主 Cargo workspace）"
docker run --rm --platform linux/arm64 \
  -v "$REPO_ROOT:/workspace" -w /workspace \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo build --release \
  --manifest-path docs/spikes/fixtures/local-rig/spike-bc-date/Cargo.toml

echo "==> 拷进 client 容器并运行"
docker cp "$CRATE_DIR/target/release/spike-bc-date" qbs-client:/usr/local/bin/spike-bc-date
docker compose exec -T client /usr/local/bin/spike-bc-date
