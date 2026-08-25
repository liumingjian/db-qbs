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

start_sink_body=$(sed -n '/^start_sink()/,/^}/p' "$runner")
grep -Fq 'kill -0 "$(cat /tmp/m3-sink.pid)"' <<<"$start_sink_body" || {
  echo "M3 sink readiness must verify the process it just started" >&2
  exit 1
}
grep -Fq 'stop_all_sinks || return 1' "$runner" || {
  echo "M3 preparation must remove stale sinks from earlier acceptance runs" >&2
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

while IFS= read -r shape; do
  precision=${shape#DECIMAL(}
  precision=${precision%%,*}
  scale=${shape##*,}
  scale=${scale%)}
  if (( precision > 65 || scale > 30 || scale > precision )); then
    echo "M3 MySQL fixture contains an invalid $shape declaration" >&2
    exit 1
  fi
done < <(grep -Eo 'DECIMAL\([0-9]+,[0-9]+\)' acceptance/mysql-m3.sql)
# 写入模型是 upsert（ADR-0035 §2）：目标端必须真有列集合与勾选主键一致的唯一约束，
# 否则 `ON DUPLICATE KEY UPDATE` 静默退化成纯 INSERT、重跑就出重复行。
[[ $(grep -c 'PRIMARY KEY (ROW_ID)' acceptance/mysql-m3.sql) == 6 ]] || {
  echo "all six M3 target tables must carry a PRIMARY KEY on ROW_ID" >&2
  exit 1
}
if grep -Eq '^[[:space:]]*ROW_ID[[:space:]]+DECIMAL\([0-9]+,[0-9]+\) NULL' acceptance/mysql-m3.sql; then
  echo "M3 primary-key columns must be NOT NULL: MySQL UNIQUE allows many NULLs" >&2
  exit 1
fi
if grep -Fq 'C_EXPR' acceptance/mysql-m3.sql; then
  echo "C_EXPR lost its object with ADR-0036 §2/§5; the target column must go with it" >&2
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
grep -Fq 'HEX(' "$runner"
# 调用面：TaskSpec + 数据源 id 绑定（ADR-0037 §1/§8）。`run_params` 已随
# 运行参数链一起退役，顶上来的是任务定义里那段 `where_clause`。
for surface in 'source_datasource_id' 'target_datasource_id' 'where_clause' 'b1_spec'; do
  grep -Fq "$surface" "$runner" || {
    echo "M3 must drive the current call surface: missing $surface" >&2
    exit 1
  }
done
# 匹配的是**报文字段**（`source_sql:` / `biz_date:`），不是散文里提到的名字——
# 抬头那段说明本来就要点名这几个退役字段。
for retired_field in 'source_sql' 'source_date_col' 'target_date_col' 'biz_date'; do
  if grep -Eq "$retired_field[\"']?[:=]" "$runner"; then
    echo "M3 must not send the retired $retired_field payload field" >&2
    exit 1
  fi
done
# ADR-0040 §5.3：B1 的哨兵从「被删除」翻成「留存」。
b1_body=$(sed -n '/^scenario_b1()/,/^}/p' "$runner")
grep -Fq 'B1 sentinel retained' <<<"$b1_body" || {
  echo "B1 must assert the sentinel survives: upsert never touches a key it did not fetch" >&2
  exit 1
}
if grep -Fq 'B1 sentinel removed' <<<"$b1_body"; then
  echo "B1 still asserts the retired DELETE-era sentinel removal" >&2
  exit 1
fi

for mapping_rule in \
  "BF '源类型不在 M3 九行白名单内'" \
  "BD '源类型不在 M3 九行白名单内'" \
  "PAYLOAD '源类型不在 M3 九行白名单内'" \
  "C_CHAR '字符族目标类型必须是 VARCHAR'" \
  "N_TOO_WIDE 'MySQL DECIMAL 无法表达推导形状 DECIMAL(68,0)'" \
  "N_TOO_SCALE 'MySQL DECIMAL 无法表达推导形状 DECIMAL(35,35)'" \
  "N_MISSING '目标表缺少同名列'" \
  "D_WRONG 'DATE 的目标类型必须是 DATETIME'"; do
  grep -Fq "assert_mapping_rule $mapping_rule" "$runner" || {
    echo "B2 is missing its expected mapping rule: $mapping_rule" >&2
    exit 1
  }
done
grep -Fq '一次发现 $total 项问题' "$runner"
grep -Fq 'B2 total issues' "$runner"
# EXTRA 的判据方向反了（ADR-0038 §4 把列名集合完全相等撤成子集判定）：未被映射的可空列
# 不再报 `源端结果缺少同名列`。编号与场景都不动，断言就地改成「它一条问题都不出」。
grep -Fq 'B2 EXTRA is no longer an issue' "$runner" || {
  echo "B2 must assert the subset rule: an unmapped nullable target column is not an issue" >&2
  exit 1
}
# C_EXPR / N_EXPR 失去对象（表达式列在 v1 进不来），照实记 N/A 并写明谁判废的，不许写「通过」。
for retired in 'B1 numeric expression: N/A' 'B2 C_EXPR: N/A'; do
  grep -Fq "$retired" "$runner" || {
    echo "M3 must record the retired expression-column criterion as N/A: $retired" >&2
    exit 1
  }
done
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
grep -Fq 'kill -TERM "\$(cat /tmp/m3-sink.pid)"' "$runner"
if grep -Fq 'pkill db-qbs-sink' "$runner"; then
  echo "M3 teardown must use tools available in the client image" >&2
  exit 1
fi
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
