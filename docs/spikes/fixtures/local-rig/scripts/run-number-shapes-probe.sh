#!/usr/bin/env bash
# #104 -- NUMBER 纯小数（s > p）与负标度（s < 0）端到端往返取证。
# 源端段在 client 容器里走 oracle 驱动，目标端段把生成的 SQL 灌进 MySQL。
# 必须在 arm64 mac 台架上运行，不进 CI。幂等，重复跑安全。
set -euo pipefail
cd "$(dirname "$0")/.."

REPO_ROOT=$(cd ../../../.. && pwd)
CRATE_DIR="$PWD/spike-number-shapes"
RUST_IMAGE="rust:1-bookworm"
OUT_DIR=${OUT_DIR:-$PWD}
SQL_FILE="$OUT_DIR/number-shapes-generated.sql"

echo "==> 起台架（oracle + mysql + client）"
./scripts/up.sh

echo "==> 编译探针（不属于主 Cargo workspace）"
docker run --rm --platform linux/arm64 \
  -v "$REPO_ROOT:/workspace" -w /workspace \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo build --release \
  --manifest-path docs/spikes/fixtures/local-rig/spike-number-shapes/Cargo.toml

echo "==> 源端段：拷进 client 容器并运行"
docker cp "$CRATE_DIR/target/release/spike-number-shapes" qbs-client:/usr/local/bin/spike-number-shapes
docker compose exec -T client /usr/local/bin/spike-number-shapes | tee /tmp/spike-number-shapes.out

echo "==> 抽出目标端 SQL"
sed -n '/^-- <<<MYSQL-SQL-BEGIN>>>$/,/^-- <<<MYSQL-SQL-END>>>$/p' /tmp/spike-number-shapes.out \
  | grep -v '^-- <<<MYSQL-SQL-' > "$SQL_FILE"
echo "    ${SQL_FILE} -- $(wc -l < "${SQL_FILE}") 行"

echo "==> 目标端段：灌进 MySQL"
# --force：整数位不够这类插入**预期会报错**，报错本身就是结论，不能中断整轮。
# --show-warnings：静默舍入只以 Note 形式出现，必须打出来。
docker compose exec -T mysql \
  mysql -uspike -pspike123 qbs --default-character-set=utf8mb4 --table --force --show-warnings < "$SQL_FILE" 2>&1 \
  | grep -v '^mysql: \[Warning\] Using a password'
