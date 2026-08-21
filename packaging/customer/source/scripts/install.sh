#!/usr/bin/env bash
set -euo pipefail

ROOT=${DB_QBS_HOME:-/opt/tools/db-qbs}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

install -d "$ROOT/bin" "$ROOT/conf" "$ROOT/logs" "$ROOT/data/source"
install -m 0755 "$HERE/bin/db-qbs-source" "$HERE/bin/db-qbs-source-run" "$ROOT/bin/"

oracle_zip=$(find "$HERE/oracle" -maxdepth 1 -type f -name 'instantclient-basic-linux.x64-19*.zip' 2>/dev/null | sort | tail -1 || true)
if [[ -n "$oracle_zip" ]]; then
  if ! command -v unzip >/dev/null 2>&1; then
    echo "Found $oracle_zip, but unzip is not installed. Install unzip and rerun this script." >&2
    exit 1
  fi
  install -d "$ROOT/oracle"
  unzip -oq "$oracle_zip" -d "$ROOT/oracle"
  client_dir=$(find "$ROOT/oracle" -maxdepth 1 -type d -name 'instantclient_19_*' | sort | tail -1 || true)
  if [[ -z "$client_dir" ]]; then
    echo "Oracle Instant Client archive was extracted, but instantclient_19_* was not found." >&2
    exit 1
  fi
  ln -sfn "$client_dir" "$ROOT/oracle/instantclient"
  echo "$ROOT/oracle/instantclient" > /etc/ld.so.conf.d/db-qbs-oracle.conf
  ldconfig
  if ldd "$ROOT/oracle/instantclient/libclntsh.so" 2>/dev/null | grep -q 'not found'; then
    echo "Oracle Instant Client still has missing dependencies. On CentOS/RHEL, install libaio." >&2
  fi
  echo "Installed Oracle Instant Client from $(basename "$oracle_zip")."
else
  echo "No oracle/instantclient-basic-linux.x64-19*.zip in this package; keeping existing Oracle Client."
fi

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
