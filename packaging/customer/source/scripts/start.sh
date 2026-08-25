#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && [[ -f /etc/systemd/system/db-qbs-source.service ]]; then
  systemctl enable --now db-qbs-source
else
  nohup "$ROOT/bin/db-qbs-source" --config "$ROOT/conf/source.toml" >> "$ROOT/logs/source.log" 2>&1 &
  echo $! > "$ROOT/logs/source.pid"
fi
