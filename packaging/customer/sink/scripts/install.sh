#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

install -d -m 0755 "$ROOT/bin"
install -d -m 0700 "$ROOT/conf" "$ROOT/logs"
install -m 0755 "$HERE/bin/db-qbs-sink" "$ROOT/bin/"

if [[ ! -f "$ROOT/conf/sink.toml" ]]; then
  install -m 0600 "$HERE/conf/sink.toml.example" "$ROOT/conf/sink.toml"
  echo "Created $ROOT/conf/sink.toml. Please edit it before starting."
else
  echo "Keeping existing $ROOT/conf/sink.toml"
fi

if command -v systemctl >/dev/null 2>&1 && [[ -d /etc/systemd/system ]]; then
  install -m 0644 "$HERE/systemd/db-qbs-sink.service" /etc/systemd/system/
  systemctl daemon-reload
  echo "Installed systemd service db-qbs-sink."
fi

echo "Installed sink package under $ROOT"
