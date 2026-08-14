#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

command_docs=(
  "$fixture_dir/README.md"
  "$fixture_dir/01-env-facts.sql"
  "$fixture_dir/02-columns.sql"
  "$fixture_dir/03-type-census.sql"
  "$fixture_dir/04-gen-boundary-samples.sql"
)

non_failing_logins=$(
  grep -nH 'sqlplus ' "${command_docs[@]}" \
    | grep -v 'sqlplus -L ' \
    || true
)

if [[ -n "$non_failing_logins" ]]; then
  echo "customer collection commands must use SQL*Plus -L to fail after one login attempt:" >&2
  echo "$non_failing_logins" >&2
  exit 1
fi

non_utf8_clients=$(
  grep -nH 'sqlplus ' "${command_docs[@]}" \
    | grep -Fv 'NLS_LANG=AMERICAN_AMERICA.AL32UTF8 sqlplus -L ' \
    || true
)

if [[ -n "$non_utf8_clients" ]]; then
  echo "customer collection commands must make SQL*Plus emit UTF-8:" >&2
  echo "$non_utf8_clients" >&2
  exit 1
fi

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
