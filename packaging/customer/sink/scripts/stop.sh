#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files db-qbs-sink.service >/dev/null 2>&1; then
  systemctl stop db-qbs-sink
elif [[ -f "$ROOT/logs/sink.pid" ]]; then
  kill "$(cat "$ROOT/logs/sink.pid")"
  rm -f "$ROOT/logs/sink.pid"
else
  pkill -f "$ROOT/bin/db-qbs-sink --config $ROOT/conf/sink.toml" || true
fi
