#!/usr/bin/env bash
# #5 的 3b —— 客户端每行处理开销。幂等，重复跑安全。
#
# 台架边界（README「边界」第 2 条）说吞吐的**绝对秒数**作废，因为服务端在模拟层上。
# 本脚本量的不是墙钟，是 `getrusage(RUSAGE_SELF)` 的 `ru_utime + ru_stime` ——
# **进程真正占用 CPU 的时间，等服务端的部分不计入**，所以模拟层拖慢的是墙钟，不是这个数。
# 客户端是 arm64 原生，因此这一半在台架上成立。
#
# 矩阵两段：
#   1. 行数阶梯 × 四个累进层级 —— 看每行 CPU 是否随行数线性（斜率稳不稳）。
#   2. 列数阶梯 —— 台架表只有 4 列，生产 70+ 列，得知道成本是每行摊还是每单元格摊。
# 每档跑 REPS 次（默认 3），取最小值当代表：CPU 时间的噪声只会往上抬，不会往下压。
set -euo pipefail
cd "$(dirname "$0")/.."

CRATE_DIR="$PWD/spike-bulk"
RUST_IMAGE="rust:1-bookworm"
REPS="${REPS:-3}"

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
run() {  # mode rows batch fas prefetch table ncols
  for _ in $(seq 1 "$REPS"); do
    docker compose exec -T client /usr/local/bin/spike-bulk "$@" | tee -a "$OUT" | grep -E '^客户端 CPU' | sed "s|^|   $* |"
  done
}

# 一次进程一个配置 —— 与内存探针同理，且 getrusage 是进程累计值，同进程连测会叠加。

echo "==> A. 行数阶梯 × 四个累进层级（4 列全取，批次 5000，fas=100）"
for layer in cpu0 cpu1 cpu2 cpu3; do
  for rows in 1000 10000 100000; do
    echo "-- $layer rows=$rows"
    run "$layer" "$rows" 5000 100 2 t_bulk_probe 4
  done
done

echo "==> B. 列数阶梯（10 万行固定）—— 成本是每行摊还是每单元格摊"
for layer in cpu1 cpu3; do
  for ncols in 1 2 3 4; do
    echo "-- $layer ncols=$ncols"
    run "$layer" 100000 5000 100 2 t_bulk_probe "$ncols"
  done
done

echo "==> C. 对照：完整路径走 dblink（@fa），客户端侧该一模一样"
echo "-- cpu3 @fa"
run cpu3 100000 5000 100 2 't_bulk_probe@fa' 4

echo
echo "==== 汇总：每档取 REPS 次的**中位数**（CPU 时间的噪声只往上抬，中位数比均值稳）===="
printf '%-6s %-8s %-6s %-16s %10s %10s %12s\n' mode rows ncols table cpu_us wall_us cpu_ns/row
grep '^RESULT' "$OUT" | awk '
{
  for (i = 2; i <= NF; i++) { split($i, kv, "="); v[kv[1]] = kv[2] }
  key = v["mode"] " " v["rows"] " " v["ncols"] " " v["table"]
  cpu[key] = cpu[key] " " v["cpu_total_us"]
  wall[key] = wall[key] " " v["wall_us"]
  rows[key] = v["rows"]
}
function median(list,   n, a, i) {
  n = split(list, a, " ")
  for (i = 1; i < n; i++) for (j = i + 1; j <= n; j++) if (a[j] + 0 < a[i] + 0) { t = a[i]; a[i] = a[j]; a[j] = t }
  return (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
}
END {
  for (k in cpu) {
    split(k, f, " ")
    c = median(cpu[k]); w = median(wall[k])
    printf "%-6s %-8s %-6s %-16s %10d %10d %12.0f\n", f[1], f[2], f[3], f[4], c, w, c * 1000 / rows[k]
  }
}' | sort -k4,4 -k1,1 -k3,3n -k2,2n
rm -f "$OUT"
