#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
generator_file="$fixture_dir/04-gen-boundary-samples.sql"

# Assert generated SQL fragments without coupling the checks to source line wrapping.
normalized_generator=$(tr -s '[:space:]' ' ' < "$generator_file")

generator_contains() {
  grep -Fq "$1" <<< "$normalized_generator"
}

max_length_selector="MAX(TO_CHAR(' || column_name || ')) KEEP "\
"(DENSE_RANK LAST ORDER BY LENGTH(TO_CHAR(' || column_name || ')))"
max_scale_selector="MAX(TO_CHAR(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name"\
" || ') THEN ' || column_name || ' END)) KEEP (DENSE_RANK LAST ORDER BY "\
"NVL(LENGTH(TO_CHAR(CASE WHEN ' || column_name || ' <> TRUNC(' || column_name"\
" || ') THEN ABS(' || column_name || ') - TRUNC(ABS(' || column_name || ')) END)), 0))"

if ! generator_contains "PROMPT SET LINESIZE 32767" \
  || ! generator_contains "PROMPT SET MARKUP CSV ON QUOTE ON" \
  || ! generator_contains "kind KIND, NVL(v,' || null_marker || ') VALUE, NVL(LENGTH(v),0) VALUE_LEN" \
  || generator_contains "||NVL(v,''<NULL>'')"; then
  echo "generated sample output must use SQL*Plus CSV quoting instead of concatenating raw values" >&2
  exit 1
fi

if ! generator_contains "$max_length_selector"; then
  echo "NUMBER max-length samples must contain the actual value, not only its length" >&2
  exit 1
fi

if ! generator_contains "$max_scale_selector"; then
  echo "NUMBER max-scale samples must select the value with the longest fractional part" >&2
  exit 1
fi

if ! generator_contains "'YYYY-MM-DD HH24:MI:SS.FF' || NVL(data_scale, 6)"; then
  echo "TIMESTAMP samples must retain the source column's declared fractional precision" >&2
  exit 1
fi

if ! generator_contains "data_type = 'DATE' OR NVL(data_scale, 0) = 0"; then
  echo "DATE and TIMESTAMP(0) samples must use a seconds-only format" >&2
  exit 1
fi

if generator_contains "data_type LIKE 'TIMESTAMP%'" \
  || ! generator_contains "data_type LIKE 'TIMESTAMP(%)'"; then
  echo "boundary samples must exclude unsupported time-zone TIMESTAMP variants" >&2
  exit 1
fi

if ! generator_contains "LENGTH(ASCIISTR(' || column_name || ')) > LENGTH(' || column_name" \
  || generator_contains "ASCIISTR(' || column_name || ') <> ' || column_name" \
  || generator_contains "LENGTHB(' || column_name || ') > LENGTH(' || column_name"; then
  echo "character samples must detect non-ASCII content rather than national-character storage width" >&2
  exit 1
fi

if ! generator_contains "CASE WHEN data_type IN ('NVARCHAR2','NCHAR') THEN 'TO_NCHAR'" \
  || ! generator_contains "|| text_cast || '(COUNT(CASE WHEN '" \
  || ! generator_contains "THEN 'TO_NCHAR(''<NULL>'')'"; then
  echo "national-character samples must keep counts and null markers in the national character set" >&2
  exit 1
fi

echo "boundary sample generator checks passed"
