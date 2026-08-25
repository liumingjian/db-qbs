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

for (( scenario = 1; scenario <= 14; scenario++ )); do
  grep -Fq "scenario_a$scenario()" "$runner" || {
    echo "missing scenario_a$scenario implementation" >&2
    exit 1
  }
  grep -Eq "^run_scenario A${scenario}-[^ ]+ scenario_a${scenario}$" "$runner" || {
    echo "scenario A$scenario is not executed" >&2
    exit 1
  }
done

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
for evidence in 'expected_columns' 'run_id' 'history' 'component=sink'; do
  grep -Fq "$evidence" <<<"$a2_body" || {
    echo "A2 must record $evidence evidence" >&2
    exit 1
  }
done
grep -Fq 'sink_log_record_count' <<<"$a2_body" || {
  echo "A2 must count component=sink JSON log records" >&2
  exit 1
}

a5_body=$(sed -n '/^scenario_a5()/,/^}/p' "$runner")
for evidence in 'pause-committing' 'live_projection' 'release-child' 'terminal_projection'; do
  grep -Fq "$evidence" <<<"$a5_body" || {
    echo "A5 must capture $evidence evidence" >&2
    exit 1
  }
done

# A3 / A6 的判据已随 ADR-0036 §5 退役（SQL 形状预检整段取消）。ADR-0040 §5.2 的处置是
# **保留编号、脚本里跳过、报告里打 N/A**——不删号、不重编、不拿别的场景补位。
# 这里守的正是这一条：两个函数必须还在、必须还被 run_scenario 调用、且必须走 N/A 那条出口。
for retired in a3 a6; do
  body=$(sed -n "/^scenario_${retired}()/,/^}/p" "$runner")
  grep -Fq 'return "$SKIPPED_EXIT"' <<<"$body" || {
    echo "${retired} must exit through the N/A path, not pass or fail" >&2
    exit 1
  }
  grep -Fq 'ADR-0036 §5' <<<"$body" || {
    echo "${retired} must name the ADR that retired its criterion" >&2
    exit 1
  }
done
run_scenario_body=$(sed -n '/^run_scenario()/,/^}/p' "$runner")
grep -Fq 'N/A（判据已随 ADR-0036 §5 退役）' <<<"$run_scenario_body" || {
  echo "the runner must record retired criteria as N/A with their provenance" >&2
  exit 1
}
if grep -Fq '.shape_checks | length' "$runner"; then
  echo "the runner must not still assert on the retired shape-precheck report" >&2
  exit 1
fi

# 调用面：TaskSpec + 数据源 id 绑定（ADR-0037 §1/§8）。`run_params` 已随
# 运行参数链一起退役，顶上来的是任务定义里那段 `where_clause`。
for surface in 'source_datasource_id' 'target_datasource_id' 'where_clause' 'narrow_spec'; do
  grep -Fq "$surface" "$runner" || {
    echo "M2 must drive the current call surface: missing $surface" >&2
    exit 1
  }
done
# 匹配的是**报文字段**（`source_sql:` / `biz_date:`），不是散文里提到的名字——
# 抬头那段说明本来就要点名这几个退役字段。
for retired_field in 'source_sql' 'source_date_col' 'target_date_col' 'biz_date'; do
  if grep -Eq "$retired_field[\"']?[:=]" "$runner"; then
    echo "M2 must not send the retired $retired_field payload field" >&2
    exit 1
  fi
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
python3 - acceptance/m2-source-run-wrapper.py acceptance/commit-drop-proxy.py <<'PY'
import ast
import pathlib
import sys

for path in sys.argv[1:]:
    ast.parse(pathlib.Path(path).read_text())
PY

grep -Fq 'docs/design-system/README.md' README.md
grep -Fq 'tokens.css' README.md
grep -Fq 'm2-visual-walkthrough.md' README.md
grep -Fq '**G1**' README.md
grep -Fq '**G2**' README.md
grep -Fq 'm2-visual-walkthrough.md' ../../../../CLAUDE.md
grep -Fq 'docs/design-system/README.md' ../../../../CLAUDE.md
grep -Fq 'docs/design-system/tokens.css' ../../../../CLAUDE.md
grep -Fq 'every M2 acceptance' ../../../../CLAUDE.md
# ADR-0040 §7 把两道视觉门禁段并成了一段三行表，「before merge」这句措辞随之不在了。
# 守的还是同一件事：触发即跑、不许豁免。
grep -Fq 'A trigger fires, you run it' ../../../../CLAUDE.md
grep -Fq 'actual observations' ../../../../CLAUDE.md
