#!/usr/bin/env bash
# 第一版验收入口的静态自检 —— 与 test-m{1,2,3}-acceptance.sh 同一职责：
# 不起台架，只守住那些「跑起来才发现就太晚」的结构性约定。
set -euo pipefail
cd "$(dirname "$0")/.."

runner=./scripts/run-v1-acceptance.sh
wrapper=./acceptance/v1-memory-wrapper.py
expected=$(cat <<'EOF'
C1-datasource-crud
C2-column-mapping
C3-user-conditions
C4-upsert-idempotence
C5-precheck-branches
C6-memory-shape
EOF
)

bash -n "$runner"
actual=$($runner --list)
[[ "$actual" == "$expected" ]] || {
  echo "unexpected v1 scenario list" >&2
  diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
  exit 1
}

for scenario in c1 c2 c3 c4 c5 c6; do
  grep -Fq "scenario_${scenario}()" "$runner" || {
    echo "missing scenario_${scenario} implementation" >&2
    exit 1
  }
done

for invocation in \
  'run_scenario C1-datasource-crud scenario_c1' \
  'run_scenario C2-column-mapping scenario_c2' \
  'run_scenario C3-user-conditions scenario_c3' \
  'run_scenario C4-upsert-idempotence scenario_c4' \
  'run_scenario C5-precheck-branches scenario_c5' \
  'run_scenario C6-memory-shape scenario_c6'; do
  grep -Fq "$invocation" "$runner" || {
    echo "missing v1 scenario invocation: $invocation" >&2
    exit 1
  }
done

# 编号字母全局唯一（ADR-0040 §2）：C 系列不许出现别人的编号，也不许自造 C7。
if grep -Eq 'run_scenario (A|B)[0-9]' "$runner"; then
  echo "v1 runner must not reuse the M2/M3 scenario numbering" >&2
  exit 1
fi
if grep -Fq 'scenario_c7' "$runner"; then
  echo "there is no C7: 10w/100M is M1 wide-100k + C6 (ADR-0040 §1/§4)" >&2
  exit 1
fi

for forbidden in 'declare -A' ' sha256sum' ' seq '; do
  if grep -Fq "$forbidden" "$runner"; then
    echo "v1 acceptance runner must support the macOS Bash 3.2 baseline" >&2
    exit 1
  fi
done

start_source_body=$(sed -n '/^start_source()/,/^}/p' "$runner")
grep -Fq 'nohup "$SOURCE_BIN" --config "$SOURCE_CONFIG"' <<<"$start_source_body" || {
  echo "v1 source must start as a host background process" >&2
  exit 1
}
if grep -q 'compose.*db-qbs-source' <<<"$start_source_body"; then
  echo "v1 source must not run under docker compose" >&2
  exit 1
fi
grep -Fq 'GET /api/tasks' "$runner" || {
  echo "v1 readiness must use GET /api/tasks" >&2
  exit 1
}
grep -Fq 'kill -TERM "$SOURCE_PID"' "$runner" || {
  echo "v1 cleanup must try SIGTERM first" >&2
  exit 1
}
grep -Fq 'kill -KILL "$SOURCE_PID"' "$runner" || {
  echo "v1 cleanup must have a SIGKILL fallback" >&2
  exit 1
}

start_sink_body=$(sed -n '/^start_sink()/,/^}/p' "$runner")
grep -Fq 'kill -0 \"\$(cat $SINK_PID_FILE)\"' <<<"$start_sink_body" || {
  echo "v1 sink readiness must verify the process it just started" >&2
  exit 1
}

# --- C6 的四条：判据只有在这四件事都成立时才成立（ADR-0040 §3） ---

# 1. 两档之间必须重启 sink，否则 VmHWM 的跨 run 残留让比值恒为 1、判据永久假绿。
c6_tier_body=$(sed -n '/^c6_tier()/,/^}/p' "$runner")
grep -Fq 'start_sink' <<<"$c6_tier_body" || {
  echo "C6 must restart the sink at the start of each tier (ADR-0040 §3.5)" >&2
  exit 1
}
grep -Fq 'C6 sink was restarted between tiers' "$runner" || {
  echo "C6 must assert (and report) that the restart actually happened" >&2
  exit 1
}

# 2. 量的必须是内核高水位，不是轮询采样——采样漏峰值会让判据假绿。
grep -Fq 'VmHWM' "$runner" || { echo "C6 sink measurement must read VmHWM" >&2; exit 1; }
grep -Fq 'os.wait4' "$wrapper" || { echo "C6 source measurement must use wait4/ru_maxrss" >&2; exit 1; }
if grep -Eq 'VmRSS|ps -o rss' "$runner"; then
  echo "C6 must not sample RSS: sampling misses the peak and greens a broken build" >&2
  exit 1
fi

# 3. 判据是「减基线、比斜率、系数 2」，source 与 sink 各判一次。
grep -Fq 'assert_le "C6 source slope"' "$runner" || { echo "C6 must judge the source slope" >&2; exit 1; }
grep -Fq 'assert_le "C6 sink slope"' "$runner" || { echo "C6 must judge the sink slope" >&2; exit 1; }
grep -Fq '2 * delta_src_10k' "$runner" || { echo "C6 source criterion must be <= 2x the 10k delta" >&2; exit 1; }
grep -Fq '2 * delta_sink_10k' "$runner" || { echo "C6 sink criterion must be <= 2x the 10k delta" >&2; exit 1; }

# 4. 分母为零的话判据恒真——一条永远为真的断言比没有断言更坏。
grep -Fq 'assert_gt "C6 source 10k delta is measurable"' "$runner" || {
  echo "C6 must reject a zero 10k delta instead of passing vacuously" >&2
  exit 1
}
grep -Fq 'assert_gt "C6 sink 10k delta is measurable"' "$runner" || {
  echo "C6 must reject a zero 10k delta instead of passing vacuously" >&2
  exit 1
}

# `if cmd; then ...; fi` 之后的 `$?` 是 if 语句自己的 0，读不到 cmd 的退出码。
# run-m2 那条 N/A 出口从来没生效过就是这个坑；C 系列里不许再出现这个写法。
if grep -A 2 -E '^[[:space:]]*fi[[:space:]]*$' "$runner" | grep -q 'status=\$?'; then
  echo "do not read \$? after an if statement; capture it with 'cmd; status=\$?' first" >&2
  exit 1
fi

# 报告必须回答「客户那五条各自在哪儿验的」，且必须交代没跑的门禁。
grep -Fq '客户五条需求的兑现对照' "$runner" || {
  echo "the report must open with the five customer needs (owner ruling 2026-08-19)" >&2
  exit 1
}
grep -Fq '本入口未跑' "$runner" || {
  echo "the report must say which gates were not run and why" >&2
  exit 1
}

# fixture 另起文件：M1 基线是常量（ADR-0040 §1），C 系列不许改 oracle.sql / mysql.sql。
for fixture in ./acceptance/oracle-v1.sql ./acceptance/mysql-v1.sql; do
  [[ -f "$fixture" ]] || { echo "missing v1 fixture: $fixture" >&2; exit 1; }
done
if grep -Eq 'V1_(C2|C3|C4|C5|WIDE)' ./acceptance/mysql.sql; then
  echo "v1 target tables must live in mysql-v1.sql, not in the M1 baseline fixture" >&2
  exit 1
fi
if grep -Eq 'T_V1_' ./acceptance/oracle.sql; then
  echo "v1 source tables must live in oracle-v1.sql, not in the M1 baseline fixture" >&2
  exit 1
fi
grep -Fq 'T_M1_WIDE' "$runner" || {
  echo "C6 must run against the M1 wide table (ADR-0040 §3.3: the same wide table)" >&2
  exit 1
}

python3 -c "import ast, sys; ast.parse(open('$wrapper').read())"
grep -Fq '"a", encoding="utf-8"' "$wrapper" || {
  echo "the wrapper must append one line per child run, not overwrite" >&2
  exit 1
}

# 变量名后紧跟中文标点时必须用 `${var}`：mac 的非 UTF-8 locale 下 bash 会把高位字节
# 吃进变量名，`set -u` 当场报 unbound variable。首跑时 C6 与收尾清单各炸一次就是它。
if grep -qP '\$[A-Za-z_][A-Za-z0-9_]*[^\x00-\x7F]' "$runner"; then
  echo "brace variables that are followed by a non-ASCII character: \${var}, not \$var" >&2
  exit 1
fi

# `run_task_to_completion` 靠全局 `API_BODY` 把终态留给调用处断言，
# 用命令替换接它的输出会开子 shell，`API_BODY` 传不回来——C2① 首跑就栽在这儿。
if grep -qE '\$\(run_task_to_completion' "$runner"; then
  echo "do not capture run_task_to_completion in a command substitution; it loses API_BODY" >&2
  exit 1
fi

echo "v1 acceptance static checks: PASS"
