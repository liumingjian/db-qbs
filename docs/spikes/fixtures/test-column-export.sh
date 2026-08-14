#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exporter="$fixture_dir/02-columns.sql"

if grep -Fq 'data_default IS NULL' "$exporter"; then
  echo "column export must not apply predicates to Oracle's LONG DATA_DEFAULT field" >&2
  exit 1
fi

if ! grep -Fq "CASE WHEN default_length IS NULL THEN 'N' ELSE 'Y' END" "$exporter"; then
  echo "column export must detect defaults through DEFAULT_LENGTH" >&2
  exit 1
fi

echo "column export checks passed"
