#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

runner=./scripts/run-m1-acceptance.sh
expected=$(cat <<'EOF'
wide-100k
narrow-100k
source-kill-rerun
commit-disconnect
empty-result
verification-failures
canonical-form
EOF
)

bash -n "$runner"
actual=$($runner --list)
[[ "$actual" == "$expected" ]] || {
  echo "unexpected M1 scenario list" >&2
  diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
  exit 1
}

for file in \
  acceptance/oracle.sql \
  acceptance/mysql.sql \
  acceptance/task-wide.toml \
  acceptance/task-narrow.toml \
  acceptance/commit-drop-proxy.py; do
  [[ -f "$file" ]] || { echo "missing $file" >&2; exit 1; }
done

narrow_columns=$(sed -n '/^SELECT /,/^  FROM /p' acceptance/task-narrow.toml | grep -o ' AS ' | wc -l | tr -d ' ')
[[ "$narrow_columns" == 3 ]] || {
  echo "narrow task must select exactly 3 columns, found $narrow_columns" >&2
  exit 1
}

wide_columns=$(sed -n '/^SELECT /,/^  FROM /p' acceptance/task-wide.toml | grep -o ' AS ' | wc -l | tr -d ' ')
[[ "$wide_columns" == 70 ]] || {
  echo "wide task must select exactly 70 columns, found $wide_columns" >&2
  exit 1
}
[[ $(rg -c 'CONNECT BY LEVEL <= 100000' acceptance/oracle.sql) == 2 ]] || {
  echo "both M1 source fixtures must contain 100000 rows" >&2
  exit 1
}

if rg -n 'rows[^\n]*(==|-eq)[^\n]*5000|5000[^\n]*(==|-eq)[^\n]*rows' "$runner"; then
  echo "acceptance runner must not assume a batch contains 5000 rows" >&2
  exit 1
fi

source_kill_body=$(sed -n '/^scenario_source_kill()/,/^}/p' "$runner")
rg -q 'batch_pushed' <<<"$source_kill_body" || {
  echo "source-kill scenario must reach STREAMING before killing source" >&2
  exit 1
}

for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  rg -q "$measurement" "$runner" || {
    echo "acceptance report is missing $measurement" >&2
    exit 1
  }
done
rg -q 'batch_pushed.*\.bytes|\.bytes.*batch_pushed' "$runner" || {
  echo "acceptance report is missing the batch byte distribution" >&2
  exit 1
}

echo "M1 acceptance script contract: PASS"
