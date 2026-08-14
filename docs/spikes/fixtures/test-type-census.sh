#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
census_file="$fixture_dir/03-type-census.sql"

if ! grep -Fq "AND NOT (data_type IN ('NUMBER','DATE','VARCHAR2','NVARCHAR2','CHAR','NCHAR')" "$census_file"; then
  echo "unsupported-type census must reject the complement of the ADR-0003 whitelist" >&2
  exit 1
fi

if grep -Fq "data_type IN ('LONG','LONG RAW'" "$census_file"; then
  echo "unsupported-type census must not rely on an incomplete list of known Oracle types" >&2
  exit 1
fi

if grep -Fq "OR data_type LIKE 'TIMESTAMP%')" "$census_file"; then
  echo "ordinary TIMESTAMP columns are in the ADR-0003 whitelist and must not be flagged" >&2
  exit 1
fi

if ! grep -Fq "OR data_type LIKE 'TIMESTAMP(%)'" "$census_file"; then
  echo "only ordinary TIMESTAMP(n) columns may be included in the whitelist" >&2
  exit 1
fi

echo "type census checks passed"
