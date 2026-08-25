#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files db-qbs-sink.service >/dev/null 2>&1; then
  systemctl status db-qbs-sink --no-pager
else
  pgrep -af "$ROOT/bin/db-qbs-sink" || true
  tail -50 "$ROOT/logs/sink.log" 2>/dev/null || true
fi
