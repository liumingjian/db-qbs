#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
if command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files db-qbs-source.service >/dev/null 2>&1; then
  systemctl status db-qbs-source --no-pager
else
  pgrep -af "$ROOT/bin/db-qbs-source" || true
  tail -50 "$ROOT/logs/source.log" 2>/dev/null || true
fi
