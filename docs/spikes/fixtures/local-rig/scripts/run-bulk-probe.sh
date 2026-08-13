#!/usr/bin/env bash
# #5 —— 在台架上跑流式 fetch 的内存形状探针。幂等，重复跑安全。
#
# 台架边界（README「边界」第 2 条）：服务端在模拟层上，**吞吐的绝对数字作废**。
# 这里量的是内存**形状** —— 峰值随批次走还是随行数走。那是驱动客户端侧的行为，
# 与服务端快慢无关，因此在本台架上成立。耗时只打印不下结论。
#
# 编译方式与 #3 的 run-odpi-probe.sh 一致：rust 镜像里编译 → 拷进 client 容器里跑
# （ODPI-C 是运行时 dlopen libclntsh.so 的，编译期不需要 Instant Client）。
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE_DIR="$PWD/spike-bulk"
RUST_IMAGE="rust:1-bookworm"

echo "==> 确认台架在跑"
docker compose ps --status running --format '{{.Service}}' | grep -qx client \
  || { echo "!! client 容器没起，先跑 ./scripts/up.sh"; exit 1; }

echo "==> 编译探针（arm64 原生）"
docker run --rm --platform linux/arm64 \
  -v "$CRATE_DIR:/src" -w /src \
  -v qbs-cargo-registry:/usr/local/cargo/registry \
  "$RUST_IMAGE" cargo build --release

docker cp "$CRATE_DIR/target/release/spike-bulk" qbs-client:/usr/local/bin/spike-bulk

OUT=$(mktemp)
run() {  # mode rows batch fetch_array_size prefetch_rows [table]
  echo "-- $*"
  docker compose exec -T client /usr/local/bin/spike-bulk "$@" | tee -a "$OUT" | sed 's/^/   /'
}

# 一次进程一个配置 —— VmHWM 是进程存续期峰值，同进程连测会被前一个配置污染。

echo "==> A. 地板：只连库不查询"
run baseline 0 0 100 2

echo "==> B. 形状判据：批次固定 5000，行数 ×10 —— 峰值该跟着行数走吗？"
for rows in 1000 10000 100000; do run stream "$rows" 5000 100 2; done

echo "==> C. 反证：一次性 fetch 全量（不流式），同样的行数阶梯"
for rows in 1000 10000 100000; do run collect "$rows" 0 100 2; done

echo "==> D. 批次大小的影响：10 万行固定，批次阶梯"
for batch in 500 5000 20000 50000; do run stream 100000 "$batch" 100 2; done

echo "==> E. 驱动取数参数：fetch_array_size / prefetch_rows"
for fas in 10 100 1000 5000; do run stream 100000 5000 "$fas" 2; done
for pf in 0 2 1000; do run stream 100000 5000 100 "$pf"; done

echo "==> F. 走 dblink 的同一条链路（@fa 指回自身）"
run stream 100000 5000 100 2 't_bulk_probe@fa'
run stream 100000 5000 1000 2 't_bulk_probe@fa'

echo
echo "==== 汇总（rss_peak/vmhwm 单位 kB；elapsed 在模拟层上只看趋势）===="
grep '^RESULT' "$OUT" | sed 's/^RESULT //'
rm -f "$OUT"
