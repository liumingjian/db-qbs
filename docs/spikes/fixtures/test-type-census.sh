#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
census="$fixture_dir/03-type-census.sql"

if grep -Fq "OR data_type LIKE 'TIMESTAMP%');" "$census"; then
  echo "ordinary TIMESTAMP columns are in the ADR-0003 whitelist and must not be flagged" >&2
  exit 1
fi

if ! grep -Fq "OR data_type LIKE 'TIMESTAMP%WITH TIME ZONE'" "$census" \
  || ! grep -Fq "OR data_type LIKE 'TIMESTAMP%WITH LOCAL TIME ZONE'" "$census"; then
  echo "timezone-bearing TIMESTAMP variants must remain in the unsupported-type census" >&2
  exit 1
fi

echo "type census checks passed"
