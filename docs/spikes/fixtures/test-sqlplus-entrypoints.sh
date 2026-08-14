#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

for script in \
  01-env-facts.sql \
  02-columns.sql \
  03-type-census.sql \
  04-gen-boundary-samples.sql
do
  last_command=$(
    sed 's/--.*$//' "$fixture_dir/$script" \
      | sed '/^[[:space:]]*$/d' \
      | tail -n 1
  )

  if [[ ! "$last_command" =~ ^[[:space:]]*EXIT([[:space:]]*\;)?[[:space:]]*$ ]]; then
    echo "$script must end with an explicit EXIT command" >&2
    exit 1
  fi
done

echo "SQL*Plus entrypoint checks passed"
