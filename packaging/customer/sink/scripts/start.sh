#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && [[ -f /etc/systemd/system/db-qbs-sink.service ]]; then
  systemctl enable --now db-qbs-sink
else
  nohup "$ROOT/bin/db-qbs-sink" --config "$ROOT/conf/sink.toml" >> "$ROOT/logs/sink.log" 2>&1 &
  echo $! > "$ROOT/logs/sink.pid"
fi
