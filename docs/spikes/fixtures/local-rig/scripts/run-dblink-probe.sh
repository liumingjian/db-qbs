#!/usr/bin/env bash
# 跑 #6 的 dblink 列投影探针。台架需已 up。
set -euo pipefail
cd "$(dirname "$0")/.."
# 探针要读 v$mystat / v$statname，先补权限（幂等）。
docker compose exec -T client bash -c \
  "printf '%s\n' 'GRANT SELECT ANY DICTIONARY TO spike;' 'EXIT' | sqlplus -S system/spike123@//oracle:1521/XE" >/dev/null
exec docker compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE @/workspace/docs/spikes/fixtures/local-rig/probes/${1:-dblink-pushdown.sql}
