#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

runner=./scripts/run-m3-acceptance.sh
expected=$(cat <<'EOF'
B1-nine-shape-round-trip
B2-all-mapping-rejections
B3-timestamp-fsp-rejection
B4-bare-number-range-rejection
B5-no-range-check
B6-bc-date-source-value
EOF
)

bash -n "$runner"
actual=$($runner --list)
[[ "$actual" == "$expected" ]] || {
  echo "unexpected M3 scenario list" >&2
  diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
  exit 1
}

for scenario in b1 b2 b3 b4 b5 b6; do
  grep -Fq "scenario_${scenario}()" "$runner" || {
    echo "missing scenario_${scenario} implementation" >&2
    exit 1
  }
done

for invocation in \
  'run_scenario B1-nine-shape-round-trip scenario_b1' \
  'run_scenario B2-all-mapping-rejections scenario_b2' \
  'run_scenario B3-timestamp-fsp-rejection scenario_b3' \
  'run_scenario B4-bare-number-range-rejection scenario_b4' \
  'run_scenario B5-no-range-check scenario_b5' \
  'run_scenario B6-bc-date-source-value scenario_b6'; do
  grep -Fq "$invocation" "$runner" || {
    echo "missing M3 scenario invocation: $invocation" >&2
    exit 1
  }
done

for forbidden in 'declare -A' ' sha256sum' ' seq '; do
  if grep -Fq "$forbidden" "$runner"; then
    echo "M3 acceptance runner must support the macOS Bash 3.2 baseline" >&2
    exit 1
  fi
done

start_source_body=$(sed -n '/^start_source()/,/^}/p' "$runner")
grep -Fq 'nohup "$SOURCE_BIN" --config "$SOURCE_CONFIG"' <<<"$start_source_body" || {
  echo "M3 source must start as a host background process" >&2
  exit 1
}
if grep -q 'compose.*db-qbs-source' <<<"$start_source_body"; then
  echo "M3 source must not run under docker compose" >&2
  exit 1
fi
grep -Fq 'GET /api/tasks' "$runner" || {
  echo "M3 readiness must use GET /api/tasks" >&2
  exit 1
}
grep -Fq 'kill -TERM "$SOURCE_PID"' "$runner" || {
  echo "M3 cleanup must try SIGTERM first" >&2
  exit 1
}
grep -Fq 'kill -KILL "$SOURCE_PID"' "$runner" || {
  echo "M3 cleanup must have a SIGKILL fallback" >&2
  exit 1
}
grep -Fq 'run_executable = "$REAL_SOURCE_BIN"' "$runner" || {
  echo "M3 must use the real one-shot source runner" >&2
  exit 1
}

for fixture in acceptance/oracle-m3.sql acceptance/mysql-m3.sql; do
  [[ -f "$fixture" ]] || { echo "missing $fixture" >&2; exit 1; }
done
[[ -f m3-visual-walkthrough.md ]] || {
  echo "missing m3-visual-walkthrough.md" >&2
  exit 1
}
grep -Fq './scripts/up.sh' "$runner" || {
  echo "M3 must use the existing self-contained rig bootstrap" >&2
  exit 1
}
if grep -Fq 'up-m3' "$runner"; then
  echo "M3 must not introduce a second rig bootstrap" >&2
  exit 1
fi
grep -Fq 'acceptance/oracle-m3.sql' "$runner"
grep -Fq 'mysql-m3.sql' "$runner"
if grep -Fq 'acceptance/oracle.sql' "$runner" || grep -Fq 'acceptance/mysql.sql' "$runner"; then
  echo "M3 must load standalone fixtures" >&2
  exit 1
fi

for shape in \
  'NUMBER(38,2)' 'NUMBER(4,6)' 'NUMBER(8,-2)' 'n_bare       NUMBER' \
  'VARCHAR2(10 CHAR)' 'NVARCHAR2(10)' 'CHAR(10 CHAR)' 'NCHAR(10)' \
  'DATE' 'TIMESTAMP(0)' 'TIMESTAMP(3)' 'TIMESTAMP(6)'; do
  grep -Fq "$shape" acceptance/oracle-m3.sql || {
    echo "B1 fixture is missing $shape" >&2
    exit 1
  }
done
for value in \
  '123456789012345678901234567890123456.78' \
  '0.000001' '-0.000001' '0.009999' '-0.009999' \
  '9999999900' '-9999999900' '12300' '12400' \
  '9999-12-31 23:59:59' '0044-01-01' '0999-12-31'; do
  grep -Fq -- "$value" "$runner" acceptance/oracle-m3.sql || {
    echo "B1 matrix is missing $value" >&2
    exit 1
  }
done
grep -Fq 'N_EXPR' "$runner"
grep -Fq 'column_precision' "$runner"
grep -Fq 'HEX(' "$runner"

grep -Fq 'for column in BF BD PAYLOAD C_EXPR C_CHAR N_TOO_WIDE N_TOO_SCALE N_MISSING D_WRONG EXTRA; do' "$runner" || {
  echo "B2 is missing one or more invalid-column assertions" >&2
  exit 1
}
grep -Fq '一次发现 $total 项问题' "$runner"
grep -Fq 'B2 total issues' "$runner"
grep -Fq 'staging tables' "$runner"
grep -Fq 'TIMESTAMP(n>6) 不在白名单' "$runner"
grep -Fq 'range_check_executed' "$runner"
grep -Fq 'scanned_rows' "$runner"
grep -Fq 'range_ms' "$runner"
grep -Fq 'B4 invalid rows' "$runner"
grep -Fq 'B5 range checks' "$runner"
grep -Fq 'B6 outcome' "$runner"
grep -Fq 'B6 failure kind' "$runner"
grep -Fq 'B6 column' "$runner"
grep -Fq 'B6 original value present' "$runner"

grep -Fq 'M3_KEEP_RIG' "$runner"
grep -Fq 'B2 / W1-W2 run' "$runner"
grep -Fq 'B4 / W6 run' "$runner"
grep -Fq 'W3-W4 builder SQL' "$runner"
grep -Fq 'W5 builder SQL' "$runner"
grep -Fq 'm3-acceptance-' "$runner"
grep -Fq 'G1:' "$runner"
grep -Fq 'scripts/run-canon-gate.sh unchanged' "$runner"
grep -Fq 'W1-W6:' "$runner"
if grep -Eq 'Measurements|p50|fetch_ms|push_ms|commit_ms|count_ms|cursor_ms|通过|通過' "$runner"; then
  echo "M3 report must contain assertions, not generic performance or pass claims" >&2
  exit 1
fi

grep -Fq 'run-m3-acceptance.sh' README.md
grep -Fq 'm3-visual-walkthrough.md' README.md
grep -Fq 'G1' README.md

echo 'M3 acceptance static checks: PASS'
