#!/usr/bin/env bash
# 起台架。幂等，重复跑安全。
set -euo pipefail
cd "$(dirname "$0")/.."

wait_healthy() {  # $1=容器名 $2=超时秒
  local name=$1 deadline=$(( SECONDS + $2 )) status
  while :; do
    status=$(docker inspect -f '{{.State.Health.Status}}' "$name" 2>/dev/null || echo starting)
    [[ "$status" == healthy ]] && { echo "    $name healthy"; return 0; }
    (( SECONDS > deadline )) && { echo "!! $name 超时，状态=$status"; docker logs --tail=50 "$name"; return 1; }
    sleep 5
  done
}

echo "==> 起 Oracle XE 11.2.0.2（amd64 模拟）与 MySQL 8.0（arm64 原生）"
docker compose up -d oracle mysql

echo "==> 等健康检查（Oracle 首次建库 + 跑 initdb 脚本，模拟层下可能几分钟）"
wait_healthy qbs-oracle11 900
wait_healthy qbs-mysql8   300

echo "==> 构建并起客户端（arm64 原生 Instant Client 19.32）"
docker compose up -d --build client

echo "==> 冒烟"
exec ./scripts/smoke.sh
