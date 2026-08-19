#!/usr/bin/env bash
# #152 —— 起演练台的两台「主机」容器（源端 / 目标端）。幂等，重复跑安全。
#
# 两台主机在 compose 的 `rehearsal` profile 下：默认 `up.sh` 不动它们，
# 四份既有台架的起停一个字节不变（#152 判据「既有台架不受影响」）。
#
# 前提：Oracle / MySQL 已经起着（源端主机要够得着 Oracle，目标端主机要够得着 MySQL）。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 确认两个库在跑（演练台挂在它们两侧）"
for c in qbs-oracle11 qbs-mysql8; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/up.sh"; exit 1; }
done

echo "==> 起两台 centos:7 主机容器（amd64 —— 与客户机同架构，见 README）"
docker compose --profile rehearsal up -d host-source host-target

docker compose --profile rehearsal ps host-source host-target
echo "== 演练台就位；拓扑判据跑 ./scripts/rehearsal-topology-check.sh =="
