#!/usr/bin/env bash
# 在客户端容器里开一个 sqlplus。带参数则当作 SQL 执行完即退。
#   ./scripts/sqlplus.sh                      # 交互
#   ./scripts/sqlplus.sh "SELECT * FROM v\$version;"
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -eq 0 ]]; then
  exec docker compose exec client sqlplus spike/spike123@//oracle:1521/XE
else
  exec docker compose exec -T client bash -c \
    "printf '%s\n' \"\$1\" 'EXIT' | sqlplus -S spike/spike123@//oracle:1521/XE" _ "$1"
fi
