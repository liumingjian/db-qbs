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
if grep -Eq '^[[:space:]]*declare[[:space:]]+-A' "$runner"; then
  echo "M1 acceptance runner must support the macOS Bash 3.2 baseline" >&2
  exit 1
fi
if grep -Eq '(^|[^[:alnum:]_])seq[[:space:]]' "$runner"; then
  echo "M1 acceptance runner must not require GNU seq on macOS" >&2
  exit 1
fi
if grep -q 'sha256sum' "$runner"; then
  echo "M1 acceptance runner must use the macOS shasum command" >&2
  exit 1
fi
grep -Fq 'shasum -a 256' "$runner" || {
  echo "M1 acceptance runner must hash target rows with SHA-256" >&2
  exit 1
}
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
wide_body=$(sed -n '/^scenario_wide()/,/^}/p' "$runner")
for assertion in '65535 / 70' 'max_rows > max_rows_per_insert'; do
  grep -Fq "$assertion" <<<"$wide_body" || {
    echo "wide scenario must prove placeholder sub-statement splitting was exercised" >&2
    exit 1
  }
done
[[ $(grep -c 'CONNECT BY LEVEL <= 100000' acceptance/oracle.sql) == 2 ]] || {
  echo "both M1 source fixtures must contain 100000 rows" >&2
  exit 1
}

if grep -En 'rows.*(==|-eq).*5000|5000.*(==|-eq).*rows' "$runner"; then
  echo "acceptance runner must not assume a batch contains 5000 rows" >&2
  exit 1
fi

source_kill_body=$(sed -n '/^scenario_source_kill()/,/^}/p' "$runner")
grep -q 'batch_pushed' <<<"$source_kill_body" || {
  echo "source-kill scenario must reach STREAMING before killing source" >&2
  exit 1
}
for assertion in 'kill-sentinel' 'target_with_sentinel' 'target hash after source kill.*target_with_sentinel'; do
  grep -Eq "$assertion" <<<"$source_kill_body" || {
    echo "source-kill scenario must prove the target was not replaced with identical data" >&2
    exit 1
  }
done

commit_disconnect_body=$(sed -n '/^scenario_commit_disconnect()/,/^}/p' "$runner")
for assertion in 'commit-sentinel' 'commit disconnect target rows.*0'; do
  grep -Eq "$assertion" <<<"$commit_disconnect_body" || {
    echo "commit-disconnect scenario must prove the diagnosed swap reached the target" >&2
    exit 1
  }
done
grep -Fq 'assert_eq "commit disconnect source exit" 1 "$status"' <<<"$commit_disconnect_body" || {
  echo "commit-disconnect scenario must enforce the source 0/1 exit contract" >&2
  exit 1
}

prepare_body=$(sed -n '/^prepare_rig()/,/^}/p' "$runner")
grep -q 'drop_orphan_staging' <<<"$prepare_body" || {
  echo "acceptance setup must remove staging tables left by an interrupted run" >&2
  exit 1
}

cleanup_body=$(sed -n '/^cleanup()/,/^}/p' "$runner")
grep -q 'drop_orphan_staging' <<<"$cleanup_body" || {
  echo "acceptance cleanup must remove staging tables left by fault injection" >&2
  exit 1
}

for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  grep -q "$measurement" "$runner" || {
    echo "acceptance report is missing $measurement" >&2
    exit 1
  }
done
success_body=$(sed -n '/^assert_source_success()/,/^}/p' "$runner")
for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  grep -q "$measurement" <<<"$success_body" || {
    echo "successful runs must require a numeric $measurement measurement" >&2
    exit 1
  }
done
for measurement in rows bytes; do
  grep -Fq ".$measurement" <<<"$success_body" || {
    echo "successful runs must require numeric per-batch $measurement measurements" >&2
    exit 1
  }
done
[[ $(grep -c 'type.*number' <<<"$success_body") -ge 2 ]] || {
  echo "successful-run measurements must be numeric" >&2
  exit 1
}
if grep -Eq '(count_ms|purged_rows) // 0' "$runner"; then
  echo "acceptance report must not turn unavailable commit measurements into zero" >&2
  exit 1
fi
for measurement in count_ms purged_rows; do
  grep -Fq ".$measurement // \"n/a\"" "$runner" || {
    echo "acceptance report must identify unavailable $measurement as n/a" >&2
    exit 1
  }
done
grep -Eq 'batch_pushed.*\.bytes|\.bytes.*batch_pushed' "$runner" || {
  echo "acceptance report is missing the batch byte distribution" >&2
  exit 1
}

report_body=$(sed -n '/^write_report()/,/^}/p' "$runner")
grep -q 'canonical-form\.out' <<<"$report_body" || {
  echo "acceptance report must retain the canonical-form gate output" >&2
  exit 1
}

echo "M1 acceptance script contract: PASS"
