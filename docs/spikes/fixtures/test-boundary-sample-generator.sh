#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
generator="$fixture_dir/04-gen-boundary-samples.sql"

if ! grep -Fq "MAX(TO_CHAR(' || column_name || ')) KEEP (DENSE_RANK LAST ORDER BY LENGTH(TO_CHAR(' || column_name || ')))" "$generator"; then
  echo "NUMBER max-length samples must contain the actual value, not only its length" >&2
  exit 1
fi

if ! grep -Fq "MAX(TO_CHAR(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name || ') THEN ' || column_name || ' END)) KEEP (DENSE_RANK LAST ORDER BY NVL(LENGTH(TO_CHAR(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name || ') THEN ABS(' || column_name || ') - TRUNC(ABS(' || column_name || ')) END)), 0))" "$generator"; then
  echo "NUMBER max-scale samples must select the value with the longest fractional part" >&2
  exit 1
fi

if ! grep -Fq "'YYYY-MM-DD HH24:MI:SS.FF' || NVL(data_scale, 6)" "$generator"; then
  echo "TIMESTAMP samples must retain the source column's declared fractional precision" >&2
  exit 1
fi

if ! grep -Fq "data_type = 'DATE' OR NVL(data_scale, 0) = 0" "$generator"; then
  echo "DATE and TIMESTAMP(0) samples must use a seconds-only format" >&2
  exit 1
fi

echo "boundary sample generator checks passed"
