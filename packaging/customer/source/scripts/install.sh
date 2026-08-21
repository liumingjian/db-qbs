#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

install -d "$ROOT/bin" "$ROOT/conf" "$ROOT/logs" "$ROOT/data/source"
install -m 0755 "$HERE/bin/db-qbs-source" "$HERE/bin/db-qbs-source-run" "$ROOT/bin/"

if [[ ! -f "$ROOT/conf/source.toml" ]]; then
  install -m 0600 "$HERE/conf/source.toml.example" "$ROOT/conf/source.toml"
  echo "Created $ROOT/conf/source.toml. Please edit it before starting."
else
  echo "Keeping existing $ROOT/conf/source.toml"
fi

if command -v systemctl >/dev/null 2>&1 && [[ -d /etc/systemd/system ]]; then
  install -m 0644 "$HERE/systemd/db-qbs-source.service" /etc/systemd/system/
  systemctl daemon-reload
  echo "Installed systemd service db-qbs-source."
fi

echo "Installed source package under $ROOT"
