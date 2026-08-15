#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

runner=./scripts/run-m2-acceptance.sh
expected=$(cat <<'EOF'
A1-start-stop-readiness
A2-task-column-fetch
A3-column-fetch-shape-failure
A4-column-fetch-oracle-failure
A5-success-projection-history
A6-run-shape-failure
A7-mapping-precheck-failure
A8-verification-failure
A9-sentinel-escape
A10-concurrent-rejection
A11-committing-cancel-rejection
A12-process-disappeared
A13-service-restarted
A14-detail-lifecycle
EOF
)

bash -n "$runner"
actual=$($runner --list)
[[ "$actual" == "$expected" ]] || {
  echo "unexpected M2 scenario list" >&2
  diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
  exit 1
}

if grep -Eq '^[[:space:]]*declare[[:space:]]+-A' "$runner"; then
  echo "M2 acceptance runner must support the macOS Bash 3.2 baseline" >&2
  exit 1
fi

start_source_body=$(sed -n '/^start_source()/,/^}/p' "$runner")
grep -Fq '"$SOURCE_BIN" --config "$SOURCE_CONFIG"' <<<"$start_source_body" || {
  echo "M2 source must start as a host background process" >&2
  exit 1
}
if grep -q 'compose.*db-qbs-source' <<<"$start_source_body"; then
  echo "M2 source must not run under docker compose" >&2
  exit 1
fi
grep -Fq 'GET /api/tasks' "$runner" || {
  echo "M2 readiness must use GET /api/tasks" >&2
  exit 1
}
grep -Fq 'kill -TERM "$SOURCE_PID"' "$runner" || {
  echo "M2 cleanup must try SIGTERM first" >&2
  exit 1
}
grep -Fq 'kill -KILL "$SOURCE_PID"' "$runner" || {
  echo "M2 cleanup must have a SIGKILL fallback" >&2
  exit 1
}

a2_body=$(sed -n '/^scenario_a2()/,/^}/p' "$runner")
for evidence in 'columns' 'run_id' 'history' 'component=sink'; do
  grep -Fq "$evidence" <<<"$a2_body" || {
    echo "A2 must record $evidence evidence" >&2
    exit 1
  }
done

grep -Fq 'commit-drop-proxy.py' "$runner" || {
  echo "A11 must reuse the existing commit-drop proxy" >&2
  exit 1
}
grep -Fq 'projection-versus-history' "$runner" || {
  echo "A5 must report the six-scalar projection/history comparison" >&2
  exit 1
}
grep -Fq 'm2-acceptance-' "$runner" || {
  echo "M2 report must use the specified filename" >&2
  exit 1
}
if grep -Eq 'Measurements|performance|p50|Push/cursor' "$runner"; then
  echo "M2 report must not collect performance measurements" >&2
  exit 1
fi

for file in \
  acceptance/m2-source-run-wrapper.py \
  acceptance/commit-drop-proxy.py \
  m2-visual-walkthrough.md; do
  [[ -f "$file" ]] || { echo "missing $file" >&2; exit 1; }
done

grep -Fq 'docs/design-system/README.md' README.md
grep -Fq 'tokens.css' README.md
grep -Fq 'm2-visual-walkthrough.md' README.md
