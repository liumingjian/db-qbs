#!/usr/bin/env bash
# #35 MySQL DATETIME 值域实测。只需要 MySQL，不起 Oracle / client（省掉模拟层那几分钟）。
# 幂等，重复跑安全。
set -euo pipefail
cd "$(dirname "$0")/.."

SQL_FILE=${1:-probes/mysql-datetime-domain.sql}

echo "==> 起 MySQL 8.0（arm64 原生）"
docker compose up -d mysql

echo "==> 等健康检查"
deadline=$(( SECONDS + 300 ))
while :; do
  status=$(docker inspect -f '{{.State.Health.Status}}' qbs-mysql8 2>/dev/null || echo starting)
  [[ "$status" == healthy ]] && { echo "    qbs-mysql8 healthy"; break; }
  (( SECONDS > deadline )) && { echo "!! qbs-mysql8 超时，状态=$status"; docker logs --tail=50 qbs-mysql8; exit 1; }
  sleep 5
done

echo "==> 跑 $SQL_FILE"
# --force：组 B 那三条**预期会报错**，报错本身就是结论，不能因此中断整轮。
# --show-warnings：静默改值只以 warning / note 形式出现，必须打出来。
docker compose exec -T mysql \
  mysql -uspike -pspike123 qbs --default-character-set=utf8mb4 --table --force --show-warnings < "$SQL_FILE" 2>&1 \
  | grep -v '^mysql: \[Warning\] Using a password'
