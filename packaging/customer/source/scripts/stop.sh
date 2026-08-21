#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files db-qbs-source.service >/dev/null 2>&1; then
  systemctl stop db-qbs-source
elif [[ -f "$ROOT/logs/source.pid" ]]; then
  kill "$(cat "$ROOT/logs/source.pid")"
  rm -f "$ROOT/logs/source.pid"
else
  pkill -f "$ROOT/bin/db-qbs-source --config $ROOT/conf/source.toml" || true
fi
