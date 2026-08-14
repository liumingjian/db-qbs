#!/usr/bin/env bash
# Issue #45: one-entry M1 rig acceptance orchestration. Run on the ADR-0005 arm64 mac rig.
set -uo pipefail

SCENARIOS=(
  wide-100k
  narrow-100k
  source-kill-rerun
  commit-disconnect
  empty-result
  verification-failures
  canonical-form
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
REPORT=${M1_REPORT:-"$RIG_ROOT/m1-acceptance-$(date -u +%Y%m%dT%H%M%SZ).md"}
SOURCE_BIN=/usr/local/bin/db-qbs-source
SINK_BIN=/usr/local/bin/db-qbs-sink
SOURCE_CONFIG=/workspace/docs/spikes/fixtures/local-rig/acceptance/source.toml
DROP_CONFIG=/workspace/docs/spikes/fixtures/local-rig/acceptance/source-commit-drop.toml
SINK_CONFIG=/workspace/docs/spikes/fixtures/local-rig/acceptance/sink.toml
NARROW_TASK=/workspace/docs/spikes/fixtures/local-rig/acceptance/task-narrow.toml
WIDE_TASK=/workspace/docs/spikes/fixtures/local-rig/acceptance/task-wide.toml
BIZ_DATE=2026-08-14
EMPTY_DATE=2026-08-15
RESULTS=()

cd "$RIG_ROOT"

compose() {
  docker compose "$@"
}

mysql_exec() {
  compose exec -T mysql mysql -N -B -uspike -pspike123 qbs -e "$1" 2>/dev/null
}

fail() {
  echo "    FAIL: $*" >&2
  return 1
}

assert_eq() {
  local label=$1 expected=$2 actual=$3
  [[ "$actual" == "$expected" ]] || fail "$label expected=$expected actual=$actual"
}

assert_json_eq() {
  local file=$1 filter=$2 expected=$3 label=$4 actual
  actual=$(jq -r "$filter" "$file") || return 1
  assert_eq "$label" "$expected" "$actual"
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

stop_client_process() {
  local pid_file=$1 label=$2
  compose exec -T client sh -c '
    pid_file=$1
    label=$2
    test -f "$pid_file" || exit 0
    pid=$(cat "$pid_file")
    kill -0 "$pid" 2>/dev/null || { rm -f "$pid_file"; exit 0; }
    kill "$pid" 2>/dev/null || true
    attempt=0
    while kill -0 "$pid" 2>/dev/null; do
      attempt=$((attempt + 1))
      test "$attempt" -lt 100 || { echo "timed out stopping $label" >&2; exit 1; }
      sleep 0.05
    done
    rm -f "$pid_file"
  ' sh "$pid_file" "$label"
}

stop_proxy() {
  stop_client_process /tmp/m1-proxy.pid "commit-drop proxy"
}

stop_sink() {
  stop_client_process /tmp/m1-sink.pid sink
}

cleanup() {
  stop_proxy
  compose exec -T client sh -c \
    'test ! -f /tmp/m1-source.pid || ! kill -0 "$(cat /tmp/m1-source.pid)" 2>/dev/null || kill -KILL "$(cat /tmp/m1-source.pid)"' \
    >/dev/null 2>&1 || true
  stop_sink
  drop_orphan_staging >/dev/null 2>&1 || true
  rm -rf "$WORK_ROOT"
}
trap cleanup EXIT

start_sink() {
  stop_sink || return 1
  compose exec -T client rm -f /tmp/m1-sink.pid /tmp/m1-sink.jsonl || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > /tmp/m1-sink.pid; exec $SINK_BIN --config $SINK_CONFIG > /tmp/m1-sink.jsonl 2>&1" || return 1
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    if compose exec -T client curl -sS -o /dev/null http://127.0.0.1:18080/v1/runs/not-a-run; then
      return 0
    fi
    sleep 0.1
  done
  compose exec -T client cat /tmp/m1-sink.jsonl >&2 || true
  fail "sink did not become ready"
}

drop_orphan_staging() {
  local table tables
  tables=$(mysql_exec "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='qbs' AND TABLE_NAME REGEXP '^M1_(NARROW|WIDE)__stg_'") || return 1
  while IFS= read -r table; do
    [[ -z "$table" ]] && continue
    [[ "$table" =~ ^M1_(NARROW|WIDE)__stg_[0-9]{14}_[0-9a-f]{6}$ ]] ||
      return 1
    mysql_exec "DROP TABLE IF EXISTS \`$table\`" || return 1
  done <<< "$tables"
}

reset_sink_state() {
  stop_proxy || return 1
  stop_sink || return 1
  drop_orphan_staging || return 1
  start_sink
}

prepare_rig() {
  for command in docker jq curl shasum; do
    command -v "$command" >/dev/null || { echo "missing required command: $command" >&2; return 1; }
  done

  ./scripts/up.sh || return 1
  docker run --rm --platform linux/arm64 \
    -v "$REPO_ROOT:/workspace" -w /workspace \
    -v qbs-cargo-registry:/usr/local/cargo/registry \
    rust:1-bookworm cargo build --release --workspace || return 1
  docker cp "$REPO_ROOT/target/release/db-qbs-source" qbs-client:"$SOURCE_BIN" || return 1
  docker cp "$REPO_ROOT/target/release/db-qbs-sink" qbs-client:"$SINK_BIN" || return 1

  compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE \
    @/workspace/docs/spikes/fixtures/local-rig/acceptance/oracle.sql || return 1
  compose exec -T mysql mysql -uspike -pspike123 qbs \
    < "$ACCEPTANCE_ROOT/mysql.sql" || return 1
  drop_orphan_staging || return 1
  start_sink
}

run_source() {
  local task=$1 biz_date=$2 log=$3 config=${4:-$SOURCE_CONFIG}
  compose exec -T client "$SOURCE_BIN" \
    --config "$config" --task "$task" --biz-date "$biz_date" > "$log"
}

source_run_id() {
  jq -r 'select(.event == "run_opened") | .run_id' "$1" | head -1
}

assert_run_id() {
  local label=$1 run_id=$2
  [[ "$run_id" =~ ^[0-9]{14}_[0-9a-f]{6}$ ]] || fail "$label is not an ADR-0016 run_id: $run_id"
}

assert_source_success() {
  local log=$1 expected_rows=$2 batch_events source_batches run_id
  jq -e . "$log" >/dev/null || return 1
  assert_json_eq "$log" \
    'select(.event == "run_finished") | .terminal' SUCCEEDED "terminal" || return 1
  assert_json_eq "$log" \
    'select(.event == "run_finished") | .source_rows | numbers' "$expected_rows" "source rows" || return 1
  jq -e '
    select(.event == "run_finished")
    | [.fetch_ms, .push_ms, .cursor_ms, .commit_ms, .count_ms, .purged_rows]
    | all(.[]; type == "number")
  ' "$log" >/dev/null || fail "successful run is missing numeric terminal measurements" || return 1
  jq -s -e '
    [.[] | select(.event == "batch_pushed")]
    | all(.[]; ((.rows | type) == "number" and (.bytes | type) == "number"))
  ' "$log" >/dev/null || fail "successful run is missing numeric per-batch measurements" || return 1
  source_batches=$(jq -er \
    'select(.event == "run_finished") | .source_batches | numbers' "$log") ||
    fail "successful run is missing a numeric source_batches total" || return 1
  batch_events=$(jq -s '[.[] | select(.event == "batch_pushed")] | length' "$log") || return 1
  assert_eq "source batch event count" "$source_batches" "$batch_events" || return 1
  jq -s -e --argjson source_batches "$source_batches" '
    [.[] | select(.event == "batch_pushed") | .seq]
    == [range(1; $source_batches + 1)]
  ' "$log" >/dev/null || fail "batch event sequence is not exactly 1..source_batches" || return 1
  local batch_sum
  batch_sum=$(jq -s '[.[] | select(.event == "batch_pushed") | .rows] | add // 0' "$log") || return 1
  assert_eq "sum(batch rows)" "$expected_rows" "$batch_sum" || return 1
  run_id=$(source_run_id "$log") || return 1
  assert_run_id "run_id from run_opened" "$run_id"
}

narrow_hash() {
  mysql_exec "SELECT CONCAT_WS('|', ROW_ID, V_TEXT, DATE_FORMAT(D_BIZ,'%Y-%m-%d %H:%i:%s')) FROM M1_NARROW ORDER BY ROW_ID" |
    shasum -a 256 | cut -d' ' -f1
}

scenario_wide() {
  local log="$WORK_ROOT/wide.jsonl" max_bytes max_rows max_rows_per_insert
  mysql_exec "DELETE FROM M1_WIDE" >/dev/null || return 1
  run_source "$WIDE_TASK" "$BIZ_DATE" "$log" || return 1
  assert_source_success "$log" 100000 || return 1
  assert_eq "wide target rows" 100000 "$(mysql_exec "SELECT COUNT(*) FROM M1_WIDE")" || return 1
  max_rows=$(jq -s '[.[] | select(.event == "batch_pushed") | .rows] | max' "$log") || return 1
  max_rows_per_insert=$(( 65535 / 70 ))
  (( max_rows > max_rows_per_insert )) ||
    fail "70-column batches did not exercise placeholder sub-statement splitting" || return 1
  max_bytes=$(jq -s '[.[] | select(.event == "batch_pushed") | .bytes] | max' "$log") || return 1
  (( max_bytes <= 16 * 1024 * 1024 )) || fail "wide batch exceeded 16 MiB: $max_bytes"
}

scenario_narrow() {
  local log="$WORK_ROOT/narrow.jsonl" max_rows max_rows_per_insert
  mysql_exec "DELETE FROM M1_NARROW" >/dev/null || return 1
  run_source "$NARROW_TASK" "$BIZ_DATE" "$log" || return 1
  assert_source_success "$log" 100000 || return 1
  assert_eq "narrow target rows" 100000 "$(mysql_exec "SELECT COUNT(*) FROM M1_NARROW")" || return 1
  max_rows=$(jq -s '[.[] | select(.event == "batch_pushed") | .rows] | max' "$log") || return 1
  max_rows_per_insert=$(( 65535 / 3 ))
  (( max_rows <= max_rows_per_insert )) ||
    fail "3-column batch requires placeholder sub-statement splitting"
}

scenario_source_kill() {
  local direct="$WORK_ROOT/kill-direct.jsonl" killed="$WORK_ROOT/kill.jsonl"
  local rerun="$WORK_ROOT/kill-rerun.jsonl" baseline target_with_sentinel after old_run new_run pid attempt
  mysql_exec "DELETE FROM M1_NARROW" >/dev/null || return 1
  run_source "$NARROW_TASK" "$BIZ_DATE" "$direct" || return 1
  assert_source_success "$direct" 100000 || return 1
  baseline=$(narrow_hash) || return 1
  mysql_exec "INSERT INTO M1_NARROW (ROW_ID,V_TEXT,D_BIZ) VALUES (99999999,'kill-sentinel','$BIZ_DATE')" >/dev/null || return 1
  target_with_sentinel=$(narrow_hash) || return 1
  [[ "$target_with_sentinel" != "$baseline" ]] || fail "target sentinel did not change the direct-run baseline" || return 1

  compose exec -T client rm -f /tmp/m1-source.pid /tmp/m1-kill.jsonl || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > /tmp/m1-source.pid; exec $SOURCE_BIN --config $SOURCE_CONFIG --task $NARROW_TASK --biz-date $BIZ_DATE > /tmp/m1-kill.jsonl" || return 1
  for (( attempt = 1; attempt <= 400; attempt++ )); do
    if compose exec -T client sh -c \
      "test -f /tmp/m1-kill.jsonl && grep -q '\"event\":\"batch_pushed\"' /tmp/m1-kill.jsonl"; then
      break
    fi
    sleep 0.05
  done
  compose exec -T client grep -q '"event":"batch_pushed"' /tmp/m1-kill.jsonl ||
    fail "source did not reach STREAMING before the kill deadline" || return 1
  pid=$(compose exec -T client cat /tmp/m1-source.pid | tr -d '\r') || return 1
  compose exec -T client kill -KILL "$pid" ||
    fail "source completed before the STREAMING kill could be injected" || return 1
  compose exec -T client cat /tmp/m1-kill.jsonl > "$killed" || return 1
  jq -e 'select(.event == "batch_pushed")' "$killed" >/dev/null ||
    fail "killed source log has no completed batch" || return 1
  if jq -s -e 'any(.[]; .event == "stage_changed" and .stage == "COMMITTING")' \
    "$killed" >/dev/null; then
    fail "source reached COMMITTING before the STREAMING kill was injected"
    return 1
  fi
  old_run=$(source_run_id "$killed") || return 1
  assert_run_id "killed source run_id" "$old_run" || return 1
  after=$(narrow_hash) || return 1
  assert_eq "target hash after source kill" "$target_with_sentinel" "$after" || return 1

  reset_sink_state || return 1
  run_source "$NARROW_TASK" "$BIZ_DATE" "$rerun" || return 1
  assert_source_success "$rerun" 100000 || return 1
  new_run=$(source_run_id "$rerun")
  [[ "$new_run" != "$old_run" ]] || fail "rerun reused killed run_id" || return 1
  assert_eq "rerun hash versus direct run" "$baseline" "$(narrow_hash)"
}

start_commit_drop_proxy() {
  stop_proxy || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > /tmp/m1-proxy.pid; exec python3 /workspace/docs/spikes/fixtures/local-rig/acceptance/commit-drop-proxy.py" || return 1
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    if compose exec -T client curl -sS -o /dev/null http://127.0.0.1:18081/v1/runs/not-a-run; then
      return 0
    fi
    sleep 0.1
  done
  fail "commit-drop proxy did not become ready"
}

scenario_commit_disconnect() {
  local log="$WORK_ROOT/commit-disconnect.jsonl" status
  reset_sink_state || return 1
  mysql_exec "DELETE FROM M1_NARROW WHERE D_BIZ >= '$EMPTY_DATE' AND D_BIZ < '$EMPTY_DATE' + INTERVAL 1 DAY; INSERT INTO M1_NARROW (ROW_ID,V_TEXT,D_BIZ) VALUES (99999999,'commit-sentinel','$EMPTY_DATE')" >/dev/null || return 1
  start_commit_drop_proxy || return 1
  run_source "$NARROW_TASK" "$EMPTY_DATE" "$log" "$DROP_CONFIG"
  status=$?
  assert_eq "commit disconnect source exit" 1 "$status" || return 1
  jq -e . "$log" >/dev/null || return 1
  assert_json_eq "$log" \
    'select(.event == "commit_diagnosed") | .terminal' SWAPPED "commit diagnostic terminal" || return 1
  jq -e 'select(.event == "commit_diagnosed" and (.message | contains("目标表已是新数据")))' \
    "$log" >/dev/null || fail "commit diagnostic did not explain that target was swapped" || return 1
  assert_json_eq "$log" \
    'select(.event == "run_finished") | .stage' COMMITTING "failed stage" || return 1
  assert_eq "commit disconnect target rows" 0 \
    "$(mysql_exec "SELECT COUNT(*) FROM M1_NARROW WHERE D_BIZ >= '$EMPTY_DATE' AND D_BIZ < '$EMPTY_DATE' + INTERVAL 1 DAY")"
}

scenario_empty() {
  local log="$WORK_ROOT/empty.jsonl"
  mysql_exec "DELETE FROM M1_NARROW; INSERT INTO M1_NARROW (ROW_ID,V_TEXT,D_BIZ) VALUES (1,'old-1','$EMPTY_DATE'),(2,'old-2','$EMPTY_DATE'),(3,'old-3','$EMPTY_DATE'),(4,'old-4','$EMPTY_DATE'),(5,'old-5','$EMPTY_DATE'),(6,'old-6','$EMPTY_DATE'),(7,'old-7','$EMPTY_DATE')" >/dev/null || return 1
  run_source "$NARROW_TASK" "$EMPTY_DATE" "$log" || return 1
  assert_source_success "$log" 0 || return 1
  assert_json_eq "$log" \
    'select(.event == "run_finished") | .purged_rows' 7 "purged rows" || return 1
  assert_eq "empty date target rows" 0 \
    "$(mysql_exec "SELECT COUNT(*) FROM M1_NARROW WHERE D_BIZ >= '$EMPTY_DATE' AND D_BIZ < '$EMPTY_DATE' + INTERVAL 1 DAY")"
}

api_post() {
  local path=$1 payload=$2 response
  response=$(printf '%s' "$payload" | compose exec -T client curl -sS \
    -H 'Content-Type: application/json' --data-binary @- \
    -w $'\n%{http_code}' "http://127.0.0.1:18080$path") || return 1
  API_STATUS=${response##*$'\n'}
  API_BODY=${response%$'\n'*}
}

open_narrow_run() {
  local run_id=$1 payload
  payload=$(jq -nc --arg run_id "$run_id" --arg date "$BIZ_DATE" '{
    run_id: $run_id,
    target_table: "M1_NARROW",
    target_date_col: "D_BIZ",
    biz_date: $date,
    source_columns: [
      {name:"ROW_ID", type:"NUMBER", precision:8, scale:0, length:null},
      {name:"V_TEXT", type:"VARCHAR2", precision:null, scale:null, length:200},
      {name:"D_BIZ", type:"DATE", precision:null, scale:null, length:null}
    ]
  }') || return 1
  api_post /v1/runs "$payload" || return 1
  assert_eq "open status" 200 "$API_STATUS"
}

assert_verify_failure() {
  local body=$1 expected_staged=$2 expected_received=$3 expected_reported=$4 phrase=$5
  assert_eq "verify code" VERIFY_FAILED "$(jq -r '.error.code' <<<"$body")" || return 1
  assert_eq "source_rows" 2 "$(jq -r '.error.details.source_rows' <<<"$body")" || return 1
  assert_eq "staged_rows" "$expected_staged" "$(jq -r '.error.details.staged_rows' <<<"$body")" || return 1
  assert_eq "source_batches" 1 "$(jq -r '.error.details.source_batches' <<<"$body")" || return 1
  assert_eq "received_batches" "$expected_received" "$(jq -r '.error.details.received_batches' <<<"$body")" || return 1
  assert_eq "sink_reported_rows" "$expected_reported" "$(jq -r '.error.details.sink_reported_rows' <<<"$body")" || return 1
  jq -e '.error.details.count_ms | numbers' <<<"$body" >/dev/null || return 1
  grep -Fq "$phrase" <<<"$body" || fail "VERIFY_FAILED message missing: $phrase"
}

scenario_verification_failures() {
  local run_loss run_missing staging target_before target_after
  reset_sink_state || return 1
  mysql_exec "DELETE FROM M1_NARROW" >/dev/null || return 1
  target_before=$(narrow_hash) || return 1

  run_loss="$(date -u +%Y%m%d%H%M%S)_d10001"
  open_narrow_run "$run_loss" || return 1
  staging=$(jq -r '.staging_table' <<<"$API_BODY") || return 1
  api_post "/v1/runs/$run_loss/batches" \
    '{"seq":1,"rows":[["1","one","2026-08-14 00:00:00"],["2","two","2026-08-14 00:00:00"]]}' || return 1
  assert_eq "batch status" 200 "$API_STATUS" || return 1
  [[ "$staging" =~ ^M1_NARROW__stg_[0-9]{14}_[0-9a-f]{6}$ ]] || return 1
  mysql_exec "DELETE FROM \`$staging\` WHERE ROW_ID=2" >/dev/null || return 1
  api_post "/v1/runs/$run_loss/commit" '{"total_batches":1,"total_rows":2}' || return 1
  assert_eq "loss commit status" 409 "$API_STATUS" || return 1
  assert_verify_failure "$API_BODY" 1 1 2 "数据在写入 MySQL 的过程中丢失" || return 1
  printf '%s\n' "$API_BODY" > "$WORK_ROOT/verify-loss.json"

  run_missing="$(date -u +%Y%m%d%H%M%S)_d10002"
  open_narrow_run "$run_missing" || return 1
  api_post "/v1/runs/$run_missing/commit" '{"total_batches":1,"total_rows":2}' || return 1
  assert_eq "missing commit status" 409 "$API_STATUS" || return 1
  assert_verify_failure "$API_BODY" 0 0 0 "有批次未送达" || return 1
  printf '%s\n' "$API_BODY" > "$WORK_ROOT/verify-missing.json"

  target_after=$(narrow_hash) || return 1
  assert_eq "target hash after both verification failures" "$target_before" "$target_after"
}

scenario_canonical() {
  ./scripts/run-canon-gate.sh
}

write_report() {
  mkdir -p "$(dirname "$REPORT")" || return 1
  {
    echo "# M1 rig acceptance report"
    echo
    echo "- Generated (UTC): $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Git commit: $(git -C "$REPO_ROOT" rev-parse HEAD)"
    echo "- Business date: $BIZ_DATE"
    echo
    echo "## Scenarios"
    echo
    echo "| Scenario | Result |"
    echo "|---|---|"
    local scenario index
    for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
      scenario=${SCENARIOS[$index]}
      echo "| $scenario | ${RESULTS[$index]:-FAIL} |"
    done
    echo
    echo "## Measurements"
    echo
    echo "| Log | Rows | Batches | Fetch ms | Push ms | Push/cursor | Cursor ms | Commit ms | COUNT ms | Purged | Batch rows min/max | Bytes min/p50/max |"
    echo "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    local log terminal name rows batches fetch push cursor commit count purged row_dist byte_dist ratio
    while IFS= read -r log; do
      terminal=$(jq -sc '[.[] | select(.event == "run_finished")] | last // {}' "$log") || return 1
      name=$(basename "$log")
      rows=$(jq -r '.source_rows // "n/a"' <<<"$terminal")
      batches=$(jq -r '.source_batches // "n/a"' <<<"$terminal")
      fetch=$(jq -r '.fetch_ms // "n/a"' <<<"$terminal")
      push=$(jq -r '.push_ms // "n/a"' <<<"$terminal")
      cursor=$(jq -r '.cursor_ms // "n/a"' <<<"$terminal")
      commit=$(jq -r '.commit_ms // "n/a"' <<<"$terminal")
      count=$(jq -r '.count_ms // "n/a"' <<<"$terminal")
      purged=$(jq -r '.purged_rows // "n/a"' <<<"$terminal")
      ratio=$(jq -r '
        if ((.push_ms | type) != "number") or ((.cursor_ms | type) != "number") or .cursor_ms == 0
        then "n/a"
        else (((.push_ms * 10000 / .cursor_ms | floor) / 100 | tostring) + "%")
        end
      ' <<<"$terminal")
      row_dist=$(jq -sr '[.[] | select(.event == "batch_pushed") | .rows] | if length == 0 then "n/a" else ((min|tostring)+"/"+(max|tostring)) end' "$log")
      byte_dist=$(jq -sr '[.[] | select(.event == "batch_pushed") | .bytes] | sort | if length == 0 then "n/a" else ((.[0]|tostring)+"/"+(.[length/2|floor]|tostring)+"/"+(.[-1]|tostring)) end' "$log")
      echo "| $name | $rows | $batches | $fetch | $push | $ratio | $cursor | $commit | $count | $purged | $row_dist | $byte_dist |"
    done < <(find "$WORK_ROOT" -name '*.jsonl' -type f | sort)
    echo
    echo "## Verification evidence"
    echo
    echo "| Failure | source_rows | staged_rows | source_batches | received_batches | sink_reported_rows | COUNT ms | Message |"
    echo "|---|---:|---:|---:|---:|---:|---:|---|"
    local evidence kind
    for evidence in "$WORK_ROOT/verify-loss.json" "$WORK_ROOT/verify-missing.json"; do
      [[ -f "$evidence" ]] || continue
      kind=$(basename "$evidence" .json)
      jq -r --arg kind "$kind" '
        .error.details as $d |
        "| \($kind) | \($d.source_rows) | \($d.staged_rows) | \($d.source_batches) | \($d.received_batches) | \($d.sink_reported_rows) | \($d.count_ms) | \(.error.message | gsub("\\|"; "\\|")) |"
      ' "$evidence"
    done
    echo
    echo "## Canonical-form evidence"
    echo
    if [[ -f "$WORK_ROOT/canonical-form.out" ]]; then
      echo '```text'
      cat "$WORK_ROOT/canonical-form.out"
      echo '```'
    else
      echo "No canonical-form output was captured."
    fi
    echo
    echo "## Review gates"
    echo
    echo "- Review ADR-0007 serial push when Push/cursor exceeds 50%."
    echo "- Compare Commit ms and COUNT ms with ADR-0010's 30-minute read timeout."
    echo "- Compare batch byte max with ADR-0011's 16 MiB budget; row counts are observed, not assumed."
  } > "$REPORT"
  chmod 600 "$REPORT"
}

run_scenario() {
  local name=$1 function=$2 output="$WORK_ROOT/$name.out" index
  index=$(scenario_index "$name") || { fail "unknown scenario: $name"; return 1; }
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

echo "==> prepare M1 acceptance rig"
if ! prepare_rig; then
  echo "rig preparation failed" >&2
  exit 1
fi

run_scenario wide-100k scenario_wide
run_scenario narrow-100k scenario_narrow
run_scenario source-kill-rerun scenario_source_kill
run_scenario commit-disconnect scenario_commit_disconnect
run_scenario empty-result scenario_empty
run_scenario verification-failures scenario_verification_failures
run_scenario canonical-form scenario_canonical

write_report || { echo "failed to write report" >&2; exit 1; }
echo "report: $REPORT"

failed=0
for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
  [[ "${RESULTS[$index]:-FAIL}" == PASS ]] || failed=$((failed + 1))
done
if (( failed > 0 )); then
  echo "M1 acceptance: FAIL ($failed/${#SCENARIOS[@]} scenarios)"
  exit 1
fi
echo "M1 acceptance: PASS (${#SCENARIOS[@]}/${#SCENARIOS[@]} scenarios)"
