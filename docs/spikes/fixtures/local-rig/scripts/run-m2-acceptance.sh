#!/usr/bin/env bash
# Issue #60 / ADR-0028: one-entry M2 API and lifecycle acceptance on the arm64 mac rig.
set -uo pipefail

SCENARIOS=(
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
)

if [[ "${1:-}" == "--list" ]]; then
  printf '%s\n' "${SCENARIOS[@]}"
  exit 0
fi
if (( $# != 0 )); then
  echo "usage: $0 [--list]" >&2
  exit 2
fi

umask 077
RIG_ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO_ROOT=$(cd "$RIG_ROOT/../../../.." && pwd)
ACCEPTANCE_ROOT="$RIG_ROOT/acceptance"
WORK_ROOT=$(mktemp -d)
REPORT=${M2_REPORT:-"$RIG_ROOT/m2-acceptance-$(date -u +%Y%m%dT%H%M%SZ).md"}
SOURCE_BIN="$REPO_ROOT/target/release/db-qbs-source"
REAL_SOURCE_BIN="$REPO_ROOT/target/release/db-qbs-source-run"
SINK_BIN=/usr/local/bin/db-qbs-sink
SOURCE_CONFIG="$WORK_ROOT/source.toml"
SOURCE_DATA="$WORK_ROOT/source-data"
SOURCE_LOG="$WORK_ROOT/source.jsonl"
SINK_LOG=/tmp/m2-sink.jsonl
WRAPPER="$ACCEPTANCE_ROOT/m2-source-run-wrapper.py"
PROXY="$ACCEPTANCE_ROOT/commit-drop-proxy.py"
SOURCE_URL=http://127.0.0.1:18088
SINK_URL=http://127.0.0.1:18080
PROXY_URL=http://127.0.0.1:18081
BIZ_DATE=2026-08-14
RESULTS=()
SOURCE_PID=""
PROXY_PID=""
API_STATUS=""
API_BODY=""
A5_RECORD=""
A14_LIVE=""
A14_HISTORY=""

cd "$RIG_ROOT"

compose() {
  docker compose "$@"
}

fail() {
  echo "FAIL: $*" >&2
  return 1
}

assert_eq() {
  local label=$1 expected=$2 actual=$3
  echo "$label: $actual"
  [[ "$actual" == "$expected" ]] || fail "$label expected=$expected actual=$actual"
}

mysql_exec() {
  compose exec -T mysql mysql -N -B -uspike -pspike123 qbs -e "$1" 2>/dev/null
}

api() {
  local method=$1 path=$2 payload=${3:-} response
  if [[ "$method" == GET ]]; then
    response=$(curl -sS -X GET -H 'Accept: application/json' -w $'\n%{http_code}' "$SOURCE_URL$path") || return 1
  else
    response=$(curl -sS -X "$method" -H 'Accept: application/json' -H 'Content-Type: application/json' \
      --data-binary "$payload" -w $'\n%{http_code}' "$SOURCE_URL$path") || return 1
  fi
  API_STATUS=${response##*$'\n'}
  API_BODY=${response%$'\n'*}
}

history_count() {
  sqlite3 "$SOURCE_DATA/db-qbs.sqlite3" 'SELECT COUNT(*) FROM run_history;'
}

wait_for_run() {
  local record=$1 filter=$2 attempt
  for (( attempt = 1; attempt <= 400; attempt++ )); do
    if api GET "/api/runs/$record" && [[ "$API_STATUS" == 200 ]] && jq -e "$filter" <<<"$API_BODY" >/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  fail "run $record did not satisfy $filter; status=$API_STATUS body=$API_BODY"
}

wait_for_source() {
  local attempt status
  # Readiness contract: GET /api/tasks must return 200; no separate health endpoint exists.
  for (( attempt = 1; attempt <= 200; attempt++ )); do
    if [[ -n "$SOURCE_PID" ]] && ! kill -0 "$SOURCE_PID" 2>/dev/null; then
      wait "$SOURCE_PID" 2>/dev/null || true
      cat "$SOURCE_LOG" >&2 || true
      return 1
    fi
    status=$(curl -sS -o /dev/null -w '%{http_code}' "$SOURCE_URL/api/tasks" 2>/dev/null || true)
    if [[ "$status" == 200 ]]; then
      echo "GET /api/tasks: 200"
      return 0
    fi
    sleep 0.05
  done
  fail "source did not become ready"
}

stop_source() {
  local attempt
  [[ -n "$SOURCE_PID" ]] || return 0
  if kill -0 "$SOURCE_PID" 2>/dev/null; then
    kill -TERM "$SOURCE_PID" 2>/dev/null || true
    for (( attempt = 1; attempt <= 100; attempt++ )); do
      kill -0 "$SOURCE_PID" 2>/dev/null || break
      sleep 0.05
    done
    if kill -0 "$SOURCE_PID" 2>/dev/null; then
      kill -KILL "$SOURCE_PID" 2>/dev/null || true
    fi
  fi
  wait "$SOURCE_PID" 2>/dev/null || true
  SOURCE_PID=""
}

stop_child() {
  local pid
  [[ -f "$WORK_ROOT/child.pid" ]] || return 0
  pid=$(cat "$WORK_ROOT/child.pid")
  kill -TERM "$pid" 2>/dev/null || true
  rm -f "$WORK_ROOT/child.pid"
}

write_source_config() {
  local sink_url=$1
  mkdir -p "$SOURCE_DATA"
  cat > "$SOURCE_CONFIG" <<EOF
oracle_connect_string = "//127.0.0.1:1521/XE"
oracle_username = "spike"
oracle_password = "spike123"
oracle_client_lib_dir = "$M2_ORACLE_CLIENT_LIB_DIR"
sink_base_url = "$sink_url"
listen = "127.0.0.1:18088"
data_dir = "$SOURCE_DATA"
run_executable = "$WRAPPER"
history_retention_days = 90
EOF
}

start_source() {
  local mode=${1:-real} sink_url=${2:-$SINK_URL}
  stop_source || return 1
  stop_child || return 1
  rm -f "$WORK_ROOT/child.pid" "$WORK_ROOT/release-child"
  : > "$SOURCE_LOG"
  write_source_config "$sink_url" || return 1
  M2_CHILD_MODE="$mode" \
  M2_REAL_SOURCE_BIN="$REAL_SOURCE_BIN" \
  M2_CHILD_PID_FILE="$WORK_ROOT/child.pid" \
  M2_CHILD_RELEASE_FILE="$WORK_ROOT/release-child" \
    "$SOURCE_BIN" --config "$SOURCE_CONFIG" > "$SOURCE_LOG" 2>&1 &
  SOURCE_PID=$!
  wait_for_source
}

stop_proxy() {
  [[ -n "$PROXY_PID" ]] || return 0
  kill -TERM "$PROXY_PID" 2>/dev/null || true
  wait "$PROXY_PID" 2>/dev/null || true
  PROXY_PID=""
}

start_proxy() {
  local mode=$1 delay=${2:-0}
  stop_proxy || return 1
  M1_COMMIT_DROP_MODE="$mode" M1_COMMIT_DELAY_SECONDS="$delay" \
    python3 "$PROXY" > "$WORK_ROOT/proxy.log" 2>&1 &
  PROXY_PID=$!
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    curl -sS -o /dev/null "$PROXY_URL/v1/runs/not-a-run" 2>/dev/null && return 0
    sleep 0.05
  done
  fail "commit-drop-proxy.py did not become ready"
}

stop_sink() {
  compose exec -T client sh -c '
    test -f /tmp/m2-sink.pid || exit 0
    pid=$(cat /tmp/m2-sink.pid)
    kill "$pid" 2>/dev/null || true
    i=0
    while kill -0 "$pid" 2>/dev/null; do
      i=$((i + 1)); test "$i" -lt 100 || { kill -KILL "$pid" 2>/dev/null || true; break; }
      sleep 0.05
    done
    rm -f /tmp/m2-sink.pid
  '
}

start_sink() {
  stop_sink || return 1
  compose exec -T client rm -f /tmp/m2-sink.jsonl || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > /tmp/m2-sink.pid; exec $SINK_BIN --config /workspace/docs/spikes/fixtures/local-rig/acceptance/sink.toml > $SINK_LOG 2>&1" || return 1
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    curl -sS -o /dev/null "$SINK_URL/v1/runs/not-a-run" 2>/dev/null && return 0
    sleep 0.1
  done
  fail "sink did not become ready"
}

create_task() {
  local name=$1 sql=$2 target=${3:-M1_NARROW} payload
  payload=$(jq -nc --arg name "$name" --arg sql "$sql" --arg target "$target" '{
    name:$name, source_sql:$sql, source_date_col:"D_BIZ",
    target_table:$target, target_date_col:"D_BIZ"
  }') || return 1
  api POST /api/tasks "$payload" || return 1
  [[ "$API_STATUS" == 201 ]] || fail "create task status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.task_id' <<<"$API_BODY"
}

start_task_run() {
  local task_id=$1 date=${2:-$BIZ_DATE} payload
  payload=$(jq -nc --arg task "$task_id" --arg date "$date" '{task_id:$task,biz_date:$date}') || return 1
  api POST /api/runs "$payload" || return 1
  [[ "$API_STATUS" == 202 ]] || fail "start run status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.run_record_id' <<<"$API_BODY"
}

normal_sql() {
  printf '%s' "SELECT n.row_id AS ROW_ID, n.v_text AS V_TEXT, n.d_biz AS D_BIZ FROM t_m1_narrow n WHERE n.d_biz >= TO_DATE(:biz_date,'YYYY-MM-DD') AND n.d_biz < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

scenario_a1() {
  start_source real || return 1
  api GET /api/tasks || return 1
  assert_eq "readiness status" 200 "$API_STATUS" || return 1
  stop_source || return 1
  sqlite3 "$SOURCE_DATA/db-qbs.sqlite3" 'BEGIN IMMEDIATE; ROLLBACK;' || return 1
  echo "sqlite immediate lock after SIGTERM: available"
  start_source real || return 1
  api GET /api/tasks || return 1
  assert_eq "same-directory restart status" 200 "$API_STATUS" || return 1
  stop_source
}

scenario_a2() {
  start_source real || return 1
  local before_history after_history before_sink after_sink task_id payload columns
  before_history=$(history_count) || return 1
  before_sink=$(compose exec -T client sh -c "test -f $SINK_LOG && wc -l < $SINK_LOG || echo 0" | tr -d ' \r') || return 1
  task_id=$(create_task "A2 取列" "$(normal_sql)") || return 1
  payload=$(jq -nc --arg sql "$(normal_sql)" '{source_sql:$sql,source_date_col:"D_BIZ",target_table:"M1_NARROW",target_date_col:"D_BIZ"}')
  api POST /api/columns "$payload" || return 1
  assert_eq "column fetch status" 200 "$API_STATUS" || return 1
  columns=$(jq -c '.columns | map({name,data_type,precision,scale,length})' <<<"$API_BODY") || return 1
  echo "columns == describe: $columns"
  assert_eq "column names" '["ROW_ID","V_TEXT","D_BIZ"]' "$(jq -c '[.columns[].name]' <<<"$API_BODY")" || return 1
  assert_eq "run_id absent" false "$(jq 'has("run_id")' <<<"$API_BODY")" || return 1
  after_history=$(history_count) || return 1
  assert_eq "history rows unchanged" "$before_history" "$after_history" || return 1
  after_sink=$(compose exec -T client sh -c "test -f $SINK_LOG && wc -l < $SINK_LOG || echo 0" | tr -d ' \r') || return 1
  assert_eq "component=sink new log lines" 0 "$((after_sink - before_sink))" || return 1
  echo "task_id: $task_id"
}

scenario_a3() {
  start_source real || return 1
  local payload
  payload='{"source_sql":"SELECT * FROM t_m1_narrow","source_date_col":"D_BIZ","target_table":"M1_NARROW","target_date_col":"OTHER"}'
  api POST /api/columns "$payload" || return 1
  assert_eq "shape failure status" 422 "$API_STATUS" || return 1
  assert_eq "shape check count" 6 "$(jq '.checks | length' <<<"$API_BODY")" || return 1
  assert_eq "shape passed and failed both reported" true "$(jq '([.checks[].passed] | any) and ([.checks[].passed] | any(. == false))' <<<"$API_BODY")" || return 1
  assert_eq "error code absent" false "$(jq 'has("code") or has("error")' <<<"$API_BODY")" || return 1
  echo "actual response: $API_BODY"
}

scenario_a4() {
  start_source real || return 1
  local before after payload
  before=$(history_count) || return 1
  payload='{"source_sql":"SELECT n.D_BIZ AS D_BIZ FROM table_that_does_not_exist n WHERE n.D_BIZ >= TO_DATE(:biz_date,\"YYYY-MM-DD\") AND n.D_BIZ < TO_DATE(:biz_date,\"YYYY-MM-DD\") + 1","source_date_col":"D_BIZ","target_table":"M1_NARROW","target_date_col":"D_BIZ"}'
  api POST /api/columns "$payload" || return 1
  assert_eq "Oracle failure status" 502 "$API_STATUS" || return 1
  assert_eq "Oracle failure kind" oracle "$(jq -r '.kind' <<<"$API_BODY")" || return 1
  assert_eq "Oracle failure run_id absent" false "$(jq 'has("run_id")' <<<"$API_BODY")" || return 1
  after=$(history_count) || return 1
  assert_eq "Oracle failure history unchanged" "$before" "$after" || return 1
  echo "actual response: $API_BODY"
}

scenario_a5() {
  start_source real || return 1
  local task_id record run_id projection history
  task_id=$(create_task "A5 正常 10 万行" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "terminal effect" SWAPPED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "source rows" 100000 "$(jq -r '.source_rows' <<<"$API_BODY")" || return 1
  run_id=$(jq -r '.run_id' <<<"$API_BODY") || return 1
  history=$(jq -c '{stage,seq,rows_pushed,bytes,ms,last_ts}' <<<"$API_BODY") || return 1
  projection=$(jq -sc --arg run "$run_id" '
    [.[] | select(.run_id == $run)] as $lines |
    {stage:([$lines[] | select(.event=="stage_changed" or .event=="run_finished") | .stage] | last),
     seq:([$lines[] | select(.event=="batch_pushed") | .seq] | last // 0),
     rows_pushed:([$lines[] | select(.event=="batch_pushed") | .rows] | add // 0),
     bytes:([$lines[] | select(.event=="batch_pushed") | .bytes] | add // 0),
     ms:([$lines[] | select(.event=="batch_pushed") | .ms] | add // 0),
     last_ts:([$lines[] | .ts] | last)}' "$SOURCE_LOG") || return 1
  echo "projection-versus-history: $(jq -nc --argjson projection "$projection" --argjson history "$history" '{projection:$projection,history:$history}')"
  assert_eq "six aggregate scalars" "$(jq -S . <<<"$projection")" "$(jq -S . <<<"$history")" || return 1
  A5_RECORD=$record
}

scenario_a6() {
  start_source real || return 1
  local task_id record
  task_id=$(create_task "A6 形状失败" "SELECT * FROM t_m1_narrow") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "run_record_id retained" "$record" "$(jq -r '.run_record_id' <<<"$API_BODY")" || return 1
  assert_eq "run_id is null" null "$(jq -r '.run_id' <<<"$API_BODY")" || return 1
  assert_eq "run shape check count" 6 "$(jq '.shape_checks | length' <<<"$API_BODY")" || return 1
  assert_eq "run shape report has failure" true "$(jq '[.shape_checks[].passed] | any(. == false)' <<<"$API_BODY")" || return 1
  api GET "/api/runs?task_id=$task_id" || return 1
  assert_eq "history row present" true "$(jq --arg record "$record" 'any(.[]; .run_record_id == $record)' <<<"$API_BODY")"
}

scenario_a7() {
  start_source real || return 1
  mysql_exec 'DROP TABLE IF EXISTS M2_BAD; CREATE TABLE M2_BAD (ROW_ID DECIMAL(8,0) NULL, D_BIZ DATETIME(0) NULL, INDEX (D_BIZ)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4' >/dev/null || return 1
  local task_id record
  task_id=$(create_task "A7 映射失败" "$(normal_sql)" M2_BAD) || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "mapping run_id exists" true "$(jq '.run_id != null' <<<"$API_BODY")" || return 1
  assert_eq "mapping code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "mapping has no terminal block" true "$(jq '.target_table_effect == "DISCARDED" and .staging_table == null' <<<"$API_BODY")" || return 1
  assert_eq "mapping issue total" true "$(jq '.mapping_issues | length > 0' <<<"$API_BODY")" || return 1
  echo "actual response: $API_BODY"
}

scenario_a8() {
  start_proxy verify || return 1
  start_source real "$PROXY_URL" || return 1
  local task_id record
  task_id=$(create_task "A8 校验失败" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "verification code" VERIFY_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "verification terminal" DISCARDED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  echo "actual response: $API_BODY"
  stop_proxy
}

scenario_a9() {
  start_source fail-escape || return 1
  local task_id record
  task_id=$(create_task "A9 哨兵逃逸" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "escape code" INTERNAL_PRECHECK_ESCAPE "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "escape column" V_TEXT "$(jq -r '.column' <<<"$API_BODY")" || return 1
  assert_eq "escape value" 真实业务值-1265 "$(jq -r '.value' <<<"$API_BODY")" || return 1
  echo "actual response: $API_BODY"
}

scenario_a10() {
  start_source hang-streaming || return 1
  local task_id record payload
  task_id=$(create_task "A10 并发" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == true and .stage == "STREAMING"' || return 1
  payload=$(jq -nc --arg task "$task_id" --arg date "$BIZ_DATE" '{task_id:$task,biz_date:$date}')
  api POST /api/runs "$payload" || return 1
  assert_eq "concurrent start status" 409 "$API_STATUS" || return 1
  echo "actual rejection: $API_BODY"
  stop_child
  wait_for_run "$record" '.live == false'
}

scenario_a11() {
  start_proxy swapped 3 || return 1
  start_source real "$PROXY_URL" || return 1
  local task_id record
  task_id=$(create_task "A11 提交取消" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == true and .stage == "COMMITTING"' || return 1
  api POST "/api/runs/$record/cancel" '{}' || return 1
  assert_eq "COMMITTING cancel status" 409 "$API_STATUS" || return 1
  assert_eq "COMMITTING cancel reason" 已过封口点，停不了 "$(jq -r '.error.message' <<<"$API_BODY")" || return 1
  echo "actual rejection: $API_BODY"
  wait_for_run "$record" '.live == false' || return 1
  stop_proxy
}

scenario_a12() {
  start_source hang-streaming || return 1
  local task_id record killed_pid
  task_id=$(create_task "A12 进程消失" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == true and .stage == "STREAMING"' || return 1
  killed_pid=$SOURCE_PID
  kill -KILL "$SOURCE_PID" || return 1
  wait "$SOURCE_PID" 2>/dev/null || true
  SOURCE_PID=""
  start_source real || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "kill -KILL unknown reason" PROCESS_DISAPPEARED "$(jq -r '.unknown_reason' <<<"$API_BODY")" || return 1
  assert_eq "kill -KILL error codes absent" true "$(jq '.source_code == null and .sink_code == null' <<<"$API_BODY")" || return 1
  echo "killed host source pid: $killed_pid"
  stop_child
}

scenario_a13() {
  start_source hang-streaming || return 1
  local task_id record
  task_id=$(create_task "A13 服务重启" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == true and .stage == "STREAMING"' || return 1
  stop_source || return 1
  start_source real || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "SIGTERM unknown reason" SERVICE_RESTARTED "$(jq -r '.unknown_reason' <<<"$API_BODY")" || return 1
  assert_eq "SIGTERM error codes absent" true "$(jq '.source_code == null and .sink_code == null' <<<"$API_BODY")" || return 1
  stop_child
}

scenario_a14() {
  start_source hang-streaming || return 1
  local task_id record
  task_id=$(create_task "A14 生命周期" "$(normal_sql)") || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == true' || return 1
  A14_LIVE=$API_BODY
  assert_eq "live detail address" "$record" "$(jq -r '.run_record_id' <<<"$A14_LIVE")" || return 1
  stop_child
  wait_for_run "$record" '.live == false' || return 1
  A14_HISTORY=$API_BODY
  assert_eq "history detail address" "$record" "$(jq -r '.run_record_id' <<<"$A14_HISTORY")" || return 1
  assert_eq "same endpoint live transition" true "$(jq -nc --argjson live "$A14_LIVE" --argjson history "$A14_HISTORY" '$live.live == true and $history.live == false')" || return 1
  echo "live response: $A14_LIVE"
  echo "history fallback response: $A14_HISTORY"
  [[ -n "$A5_RECORD" ]] && echo "completed A5 address: $A5_RECORD"
}

scenario_index() {
  local wanted=$1 index
  for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
    [[ "${SCENARIOS[$index]}" == "$wanted" ]] && { printf '%s' "$index"; return 0; }
  done
  return 1
}

run_scenario() {
  local name=$1 function=$2 index output
  index=$(scenario_index "$name") || return 1
  output="$WORK_ROOT/$name.out"
  echo "==> $name"
  if "$function" > "$output" 2>&1; then
    RESULTS[$index]=PASS
    echo "    PASS"
  else
    RESULTS[$index]=FAIL
    echo "    FAIL"
    sed 's/^/    /' "$output"
  fi
}

write_report() {
  mkdir -p "$(dirname "$REPORT")" || return 1
  {
    echo "# M2 rig acceptance report"
    echo
    echo "- Generated (UTC): $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Git commit: $(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
    echo "- Visual walkthrough: required separately by m2-visual-walkthrough.md"
    echo
    echo "## Scenarios"
    echo
    echo "| Scenario | Result |"
    echo "|---|---|"
    local index name output
    for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
      echo "| ${SCENARIOS[$index]} | ${RESULTS[$index]:-FAIL} |"
    done
    echo
    echo "## Assertion Evidence"
    for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
      name=${SCENARIOS[$index]}
      output="$WORK_ROOT/$name.out"
      echo
      echo "### $name"
      echo
      echo '```text'
      [[ -f "$output" ]] && cat "$output"
      echo '```'
    done
  } > "$REPORT"
  chmod 600 "$REPORT"
}

prepare_rig() {
  local command
  for command in docker jq curl cargo sqlite3 python3; do
    command -v "$command" >/dev/null || { echo "missing required command: $command" >&2; return 1; }
  done
  [[ -n "${M2_ORACLE_CLIENT_LIB_DIR:-}" && -d "$M2_ORACLE_CLIENT_LIB_DIR" ]] || {
    echo "M2_ORACLE_CLIENT_LIB_DIR must point to the host Oracle Instant Client directory" >&2
    return 1
  }
  ./scripts/up.sh || return 1
  docker run --rm --platform linux/arm64 \
    -v "$REPO_ROOT:/workspace" -w /workspace \
    -v qbs-cargo-registry:/usr/local/cargo/registry \
    rust:1-bookworm cargo build --release --workspace || return 1
  docker cp "$REPO_ROOT/target/release/db-qbs-sink" qbs-client:"$SINK_BIN" || return 1
  cargo build --release -p db-qbs-source || return 1
  compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE \
    @/workspace/docs/spikes/fixtures/local-rig/acceptance/oracle.sql || return 1
  compose exec -T mysql mysql -uspike -pspike123 qbs < "$ACCEPTANCE_ROOT/mysql.sql" || return 1
  start_sink
}

cleanup() {
  stop_proxy >/dev/null 2>&1 || true
  stop_source >/dev/null 2>&1 || true
  stop_child >/dev/null 2>&1 || true
  stop_sink >/dev/null 2>&1 || true
  rm -rf "$WORK_ROOT"
}
trap cleanup EXIT

echo "==> prepare M2 acceptance rig"
prepare_rig || { echo "rig preparation failed" >&2; exit 1; }

run_scenario A1-start-stop-readiness scenario_a1
run_scenario A2-task-column-fetch scenario_a2
run_scenario A3-column-fetch-shape-failure scenario_a3
run_scenario A4-column-fetch-oracle-failure scenario_a4
run_scenario A5-success-projection-history scenario_a5
run_scenario A6-run-shape-failure scenario_a6
run_scenario A7-mapping-precheck-failure scenario_a7
run_scenario A8-verification-failure scenario_a8
run_scenario A9-sentinel-escape scenario_a9
run_scenario A10-concurrent-rejection scenario_a10
run_scenario A11-committing-cancel-rejection scenario_a11
run_scenario A12-process-disappeared scenario_a12
run_scenario A13-service-restarted scenario_a13
run_scenario A14-detail-lifecycle scenario_a14

write_report || { echo "failed to write report" >&2; exit 1; }
echo "report: $REPORT"

failed=0
for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
  [[ "${RESULTS[$index]:-FAIL}" == PASS ]] || failed=$((failed + 1))
done
if (( failed > 0 )); then
  echo "M2 acceptance: FAIL ($failed/${#SCENARIOS[@]} scenarios)"
  exit 1
fi
echo "M2 acceptance: PASS (${#SCENARIOS[@]}/${#SCENARIOS[@]} scenarios)"
