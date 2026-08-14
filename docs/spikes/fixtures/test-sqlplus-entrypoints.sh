#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

command_sources=(
  "$fixture_dir/README.md"
  "$fixture_dir/01-env-facts.sql"
  "$fixture_dir/02-columns.sql"
  "$fixture_dir/03-type-census.sql"
  "$fixture_dir/04-gen-boundary-samples.sql"
)

commands_without_fail_fast=$(
  grep -nH 'sqlplus ' "${command_sources[@]}" \
    | grep -v 'sqlplus -L ' \
    || true
)

if [[ -n "$commands_without_fail_fast" ]]; then
  echo "customer collection commands must use SQL*Plus -L to fail after one login attempt:" >&2
  echo "$commands_without_fail_fast" >&2
  exit 1
fi

commands_without_utf8=$(
  grep -nH 'sqlplus ' "${command_sources[@]}" \
    | grep -Fv 'NLS_LANG=AMERICAN_AMERICA.AL32UTF8 sqlplus -L ' \
    || true
)

if [[ -n "$commands_without_utf8" ]]; then
  echo "customer collection commands must make SQL*Plus emit UTF-8:" >&2
  echo "$commands_without_utf8" >&2
  exit 1
fi

for script in \
  01-env-facts.sql \
  02-columns.sql \
  03-type-census.sql \
  04-gen-boundary-samples.sql
do
  if grep -Fq 'WHENEVER SQLERROR EXIT SQL.SQLCODE' "$fixture_dir/$script" \
    || ! grep -Fq 'WHENEVER SQLERROR EXIT FAILURE' "$fixture_dir/$script"; then
    echo "$script must use a fixed nonzero exit status for SQL errors" >&2
    exit 1
  fi

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

if ! grep -Fq 'PROMPT WHENEVER SQLERROR EXIT FAILURE' "$fixture_dir/04-gen-boundary-samples.sql"; then
  echo "generated boundary sample script must use a fixed nonzero exit status for SQL errors" >&2
  exit 1
fi

echo "SQL*Plus entrypoint checks passed"
