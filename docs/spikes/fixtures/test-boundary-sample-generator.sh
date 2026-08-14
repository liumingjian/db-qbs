#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
generator="$fixture_dir/04-gen-boundary-samples.sql"

if ! grep -Fq "'YYYY-MM-DD HH24:MI:SS.FF' || NVL(data_scale, 6)" "$generator"; then
  echo "TIMESTAMP samples must retain the source column's declared fractional precision" >&2
  exit 1
fi

if ! grep -Fq "data_type = 'DATE' OR NVL(data_scale, 0) = 0" "$generator"; then
  echo "DATE and TIMESTAMP(0) samples must use a seconds-only format" >&2
  exit 1
fi

echo "boundary sample generator checks passed"
