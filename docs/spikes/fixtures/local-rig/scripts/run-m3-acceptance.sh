#!/usr/bin/env bash
# Issue #115 / ADR-0032: M3 type, mapping, and source-value acceptance on the local rig.
set -uo pipefail

SCENARIOS=(
  B1-nine-shape-round-trip
  B2-all-mapping-rejections
  B3-timestamp-fsp-rejection
  B4-bare-number-range-rejection
  B5-no-range-check
  B6-bc-date-source-value
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
REPORT=${M3_REPORT:-"$RIG_ROOT/m3-acceptance-$(date -u +%Y%m%dT%H%M%SZ).md"}
HOST_TARGET=${M2_HOST_CARGO_TARGET:-}
HOST_BIN_DIR="$REPO_ROOT/target${HOST_TARGET:+/$HOST_TARGET}/release"
SOURCE_BIN="$HOST_BIN_DIR/db-qbs-source"
REAL_SOURCE_BIN="$HOST_BIN_DIR/db-qbs-source-run"
SINK_BIN=/usr/local/bin/db-qbs-sink
SOURCE_CONFIG="$WORK_ROOT/source.toml"
SOURCE_DATA="$WORK_ROOT/source-data"
SOURCE_LOG="$WORK_ROOT/source.jsonl"
SINK_LOG=/tmp/m3-sink.jsonl
SOURCE_URL=http://127.0.0.1:18088
SOURCE_PORT=18088
SINK_URL=http://127.0.0.1:18080
BIZ_DATE=2026-08-14
KEEP_RIG=${M3_KEEP_RIG:-}
RESULTS=()
SOURCE_PID=""
API_STATUS=""
API_BODY=""
B2_RECORD=""
B4_RECORD=""

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
  compose exec -T mysql mysql -N -B -uspike -pspike123 qbs -e "$1" 2>/dev/null | tr -d '\r'
}

mysql_staging_count() {
  local table=$1
  mysql_exec "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'qbs' AND table_name LIKE '${table}__stg_%'"
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

wait_for_source() {
  local attempt status
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

ensure_source_port_free() {
  local pids attempt
  pids=$(lsof -ti "tcp:$SOURCE_PORT" 2>/dev/null || true)
  [[ -n "$pids" ]] || return 0
  echo "==> stopping stale processes on $SOURCE_PORT: $(echo "$pids" | tr '\n' ' ')"
  # shellcheck disable=SC2086
  kill -KILL $pids 2>/dev/null || true
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    lsof -ti "tcp:$SOURCE_PORT" >/dev/null 2>&1 || return 0
    sleep 0.05
  done
  fail "port $SOURCE_PORT did not become free"
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
run_executable = "$REAL_SOURCE_BIN"
history_retention_days = 90
EOF
}

start_source() {
  stop_source || return 1
  ensure_source_port_free || return 1
  : > "$SOURCE_LOG"
  write_source_config "$SINK_URL" || return 1
  nohup "$SOURCE_BIN" --config "$SOURCE_CONFIG" > "$SOURCE_LOG" 2>&1 &
  SOURCE_PID=$!
  wait_for_source || return 1
  kill -0 "$SOURCE_PID" 2>/dev/null && return 0
  cat "$SOURCE_LOG" >&2 || true
  return 1
}

stop_sink() {
  compose exec -T client sh -c '
    test -f /tmp/m3-sink.pid || exit 0
    pid=$(cat /tmp/m3-sink.pid)
    kill -TERM "$pid" 2>/dev/null || true
    i=0
    while kill -0 "$pid" 2>/dev/null; do
      i=$((i + 1)); test "$i" -lt 100 || { kill -KILL "$pid" 2>/dev/null || true; break; }
      sleep 0.05
    done
    rm -f /tmp/m3-sink.pid
  '
}

start_sink() {
  stop_sink || return 1
  compose exec -T client rm -f "$SINK_LOG" || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > /tmp/m3-sink.pid; exec $SINK_BIN --config /workspace/docs/spikes/fixtures/local-rig/acceptance/sink.toml > $SINK_LOG 2>&1" || return 1
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    curl -sS -o /dev/null "$SINK_URL/v1/runs/not-a-run" 2>/dev/null && return 0
    sleep 0.1
  done
  fail "sink did not become ready"
}

create_task() {
  local name=$1 sql=$2 target=$3 precision=${4:-} payload
  if [[ -n "$precision" ]]; then
    payload=$(jq -nc --arg name "$name" --arg sql "$sql" --arg target "$target" \
      --argjson precision "$precision" '{name:$name,source_sql:$sql,source_date_col:"LOAD_DATE",target_table:$target,target_date_col:"LOAD_DATE",column_precision:$precision}') || return 1
  else
    payload=$(jq -nc --arg name "$name" --arg sql "$sql" --arg target "$target" \
      '{name:$name,source_sql:$sql,source_date_col:"LOAD_DATE",target_table:$target,target_date_col:"LOAD_DATE"}') || return 1
  fi
  api POST /api/tasks "$payload" || return 1
  [[ "$API_STATUS" == 201 ]] || fail "create task status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.task_id' <<<"$API_BODY"
}

start_task_run() {
  local task_id=$1 payload
  payload=$(jq -nc --arg task "$task_id" --arg date "$BIZ_DATE" '{task_id:$task,biz_date:$date}') || return 1
  api POST /api/runs "$payload" || return 1
  [[ "$API_STATUS" == 202 ]] || fail "start run status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.run_record_id' <<<"$API_BODY"
}

source_log_event_count() {
  local run_id=$1 event=$2
  jq -sc --arg run "$run_id" --arg event "$event" \
    '[.[] | select(.run_id == $run and .event == $event)] | length' "$SOURCE_LOG"
}

source_log_event() {
  local run_id=$1 event=$2
  jq -sc --arg run "$run_id" --arg event "$event" \
    '[.[] | select(.run_id == $run and .event == $event)] | if length == 0 then {} else .[-1] end' "$SOURCE_LOG"
}

hex_value() {
  printf '%s' "$1" | od -An -tx1 | tr -d ' \n' | tr 'a-f' 'A-F'
}

assert_column_values() {
  local label=$1 query=$2 expected=$3 actual
  actual=$(mysql_exec "$query") || return 1
  assert_eq "$label" "$expected" "$actual"
}

b1_sql() {
  printf '%s' "SELECT n.row_id AS ROW_ID, n.n_regular AS N_REGULAR, n.n_fraction AS N_FRACTION, n.n_negative AS N_NEGATIVE, n.n_bare AS N_BARE, n.n_bare * 1 AS N_EXPR, n.v_text AS V_TEXT, n.nv_text AS NV_TEXT, n.c_text AS C_TEXT, n.nc_text AS NC_TEXT, n.d_value AS D_VALUE, n.ts0 AS TS0, n.ts3 AS TS3, n.ts6 AS TS6, n.load_date AS LOAD_DATE FROM t_m3_b1 n WHERE n.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND n.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

b2_sql() {
  printf '%s' "SELECT b.bf AS BF, b.bd AS BD, b.payload AS PAYLOAD, b.v_text || b.v_text AS C_EXPR, b.c_char AS C_CHAR, b.n_too_wide AS N_TOO_WIDE, b.n_too_scale AS N_TOO_SCALE, b.n_missing AS N_MISSING, b.d_wrong AS D_WRONG, b.load_date AS LOAD_DATE FROM t_m3_b2 b WHERE b.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND b.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

b3_sql() {
  printf '%s' "SELECT b.ts_too_precise AS TS_TOO_PRECISE, b.load_date AS LOAD_DATE FROM t_m3_b3 b WHERE b.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND b.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

b4_sql() {
  printf '%s' "SELECT b.row_id AS ROW_ID, b.n_bare AS N_BARE, b.load_date AS LOAD_DATE FROM t_m3_b4 b WHERE b.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND b.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

b5_sql() {
  printf '%s' "SELECT b.row_id AS ROW_ID, b.n_regular AS N_REGULAR, b.v_text AS V_TEXT, b.d_value AS D_VALUE, b.ts_value AS TS_VALUE, b.load_date AS LOAD_DATE FROM t_m3_b5 b WHERE b.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND b.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

b6_sql() {
  printf '%s' "SELECT b.row_id AS ROW_ID, b.d_bc AS D_BC, b.load_date AS LOAD_DATE FROM t_m3_b6 b WHERE b.load_date >= TO_DATE(:biz_date,'YYYY-MM-DD') AND b.load_date < TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
}

scenario_b1() {
  start_source || return 1
  local task_id record run_id expected
  task_id=$(create_task "B1 九行形态" "$(b1_sql)" M3_B1 '{"N_BARE":[20,4],"N_EXPR":[20,4]}') || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  run_id=$(jq -r '.run_id' <<<"$API_BODY") || return 1
  assert_eq "B1 terminal" SWAPPED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "B1 outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "B1 source rows" 6 "$(jq -r '.source_rows' <<<"$API_BODY")" || return 1
  assert_eq "B1 mapping issues" 0 "$(jq '.mapping_issues | length' <<<"$API_BODY")" || return 1
  assert_eq "B1 target rows" 6 "$(mysql_exec "SELECT COUNT(*) FROM M3_B1 WHERE LOAD_DATE >= '2026-08-14' AND LOAD_DATE < '2026-08-15'")" || return 1
  assert_eq "B1 sentinel removed" 0 "$(mysql_exec 'SELECT COUNT(*) FROM M3_B1 WHERE ROW_ID = 900')" || return 1

  expected=$(printf '%s\n' NULL 1.23 -0.01 123456789012345678901234567890123456.78 0.00 0.00)
  assert_column_values "B1 NUMBER(38,2)" 'SELECT IFNULL(CAST(N_REGULAR AS CHAR), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' 0.000001 NULL 0.009999 -0.009999 0.000000 -0.000001)
  assert_column_values "B1 NUMBER(4,6)" 'SELECT IFNULL(CAST(N_FRACTION AS CHAR), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' 9999999900 -9999999900 NULL 12300 12400 0)
  assert_column_values "B1 NUMBER(8,-2)" 'SELECT IFNULL(CAST(N_NEGATIVE AS CHAR), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' 0.0000 1.2345 -99999999999999.9999 NULL 0.0000 1.2345)
  assert_column_values "B1 bare NUMBER" 'SELECT IFNULL(CAST(N_BARE AS CHAR), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  assert_column_values "B1 numeric expression" 'SELECT IFNULL(CAST(N_EXPR AS CHAR), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1

  expected=$(printf '%s\n' "$(hex_value 'ABCD')" "$(hex_value 'AB        ')" "$(hex_value '          ')" "$(hex_value '甲乙        ')" NULL "$(hex_value 'ABCD')")
  assert_column_values "B1 VARCHAR2 bytes" 'SELECT IFNULL(HEX(V_TEXT), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value '甲乙        ')" "$(hex_value 'ABCD')" "$(hex_value '甲乙        ')" "$(hex_value 'ABCD')" "$(hex_value '          ')" NULL)
  assert_column_values "B1 NVARCHAR2 bytes" 'SELECT IFNULL(HEX(NV_TEXT), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value 'AB        ')" "$(hex_value '          ')" "$(hex_value '甲乙        ')" NULL "$(hex_value '          ')" "$(hex_value 'AB        ')")
  assert_column_values "B1 CHAR bytes" 'SELECT IFNULL(HEX(C_TEXT), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value '甲乙        ')" "$(hex_value '          ')" "$(hex_value 'AB        ')" "$(hex_value '甲乙        ')" "$(hex_value '          ')" NULL)
  assert_column_values "B1 NCHAR bytes" 'SELECT IFNULL(HEX(NC_TEXT), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1

  expected=$(printf '%s\n' "$(hex_value '0001-01-01 00:00:00')" "$(hex_value '0044-01-01 00:00:00')" "$(hex_value '0999-12-31 23:59:59')" "$(hex_value '9999-12-31 23:59:59')" "$(hex_value '2026-08-13 14:35:09')" NULL)
  assert_column_values "B1 DATE bytes" 'SELECT IFNULL(HEX(CAST(D_VALUE AS CHAR)), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value '2026-08-13 14:35:09.000000')" "$(hex_value '2026-08-13 14:35:09.000000')" "$(hex_value '2026-08-13 14:35:09.000000')" "$(hex_value '2026-08-13 14:35:09.000000')" NULL "$(hex_value '2026-08-13 14:35:09.000000')")
  assert_column_values "B1 TIMESTAMP(0) bytes" 'SELECT IFNULL(HEX(CAST(TS0 AS CHAR)), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value '2026-08-13 14:35:09.120000')" "$(hex_value '2026-08-13 14:35:09.120000')" "$(hex_value '2026-08-13 14:35:09.120000')" "$(hex_value '2026-08-13 14:35:09.120000')" NULL "$(hex_value '2026-08-13 14:35:09.120000')")
  assert_column_values "B1 TIMESTAMP(3) bytes" 'SELECT IFNULL(HEX(CAST(TS3 AS CHAR)), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  expected=$(printf '%s\n' "$(hex_value '2026-08-13 14:35:09.120000')" "$(hex_value '2026-08-13 14:35:09.999999')" "$(hex_value '2026-08-13 14:35:09.120000')" "$(hex_value '2026-08-13 00:00:00.000000')" NULL "$(hex_value '2026-08-13 14:35:09.120000')")
  assert_column_values "B1 TIMESTAMP(6) bytes" 'SELECT IFNULL(HEX(CAST(TS6 AS CHAR)), "NULL") FROM M3_B1 ORDER BY ROW_ID' "$expected" || return 1
  echo "B1 run_id: $run_id"
}

scenario_b2() {
  start_source || return 1
  local task_id record total column
  task_id=$(create_task "B2 一次报全" "$(b2_sql)" M3_B2) || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  total=$(jq '.mapping_issues | length' <<<"$API_BODY") || return 1
  assert_eq "B2 total issues" 10 "$total" || return 1
  assert_eq "B2 sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "B2 failure kind" MAPPING_PRECHECK "$(jq -r '.failure_kind' <<<"$API_BODY")" || return 1
  assert_eq "B2 staging table" null "$(jq -r '.staging_table' <<<"$API_BODY")" || return 1
  assert_eq "B2 target effect" DISCARDED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  [[ "$(jq -r '.message' <<<"$API_BODY")" == *"一次发现 $total 项问题"* ]] || fail "B2 message did not repeat total=$total: $API_BODY" || return 1
  for column in BF BD PAYLOAD C_EXPR C_CHAR N_TOO_WIDE N_TOO_SCALE N_MISSING D_WRONG EXTRA; do
    assert_eq "B2 issue $column" true "$(jq --arg column "$column" 'any(.mapping_issues[]; .column == $column)' <<<"$API_BODY")" || return 1
  done
  assert_eq "B2 staging tables" 0 "$(mysql_staging_count M3_B2)" || return 1
  echo "B2 mapping issues ($total): $(jq -c '.mapping_issues' <<<"$API_BODY")"
  B2_RECORD=$record
}

scenario_b3() {
  start_source || return 1
  local task_id record run_id
  task_id=$(create_task "B3 TIMESTAMP(9)" "$(b3_sql)" M3_B3) || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  run_id=$(jq -r '.run_id' <<<"$API_BODY") || return 1
  assert_eq "B3 sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "B3 failure kind" MAPPING_PRECHECK "$(jq -r '.failure_kind' <<<"$API_BODY")" || return 1
  assert_eq "B3 rejection rule" 'TIMESTAMP(n>6) 不在白名单' "$(jq -r '.mapping_issues[0].rule' <<<"$API_BODY")" || return 1
  assert_eq "B3 range checks" 0 "$(source_log_event_count "$run_id" range_check_executed)" || return 1
  assert_eq "B3 staging tables" 0 "$(mysql_staging_count M3_B3)" || return 1
  echo "B3 API report: $(jq -c '{sink_code, failure_kind, mapping_issues, staging_table, target_table_effect}' <<<"$API_BODY")"
}

scenario_b4() {
  start_source || return 1
  local task_id record run_id range_event scanned_rows range_ms
  task_id=$(create_task "B4 裸 NUMBER 值域" "$(b4_sql)" M3_B4 '{"N_BARE":[20,4]}') || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  run_id=$(jq -r '.run_id' <<<"$API_BODY") || return 1
  assert_eq "B4 sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "B4 failure kind" MAPPING_PRECHECK "$(jq -r '.failure_kind' <<<"$API_BODY")" || return 1
  assert_eq "B4 issue count" 1 "$(jq '.mapping_issues | length' <<<"$API_BODY")" || return 1
  assert_eq "B4 issue column" N_BARE "$(jq -r '.mapping_issues[0].column' <<<"$API_BODY")" || return 1
  assert_eq "B4 invalid rows" true "$(jq -r '.mapping_issues[0].rule | contains("1 行")' <<<"$API_BODY")" || return 1
  range_event=$(source_log_event "$run_id" range_check_executed) || return 1
  scanned_rows=$(jq -r '.scanned_rows' <<<"$range_event") || return 1
  range_ms=$(jq -r '.ms' <<<"$range_event") || return 1
  assert_eq "B4 scanned rows" 5 "$scanned_rows" || return 1
  [[ "$range_ms" =~ ^[0-9]+$ ]] || fail "B4 range-check ms is not numeric: $range_event" || return 1
  echo "B4 range_check_executed: $range_event"
  assert_eq "B4 sentinel retained" 1 "$(mysql_exec 'SELECT COUNT(*) FROM M3_B4 WHERE ROW_ID = 900 AND N_BARE = 9.9999')" || return 1
  assert_eq "B4 staging tables" 0 "$(mysql_staging_count M3_B4)" || return 1
  B4_RECORD=$record
}

scenario_b5() {
  start_source || return 1
  local task_id record run_id
  task_id=$(create_task "B5 常规形态零扫描" "$(b5_sql)" M3_B5) || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  run_id=$(jq -r '.run_id' <<<"$API_BODY") || return 1
  assert_eq "B5 terminal" SWAPPED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "B5 range checks" 0 "$(source_log_event_count "$run_id" range_check_executed)" || return 1
  assert_eq "B5 target rows" 1 "$(mysql_exec 'SELECT COUNT(*) FROM M3_B5 WHERE LOAD_DATE >= "2026-08-14" AND LOAD_DATE < "2026-08-15"')" || return 1
  echo "B5 range_check_executed count: 0 (run_id=$run_id)"
}

scenario_b6() {
  start_source || return 1
  local task_id record
  task_id=$(create_task "B6 公元前日期" "$(b6_sql)" M3_B6) || return 1
  record=$(start_task_run "$task_id") || return 1
  wait_for_run "$record" '.live == false' || return 1
  assert_eq "B6 outcome" FAILED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "B6 failure kind" SOURCE_VALUE "$(jq -r '.failure_kind' <<<"$API_BODY")" || return 1
  assert_eq "B6 stage" STREAMING "$(jq -r '.stage' <<<"$API_BODY")" || return 1
  assert_eq "B6 column" D_BC "$(jq -r '.column' <<<"$API_BODY")" || return 1
  assert_eq "B6 original value present" true "$(jq '.value != null and .value != ""' <<<"$API_BODY")" || return 1
  assert_eq "B6 target effect" DISCARDED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "B6 sentinel retained" 1 "$(mysql_exec 'SELECT COUNT(*) FROM M3_B6 WHERE ROW_ID = 900 AND D_BC = "2026-08-13 00:00:00"')" || return 1
  assert_eq "B6 staging tables" 0 "$(mysql_staging_count M3_B6)" || return 1
  echo "B6 source-value report: $(jq -c '{failure_kind, stage, column, value, target_table_effect, staging_table}' <<<"$API_BODY")"
}

scenario_index() {
  local wanted=$1 index
  for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
    if [[ "${SCENARIOS[$index]}" == "$wanted" ]]; then
      printf '%s' "$index"
      return 0
    fi
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
    echo "# M3 rig acceptance report"
    echo
    echo "- Generated (UTC): $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Git commit: ${DB_QBS_GIT_COMMIT:-$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo "unknown")}"
    echo "- Preconditions: run-m1-acceptance.sh and run-m2-acceptance.sh must already be green"
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
    echo "## Assertion evidence"
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
    if [[ -f "$WORK_ROOT/keep-b2.out" || -f "$WORK_ROOT/keep-b4.out" ]]; then
      echo
      echo "## KEEP_RIG state sequence"
      for output in "$WORK_ROOT/keep-b2.out" "$WORK_ROOT/keep-b4.out"; do
        [[ -f "$output" ]] || continue
        echo
        echo "### $(basename "$output" .out)"
        echo
        echo '```text'
        cat "$output"
        echo '```'
      done
    fi
    echo
    echo "## Manual gates"
    echo
    echo "- G1: run the generated DDL through MySQL describe and sink precheck for all nine ADR-0030 shapes; NUMBER(38,-30) must fail at generation, before MySQL sees DECIMAL(68,0)."
    echo "- G2: run scripts/run-canon-gate.sh unchanged."
    echo "- W1-W6: record actual observations in m3-visual-walkthrough-<UTC>.md; this report does not claim those observations."
  } > "$REPORT"
  chmod 600 "$REPORT"
}

prepare_rig() {
  local command
  for command in docker jq curl cargo sqlite3 lsof od; do
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
  if [[ -n "$HOST_TARGET" ]]; then
    cargo build --release --target "$HOST_TARGET" -p db-qbs-source || return 1
  else
    cargo build --release -p db-qbs-source || return 1
  fi
  compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE \
    @/workspace/docs/spikes/fixtures/local-rig/acceptance/oracle-m3.sql || return 1
  compose exec -T mysql mysql -uspike -pspike123 qbs < "$ACCEPTANCE_ROOT/mysql-m3.sql" || return 1
  start_sink
}

cleanup() {
  if [[ -n "$KEEP_RIG" ]]; then
    return 0
  fi
  stop_source >/dev/null 2>&1 || true
  stop_sink >/dev/null 2>&1 || true
  rm -rf "$WORK_ROOT"
}
trap cleanup EXIT

hand_over_rig() {
  [[ -n "$KEEP_RIG" ]] || return 0
  scenario_b2 > "$WORK_ROOT/keep-b2.out" 2>&1 || return 1
  scenario_b4 > "$WORK_ROOT/keep-b4.out" 2>&1 || return 1
  start_source || return 1
  cat <<EOF

==> rig kept for docs/spikes/fixtures/local-rig/m3-visual-walkthrough.md
    web UI              : $SOURCE_URL
    B2 / W1-W2 run      : $B2_RECORD
    B4 / W6 run         : $B4_RECORD
    W3-W4 builder SQL   : use B1 source SQL with N_BARE/N_EXPR and column_precision; use B2 for BINARY_FLOAT
    W5 builder SQL       : use B2 source SQL and inspect the CLOB column from t_m3_b2
    source data/history : $SOURCE_DATA/db-qbs.sqlite3
    source log          : $SOURCE_LOG
    tear down with      : kill $SOURCE_PID; docker compose -f $RIG_ROOT/docker-compose.yml exec -T client pkill db-qbs-sink; rm -rf $WORK_ROOT
EOF
  disown "$SOURCE_PID" 2>/dev/null || true
}

echo "==> prepare M3 acceptance rig"
prepare_rig || { echo "rig preparation failed" >&2; exit 1; }

run_scenario B1-nine-shape-round-trip scenario_b1
run_scenario B2-all-mapping-rejections scenario_b2
run_scenario B3-timestamp-fsp-rejection scenario_b3
run_scenario B4-bare-number-range-rejection scenario_b4
run_scenario B5-no-range-check scenario_b5
run_scenario B6-bc-date-source-value scenario_b6

hand_over_rig || { echo "failed to hand the rig over for the visual walkthrough" >&2; exit 1; }
write_report || { echo "failed to write report" >&2; exit 1; }
echo "report: $REPORT"

failed=0
for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
  [[ "${RESULTS[$index]:-FAIL}" == PASS ]] || failed=$((failed + 1))
done
if (( failed > 0 )); then
  echo "M3 acceptance: FAIL ($failed/${#SCENARIOS[@]} scenarios)"
  exit 1
fi
echo "M3 acceptance: PASS (${#SCENARIOS[@]}/${#SCENARIOS[@]} scenarios)"
