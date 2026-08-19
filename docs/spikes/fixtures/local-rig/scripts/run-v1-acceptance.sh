#!/usr/bin/env bash
# Issue #135 / ADR-0040: 第一版验收的第四个入口，场景 C1-C6。
#
# 判据依据：**ADR-0040 §4（C 系列那张表）与 §3（内存形状）**，一条都不在这里重新推导。
#
# **另起入口，不往 M1/M2/M3 里塞**（ADR-0040 §2）：既有入口的场景集是该里程碑的常量，
# 往里塞会让「9/9」「A1-A14」「B1-B6」这些历史锚点漂移。**字母全局唯一：M2 是 A、
# M3 是 B、第一版是 C，任何情况下不复用、不重编。**
#
# 六个场景对应第一版的六件事：
#
#   * C1 数据源 CRUD 与测试连接（ADR-0037 §6/§7、ADR-0039 §3/§4）
#   * C2 字段映射与目标端列面（ADR-0038 §2/§3）
#   * C3 用户可填筛选条件（ADR-0036 §1/§2）
#   * C4 主键 upsert 的幂等（ADR-0035 §1）
#   * C5 映射预检三分支（ADR-0038 §5）
#   * C6 内存形状（ADR-0040 §3）
#
# **不设 C7 收「10w/100M」**（ADR-0040 §1/§4）：行数与行宽那一半在 M1 的 `wide-100k`，
# 内存形状那一半在 C6，两处合起来兑现客户第 5 条需求。重复设一个只会有两个各自漂移的真源。
#
# **跑完默认不清场**（所有者 2026-08-19 裁定）：C1/C2 建出来的两条数据源与那个不同名映射的
# 任务，正是 X1-X8 走查过半条目要用的数据，清干净等于让人再手工造一遍。要清场传 `--clean`。
#
# 门槛：真跑一律派到 mac，`M2_HOST_CARGO_TARGET=x86_64-apple-darwin`、
# `M2_ORACLE_CLIENT_LIB_DIR` 指向宿主的 Instant Client。报告落
# `docs/spikes/fixtures/local-rig/v1-acceptance-<UTC>.md`。**不许写「通过」**：
# 报告贴的是逐场景实际观察，没跑的写明「未跑及为什么」。

set -uo pipefail

SCENARIOS=(
  C1-datasource-crud
  C2-column-mapping
  C3-user-conditions
  C4-upsert-idempotence
  C5-precheck-branches
  C6-memory-shape
)

CLEAN=""
if [[ "${1:-}" == "--list" ]]; then
  printf '%s\n' "${SCENARIOS[@]}"
  exit 0
fi
if [[ "${1:-}" == "--clean" ]]; then
  CLEAN=1
  shift
fi
if (( $# != 0 )); then
  echo "usage: $0 [--list] [--clean]" >&2
  exit 2
fi

umask 077
RIG_ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO_ROOT=$(cd "$RIG_ROOT/../../../.." && pwd)
ACCEPTANCE_ROOT="$RIG_ROOT/acceptance"
WORK_ROOT=$(mktemp -d)
REPORT=${V1_REPORT:-"$RIG_ROOT/v1-acceptance-$(date -u +%Y%m%dT%H%M%SZ).md"}
HOST_TARGET=${M2_HOST_CARGO_TARGET:-}
HOST_BIN_DIR="$REPO_ROOT/target${HOST_TARGET:+/$HOST_TARGET}/release"
SOURCE_BIN="$HOST_BIN_DIR/db-qbs-source"
REAL_SOURCE_BIN="$HOST_BIN_DIR/db-qbs-source-run"
MEMORY_WRAPPER="$ACCEPTANCE_ROOT/v1-memory-wrapper.py"
MEMORY_LOG="$WORK_ROOT/c6-maxrss.jsonl"
SINK_BIN=/usr/local/bin/db-qbs-sink
SINK_LOG=/tmp/v1-sink.jsonl
SINK_PID_FILE=/tmp/v1-sink.pid
SOURCE_CONFIG="$WORK_ROOT/source.toml"
SOURCE_DATA="$WORK_ROOT/source-data"
SOURCE_LOG="$WORK_ROOT/source.jsonl"
SOURCE_URL=http://127.0.0.1:18088
SOURCE_PORT=18088
SINK_URL=http://127.0.0.1:18080
BIZ_DATE=2026-08-14
ORACLE_DATASOURCE_NAME="V1 源库"
TARGET_DATASOURCE_NAME="V1 目标库"
ORACLE_DATASOURCE_ID=""
TARGET_DATASOURCE_ID=""
RESULTS=()
SOURCE_PID=""
API_STATUS=""
API_BODY=""
C6_RECORD=""
C2_TASK_ID=""
KEEP_TASK_NAME="C1 引用检查（删除会被 409 拒）"

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

# 数值断言分开写：`assert_eq` 会把 121 与 121.0 判成不等，而内存那几条比的是大小不是字面。
assert_le() {
  local label=$1 left=$2 right=$3
  echo "$label: $left <= $right"
  (( left <= right )) || fail "$label expected $left <= $right"
}

assert_gt() {
  local label=$1 left=$2 right=$3
  echo "$label: $left > $right"
  (( left > right )) || fail "$label expected $left > $right"
}

# 失败必须出声（#138 的教训）：`cmd || return 1` 的调用点不打出 SQL 或 message 的话，
# 报告里只剩 docker 的 `exit status 1`，指不到是哪一步。
mysql_exec() {
  compose exec -T mysql mysql -N -B -uspike -pspike123 qbs -e "$1" 2>/dev/null | tr -d '\r' ||
    fail "mysql 执行失败：$1"
}

# COMMIT 由本函数补在自己一行上：SQL*Plus 对「一行里两条语句」的处理不可靠，
# 而这里改的是 C4④ 要重跑的源值，改没改成必须当场看得出来。
oracle_exec() {
  local statement=$1
  printf 'WHENEVER SQLERROR EXIT SQL.SQLCODE\n%s\nCOMMIT;\nEXIT\n' "$statement" |
    compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE ||
    fail "oracle 执行失败：$statement"
}

mysql_staging_count() {
  local table=$1
  mysql_exec "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'qbs' AND table_name LIKE '${table}__stg_%'"
}

api() {
  local method=$1 path=$2 payload=${3:-} response
  if [[ "$method" == GET || "$method" == DELETE ]]; then
    response=$(curl -sS -X "$method" -H 'Accept: application/json' -w $'\n%{http_code}' "$SOURCE_URL$path") || return 1
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

# 第三个参数是轮询次数上限（每次 0.05 秒）。默认 400 = 20 秒够小场景用；
# C6 的 10 万行档在模拟层 Oracle 上要一分钟往上，必须显式放长——
# 用默认值的话它会在跑到一半时被判成「没等到」，而那是台架的错、不是产品的错。
wait_for_run() {
  local record=$1 filter=$2 attempts=${3:-400} attempt
  for (( attempt = 1; attempt <= attempts; attempt++ )); do
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
  local run_executable=$1
  mkdir -p "$SOURCE_DATA"
  # oracle_* 三件套已随 ADR-0037 §10 退役——真相源是数据源库。
  cat > "$SOURCE_CONFIG" <<EOF
oracle_client_lib_dir = "$M2_ORACLE_CLIENT_LIB_DIR"
sink_base_url = "$SINK_URL"
listen = "127.0.0.1:$SOURCE_PORT"
data_dir = "$SOURCE_DATA"
run_executable = "$run_executable"
history_retention_days = 90
EOF
}

# `run_executable` 与它的环境在编排进程启动时定死，所以 C6 的 wrapper 只能整段换一次进程：
# 不带参数起的是直调真二进制的常态；带 wrapper 起的是 C6 专用的量内存态。
start_source() {
  local run_executable=${1:-$REAL_SOURCE_BIN}
  stop_source || return 1
  ensure_source_port_free || return 1
  : > "$SOURCE_LOG"
  write_source_config "$run_executable" || return 1
  if [[ "$run_executable" == "$MEMORY_WRAPPER" ]]; then
    V1_REAL_SOURCE_BIN="$REAL_SOURCE_BIN" V1_MEMORY_FILE="$MEMORY_LOG" \
      nohup "$SOURCE_BIN" --config "$SOURCE_CONFIG" > "$SOURCE_LOG" 2>&1 &
  else
    nohup "$SOURCE_BIN" --config "$SOURCE_CONFIG" > "$SOURCE_LOG" 2>&1 &
  fi
  SOURCE_PID=$!
  wait_for_source || return 1
  ensure_datasources || return 1
  kill -0 "$SOURCE_PID" 2>/dev/null && return 0
  cat "$SOURCE_LOG" >&2 || true
  return 1
}

stop_sink() {
  compose exec -T client sh -c '
    test -f /tmp/v1-sink.pid || exit 0
    pid=$(cat /tmp/v1-sink.pid)
    kill -TERM "$pid" 2>/dev/null || true
    i=0
    while kill -0 "$pid" 2>/dev/null; do
      i=$((i + 1)); test "$i" -lt 100 || { kill -KILL "$pid" 2>/dev/null || true; break; }
      sleep 0.05
    done
    rm -f /tmp/v1-sink.pid
  '
}

stop_all_sinks() {
  compose exec -T client sh -c '
    self=$$
    for entry in /proc/[0-9]*; do
      pid=${entry#/proc/}
      test "$pid" != "$self" || continue
      cmdline=$(tr "\0" " " < "$entry/cmdline" 2>/dev/null) || continue
      case "$cmdline" in
        *db-qbs-sink*) kill -KILL "$pid" 2>/dev/null || true ;;
      esac
    done
    rm -f /tmp/m1-sink.pid /tmp/m2-sink.pid /tmp/m3-sink.pid /tmp/v1-sink.pid
  '
}

start_sink() {
  stop_sink || return 1
  compose exec -T client rm -f "$SINK_LOG" || return 1
  compose exec -T -d client sh -c \
    "echo \$\$ > $SINK_PID_FILE; exec $SINK_BIN --config /workspace/docs/spikes/fixtures/local-rig/acceptance/sink.toml > $SINK_LOG 2>&1" || return 1
  local attempt
  for (( attempt = 1; attempt <= 100; attempt++ )); do
    if curl -sS -o /dev/null "$SINK_URL/v1/runs/not-a-run" 2>/dev/null; then
      if compose exec -T client sh -c "kill -0 \"\$(cat $SINK_PID_FILE)\" 2>/dev/null"; then
        return 0
      fi
      compose exec -T client cat "$SINK_LOG" >&2 || true
      return 1
    fi
    sleep 0.1
  done
  compose exec -T client cat "$SINK_LOG" >&2 || true
  fail "sink did not become ready"
}

# sink 的高水位。`VmHWM` 是**进程存续期**的峰值、由内核维护、只增不减——
# 正因为只增不减，C6 的两档之间必须重启 sink（ADR-0040 §3.5）。单位 kB。
sink_vmhwm_kb() {
  local value
  value=$(compose exec -T client sh -c "grep VmHWM /proc/\$(cat $SINK_PID_FILE)/status" 2>/dev/null |
    awk '{print $2}' | tr -d '\r') || return 1
  [[ "$value" =~ ^[0-9]+$ ]] || fail "VmHWM 读回来的不是数字：$value" || return 1
  printf '%s' "$value"
}

# source 那一趟的高水位：wrapper 每跑完一个子进程追加一行，取最后一行即本趟。单位字节。
source_maxrss_bytes() {
  local line value
  [[ -s "$MEMORY_LOG" ]] || fail "wrapper 没有写下任何 ru_maxrss——run_executable 接错了？" || return 1
  line=$(tail -1 "$MEMORY_LOG") || return 1
  value=$(jq -r '.ru_maxrss_bytes' <<<"$line") || return 1
  [[ "$value" =~ ^[0-9]+$ ]] || fail "ru_maxrss 读回来的不是数字：$line" || return 1
  printf '%s' "$value"
}

# 数据源是任务定义的前提（ADR-0037 §1）：任务绑的是 id，不是连接串。建过就复用。
datasource_id_by_name() {
  local name=$1
  api GET /api/datasources || return 1
  jq -r --arg name "$name" 'map(select(.name == $name)) | .[0].datasource_id // empty' <<<"$API_BODY"
}

datasource_count() {
  api GET /api/datasources || return 1
  jq 'length' <<<"$API_BODY"
}

ensure_datasource() {
  local name=$1 payload=$2 existing
  existing=$(datasource_id_by_name "$name") || return 1
  if [[ -n "$existing" ]]; then
    printf '%s' "$existing"
    return 0
  fi
  api POST /api/datasources "$payload" || return 1
  [[ "$API_STATUS" == 201 ]] ||
    fail "create datasource $name status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.datasource_id' <<<"$API_BODY"
}

oracle_datasource_payload() {
  jq -nc --arg name "$ORACLE_DATASOURCE_NAME" '{
    name:$name, kind:"oracle",
    connect_string:"//127.0.0.1:1521/XE", username:"spike", password:"spike123"
  }'
}

# 目标端连接是**由 sink 用**的（ADR-0037 §1：凭据随 run 报文过线），sink 跑在 client 容器里，
# 所以给的是容器内的 `mysql`。Oracle 那条相反：source 跑在宿主机上，走发布出来的端口。
target_datasource_payload() {
  local name=$1 password=$2
  jq -nc --arg name "$name" --arg password "$password" '{
    name:$name, kind:"mysql",
    host:"mysql", port:3306, username:"spike", password:$password, database:"qbs"
  }'
}

ensure_datasources() {
  ORACLE_DATASOURCE_ID=$(ensure_datasource "$ORACLE_DATASOURCE_NAME" "$(oracle_datasource_payload)") || return 1
  TARGET_DATASOURCE_ID=$(ensure_datasource "$TARGET_DATASOURCE_NAME" \
    "$(target_datasource_payload "$TARGET_DATASOURCE_NAME" spike123)") || return 1
  [[ -n "$ORACLE_DATASOURCE_ID" && -n "$TARGET_DATASOURCE_ID" ]] ||
    fail "datasource ids missing: oracle=$ORACLE_DATASOURCE_ID target=$TARGET_DATASOURCE_ID"
}

create_task() {
  local name=$1 spec=$2 payload
  payload=$(jq -nc --arg name "$name" --arg source "$ORACLE_DATASOURCE_ID" \
    --arg target "$TARGET_DATASOURCE_ID" --argjson spec "$spec" '{
      name:$name, source_datasource_id:$source, target_datasource_id:$target, spec:$spec
    }') || return 1
  api POST /api/tasks "$payload" || return 1
  [[ "$API_STATUS" == 201 ]] || fail "create task status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.task_id' <<<"$API_BODY"
}

start_task_run() {
  local task_id=$1 params=${2:-} payload
  if [[ -z "$params" ]]; then
    params=$(jq -nc --arg date "$BIZ_DATE" '{load_date:$date}') || return 1
  fi
  payload=$(jq -nc --arg task "$task_id" --argjson params "$params" \
    '{task_id:$task, run_params:$params}') || return 1
  api POST /api/runs "$payload" || return 1
  [[ "$API_STATUS" == 202 ]] || fail "start run status=$API_STATUS body=$API_BODY" || return 1
  jq -r '.run_record_id' <<<"$API_BODY"
}

# 跑一趟并等到终态；`API_BODY` 留着给调用处继续断言。第二个参数是等待上限（轮询次数）。
run_task_to_completion() {
  local task_id=$1 attempts=${2:-400} params=${3:-} record
  record=$(start_task_run "$task_id" "$params") || return 1
  wait_for_run "$record" '.live == false' "$attempts" || return 1
  printf '%s' "$record"
}

load_date_condition() {
  jq -nc '{
    column:"LOAD_DATE", operator:"eq", value_type:"date",
    parameter:"load_date", value_source:"runtime", constant:""
  }'
}

c2_spec() {
  # 源列名与目标列名成心取得不一样（ADR-0038 §2）：投影是 `a.SRC_NAME AS DEST_NAME`。
  jq -nc --argjson load_date "$(load_date_condition)" '{
    owner:"SPIKE", table:"T_V1_C2", target_table:"V1_C2",
    primary_key:["ROW_ID"],
    columns:[
      {source:"ROW_ID", target:"ROW_ID"},
      {source:"SRC_NAME", target:"DEST_NAME"},
      {source:"SRC_AMOUNT", target:"DEST_AMOUNT"},
      {source:"LOAD_DATE", target:"LOAD_DATE"}
    ],
    conditions:[$load_date]
  }'
}

scenario_c1() {
  start_source || return 1
  local before after body rename_id delete_status

  # ① Oracle 源与 MySQL 目标各一条（start_source 里的 ensure_datasources 已建）
  assert_eq "C1① oracle datasource created" true "$([[ -n "$ORACLE_DATASOURCE_ID" ]] && echo true || echo false)" || return 1
  assert_eq "C1① target datasource created" true "$([[ -n "$TARGET_DATASOURCE_ID" ]] && echo true || echo false)" || return 1
  api GET /api/datasources || return 1
  assert_eq "C1① both kinds present" true \
    "$(jq '[.[] | .kind] | (index("oracle") != null) and (index("mysql") != null)' <<<"$API_BODY")" || return 1

  # ② 凭据错的连接测连失败 → 存不进去。
  # **台架能证到哪儿要说清楚**：服务端的 POST /api/datasources 本身不测连，
  # 「测通才让存」那道门槛在对话框上（ADR-0039 §3，界面那一半归 X 走查）。这里证的是
  # 门槛依赖的那个事实——错口令的测连确实失败——外加失败之后库里没有多出一条。
  before=$(datasource_count) || return 1
  api POST /api/datasources/test-connection \
    "$(target_datasource_payload "V1 错口令草稿" wrong-password)" || return 1
  # 断言**失败得是口令那条路上的**，不满足于「不是 200」：草稿字段在协议上是 `#[serde(flatten)]`
  # 平铺的，早先多包了一层 `{draft:…}`，请求被 400 `missing field name` 挡在解析阶段就退了回来，
  # 「不是 200」照样成立——一条被 schema 错误白捡的断言，等于没有断言（ADR-0040 §7）。
  assert_eq "C1② bad-credential test-connection rejected" false \
    "$([[ "$API_STATUS" == 200 ]] && echo true || echo false)" || return 1
  assert_eq "C1② rejected by the credential, not by the request shape" false \
    "$(jq -r '(.error.message? // .message? // "") | test("JSON 请求体无效|missing field")' <<<"$API_BODY")" || return 1
  assert_eq "C1② failure carries a message" true \
    "$(jq 'has("message") or (.error? | type == "object")' <<<"$API_BODY")" || return 1
  after=$(datasource_count) || return 1
  assert_eq "C1② datasource count unchanged" "$before" "$after" || return 1
  echo "C1② test-connection status=$API_STATUS body=$API_BODY"

  # ③ API 视图连密文都不回（ADR-0037 §3）
  api GET /api/datasources || return 1
  body=$API_BODY
  # `has_password` 这个**布尔标志本身**正是 ADR-0037 §3 要求回的（下一条断言在正面要它），
  # 所以它必须从这条「不许出现口令相关键」里排除掉，否则两条断言互相打架、永远过不去。
  assert_eq "C1③ no password-ish key in view" 0 \
    "$(jq '[paths | last | select(type == "string") | select(. != "has_password")
            | select(test("password|secret|cipher|nonce"; "i"))] | length' <<<"$body")" || return 1
  assert_eq "C1③ no plaintext credential in view" false \
    "$(jq --arg secret spike123 'tostring | contains($secret)' <<<"$body")" || return 1
  assert_eq "C1③ has_password reported instead" true \
    "$(jq '[.[] | .has_password] | all' <<<"$body")" || return 1

  # ④ 删除被任务引用的数据源 → 409 且报文点名引用它的任务（ADR-0037 §7）
  C2_TASK_ID=$(create_task "$KEEP_TASK_NAME" "$(c2_spec)") || return 1
  api DELETE "/api/datasources/$TARGET_DATASOURCE_ID" || return 1
  delete_status=$API_STATUS
  assert_eq "C1④ delete referenced datasource" 409 "$delete_status" || return 1
  assert_eq "C1④ report names the task" true \
    "$(jq --arg name "$KEEP_TASK_NAME" '.error.tasks | index($name) != null' <<<"$API_BODY")" || return 1
  assert_eq "C1④ message repeats the task name" true \
    "$(jq --arg name "$KEEP_TASK_NAME" '.error.message | contains($name)' <<<"$API_BODY")" || return 1
  echo "C1④ 409 body: $API_BODY"

  # ⑤ 只改名称免测连（ADR-0039 §3）：口令留空 = 不改，改完 has_password 仍为真。
  # 用一条一次性的数据源做，免得把 ①-④ 依赖的那两条名字改花。
  rename_id=$(ensure_datasource "V1 改名用" "$(target_datasource_payload "V1 改名用" spike123)") || return 1
  api PUT "/api/datasources/$rename_id" \
    "$(jq -nc '{name:"V1 改名用（已改名）", kind:"mysql", host:"mysql", port:3306,
                username:"spike", password:"", database:"qbs"}')" || return 1
  assert_eq "C1⑤ rename without test-connection" 200 "$API_STATUS" || return 1
  assert_eq "C1⑤ new name stored" "V1 改名用（已改名）" "$(jq -r '.name' <<<"$API_BODY")" || return 1
  assert_eq "C1⑤ password kept" true "$(jq '.has_password' <<<"$API_BODY")" || return 1
  api DELETE "/api/datasources/$rename_id" || return 1
  assert_eq "C1⑤ unreferenced datasource deletes cleanly" 200 "$API_STATUS" || return 1
}

scenario_c2() {
  start_source || return 1
  local columns tables
  mysql_exec "DELETE FROM V1_C2" >/dev/null || return 1

  # ① 源列映到不同名目标列，跑通后**按目标名**核对目标端数据
  [[ -n "$C2_TASK_ID" ]] || C2_TASK_ID=$(create_task "C2 不同名映射" "$(c2_spec)") || return 1
  run_task_to_completion "$C2_TASK_ID" >/dev/null || return 1
  assert_eq "C2① outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C2① target effect" SWAPPED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "C2① target rows" 3 "$(mysql_exec "SELECT COUNT(*) FROM V1_C2")" || return 1
  assert_eq "C2① DEST_NAME by target name" "$(printf '%s\n' alpha bravo delta)" \
    "$(mysql_exec "SELECT DEST_NAME FROM V1_C2 ORDER BY ROW_ID")" || return 1
  assert_eq "C2① DEST_AMOUNT by target name" "$(printf '%s\n' 10.25 20.50 30.75)" \
    "$(mysql_exec "SELECT DEST_AMOUNT FROM V1_C2 ORDER BY ROW_ID")" || return 1

  # ② 默认预填同名。**预填本身在前端**（构建器里 target 的初值取源列名，ADR-0039 §5），
  # 命令行台架证不到那一步；能证的是它吃的那份输入齐备——取列面回的就是源列名，
  # 恒等映射由 C4/C5 的任务照常跑通。界面那一半归 X 走查，不在这里冒充证过。
  api POST /api/builder/columns \
    "$(jq -nc --arg ds "$ORACLE_DATASOURCE_ID" '{datasource_id:$ds, owner:"SPIKE", table:"T_V1_C2"}')" || return 1
  assert_eq "C2② builder columns status" 200 "$API_STATUS" || return 1
  assert_eq "C2② source column names available for prefill" true \
    "$(jq '[.[].name] | (index("SRC_NAME") != null) and (index("SRC_AMOUNT") != null)' <<<"$API_BODY")" || return 1
  echo "C2② 预填这一步在构建器里（ADR-0039 §5），台架只证输入齐备；渲染面归 X 走查"

  # ③ 两个目标端元数据入口各取一次，错误码闭集不增（ADR-0038 §3/§9）
  api POST /api/target/tables "$(jq -nc --arg ds "$TARGET_DATASOURCE_ID" '{datasource_id:$ds}')" || return 1
  assert_eq "C2③ /target/tables status" 200 "$API_STATUS" || return 1
  tables=$API_BODY
  assert_eq "C2③ tables list contains V1_C2" true "$(jq '.tables | index("V1_C2") != null' <<<"$tables")" || return 1
  api POST /api/target/columns \
    "$(jq -nc --arg ds "$TARGET_DATASOURCE_ID" '{datasource_id:$ds, target_table:"V1_C2"}')" || return 1
  assert_eq "C2③ /target/columns status" 200 "$API_STATUS" || return 1
  columns=$API_BODY
  assert_eq "C2③ column count" 4 "$(jq '.columns | length' <<<"$columns")" || return 1
  assert_eq "C2③ keys report the primary key" true \
    "$(jq '[.keys[] | select(.columns == ["ROW_ID"])] | length >= 1' <<<"$columns")" || return 1
  # 表不存在**回空清单、不是错误**——这就是「闭集不增」在这两个入口上的具体样子。
  api POST /api/target/columns \
    "$(jq -nc --arg ds "$TARGET_DATASOURCE_ID" '{datasource_id:$ds, target_table:"V1_NO_SUCH_TABLE"}')" || return 1
  assert_eq "C2③ missing table is not an error" 200 "$API_STATUS" || return 1
  assert_eq "C2③ missing table returns an empty list" 0 "$(jq '.columns | length' <<<"$API_BODY")" || return 1
  echo "C2③ target columns: $(jq -c '.columns' <<<"$columns")"
  echo "C2③ target keys: $(jq -c '.keys' <<<"$columns")"
}

c3_spec() {
  # 第一个参数是 GRP 那条条件的取值来源：常量档写死 'A'，运行时档留给 run_params。
  local value_source=$1 constant=${2:-}
  jq -nc --arg value_source "$value_source" --arg constant "$constant" \
    --argjson load_date "$(load_date_condition)" '{
    owner:"SPIKE", table:"T_V1_C3", target_table:"V1_C3",
    primary_key:["ROW_ID"],
    columns:[
      {source:"ROW_ID", target:"ROW_ID"},
      {source:"GRP", target:"GRP"},
      {source:"LOAD_DATE", target:"LOAD_DATE"}
    ],
    conditions:[
      $load_date,
      {column:"GRP", operator:"eq", value_type:"text",
       parameter:"grp", value_source:$value_source, constant:$constant}
    ]
  }'
}

scenario_c3() {
  start_source || return 1
  local constant_task runtime_task sql
  mysql_exec "DELETE FROM V1_C3" >/dev/null || return 1

  # ① 一个常量条件 + 一个运行时填的条件，各跑一次，行数按预期变
  constant_task=$(create_task "C3 常量条件（GRP=A）" "$(c3_spec constant A)") || return 1
  run_task_to_completion "$constant_task" >/dev/null || return 1
  assert_eq "C3① constant condition outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C3① rows after constant run" 3 "$(mysql_exec "SELECT COUNT(*) FROM V1_C3")" || return 1
  assert_eq "C3① all rows are group A" 3 "$(mysql_exec "SELECT COUNT(*) FROM V1_C3 WHERE GRP = 'A'")" || return 1

  runtime_task=$(create_task "C3 运行时条件（GRP 每次填）" "$(c3_spec runtime)") || return 1
  run_task_to_completion "$runtime_task" 400 \
    "$(jq -nc --arg date "$BIZ_DATE" '{load_date:$date, grp:"B"}')" >/dev/null || return 1
  assert_eq "C3① runtime condition outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  # upsert 不删别人的行（ADR-0035 §1），所以 B 组是**加**上去的：3 + 2 = 5。
  # 判「B 组恰好 2 行」而不是只判总数——总数变了也可能是 A 组被动了。
  assert_eq "C3① group B rows after runtime run" 2 "$(mysql_exec "SELECT COUNT(*) FROM V1_C3 WHERE GRP = 'B'")" || return 1
  assert_eq "C3① total rows after both runs" 5 "$(mysql_exec "SELECT COUNT(*) FROM V1_C3")" || return 1

  # ② 生成的 SQL 里值一律是绑定变量、**常量也是**（ADR-0036 §2 抬头：理由是转义正确性）
  api POST /api/builder/sql "$(c3_spec constant A)" || return 1
  assert_eq "C3② builder sql status" 200 "$API_STATUS" || return 1
  sql=$(jq -r '.source_sql' <<<"$API_BODY") || return 1
  assert_eq "C3② constant is bound, not inlined" true \
    "$([[ "$sql" == *":grp"* && "$sql" != *"'A'"* ]] && echo true || echo false)" || return 1
  echo "C3② generated SQL (constant condition):"
  printf '%s\n' "$sql"
  api POST /api/builder/sql "$(c3_spec runtime)" || return 1
  sql=$(jq -r '.source_sql' <<<"$API_BODY") || return 1
  assert_eq "C3② runtime value is bound too" true \
    "$([[ "$sql" == *":grp"* ]] && echo true || echo false)" || return 1
  echo "C3② generated SQL (runtime condition):"
  printf '%s\n' "$sql"

  # ③ 界面无手改 SQL 入口。**台架证的是协议面**：任务定义收不下裸 SQL——
  # `TaskSpec` 是 `deny_unknown_fields`，退役的 `source_sql` 字段递进去就是 400。
  # 界面上有没有那个输入框，归 X 走查看渲染结果，这里不冒充证过。
  api POST /api/builder/sql \
    "$(jq -nc --argjson spec "$(c3_spec constant A)" '$spec + {source_sql:"SELECT 1 FROM dual"}')" || return 1
  assert_eq "C3③ hand-written SQL is refused by the protocol" 400 "$API_STATUS" || return 1
  echo "C3③ refusal body: $API_BODY"
  echo "C3③ 界面上没有手改 SQL 的控件这一半归 X 走查，本报告不主张已观察"
}

c4_spec() {
  local target_table=$1
  jq -nc --arg target_table "$target_table" --argjson load_date "$(load_date_condition)" '{
    owner:"SPIKE", table:"T_V1_C4", target_table:$target_table,
    primary_key:["ROW_ID"],
    columns:[
      {source:"ROW_ID", target:"ROW_ID"},
      {source:"V_TEXT", target:"V_TEXT"},
      {source:"LOAD_DATE", target:"LOAD_DATE"}
    ],
    conditions:[$load_date]
  }'
}

scenario_c4() {
  start_source || return 1
  local task nopk_task staged affected purged
  mysql_exec "DELETE FROM V1_C4" >/dev/null || return 1
  # 第 ④ 条要改源值，改完必须还原——否则下一轮跑起来源端已经不是 fixture 说的那样了。
  oracle_exec "UPDATE t_v1_c4 SET v_text = 'first' WHERE row_id = 1;" >/dev/null || return 1

  task=$(create_task "C4 主键 upsert 幂等" "$(c4_spec V1_C4)") || return 1

  # ① 同一 run 连跑两次，目标表行数不变
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C4① first run outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C4① rows after first run" 5 "$(mysql_exec "SELECT COUNT(*) FROM V1_C4")" || return 1
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C4① second run outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C4① rows after second run" 5 "$(mysql_exec "SELECT COUNT(*) FROM V1_C4")" || return 1

  # ② staged ≤ affected ≤ 2×staged（ADR-0035 §1：ON DUPLICATE KEY UPDATE 更新一行记 2）
  staged=$(jq -r '.staged_rows' <<<"$API_BODY") || return 1
  affected=$(jq -r '.sink_reported_rows' <<<"$API_BODY") || return 1
  assert_eq "C4② staged rows" 5 "$staged" || return 1
  assert_le "C4② staged <= affected" "$staged" "$affected" || return 1
  assert_le "C4② affected <= 2x staged" "$affected" "$(( 2 * staged ))" || return 1

  # ③ purged_rows 恒 0（DELETE 已经删干净了）
  purged=$(jq -r '.purged_rows' <<<"$API_BODY") || return 1
  assert_eq "C4③ purged rows" 0 "$purged" || return 1

  # ④ 改一列源值重跑 → 目标端该列**被更新**。
  # 这条不许省：只验「行数不变」的话 INSERT IGNORE 也能过，而它会把改值静默吞掉
  # （ADR-0034 §1b 点名的静默改值）。行数不变是必要条件，不是充分条件。
  oracle_exec "UPDATE t_v1_c4 SET v_text = 'first-v2' WHERE row_id = 1;" >/dev/null || return 1
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C4④ third run outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C4④ changed source value landed" first-v2 \
    "$(mysql_exec "SELECT V_TEXT FROM V1_C4 WHERE ROW_ID = 1")" || return 1
  assert_eq "C4④ row count still unchanged" 5 "$(mysql_exec "SELECT COUNT(*) FROM V1_C4")" || return 1
  oracle_exec "UPDATE t_v1_c4 SET v_text = 'first' WHERE row_id = 1;" >/dev/null || return 1

  # ⑤ 目标表缺 PK/UNIQUE → 预检拒跑（ADR-0035 §2 的静默退化防线）
  nopk_task=$(create_task "C4 目标表无唯一约束" "$(c4_spec V1_C4_NOPK)") || return 1
  run_task_to_completion "$nopk_task" >/dev/null || return 1
  assert_eq "C4⑤ sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "C4⑤ failure kind" MAPPING_PRECHECK "$(jq -r '.failure_kind' <<<"$API_BODY")" || return 1
  assert_eq "C4⑤ rule names the missing unique constraint" true \
    "$(jq '[.mapping_issues[] | select(.rule | contains("PRIMARY KEY 或 UNIQUE"))] | length >= 1' <<<"$API_BODY")" || return 1
  assert_eq "C4⑤ target untouched" DISCARDED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  assert_eq "C4⑤ no staging table left" 0 "$(mysql_staging_count V1_C4_NOPK)" || return 1
  echo "C4⑤ mapping issues: $(jq -c '.mapping_issues' <<<"$API_BODY")"
}

c5_spec() {
  local target_table=$1
  jq -nc --arg target_table "$target_table" --argjson load_date "$(load_date_condition)" '{
    owner:"SPIKE", table:"T_V1_C5", target_table:$target_table,
    primary_key:["ROW_ID"],
    columns:[
      {source:"ROW_ID", target:"ROW_ID"},
      {source:"V_TEXT", target:"V_TEXT"},
      {source:"LOAD_DATE", target:"LOAD_DATE"}
    ],
    conditions:[$load_date]
  }'
}

# 映射预检三分支（ADR-0038 §5）。**这一档只在 C5 出现**：同一判据两个真源会各自漂移，
# M3 的 B2 因此特意没有再设一份（#134 的裁定）。别把它挪到别处，也别在别处再设一份。
scenario_c5() {
  start_source || return 1
  local task
  mysql_exec "DELETE FROM V1_C5_PASS" >/dev/null || return 1

  # ① 主键列在目标端可空 → 拒。
  # MySQL 的 PRIMARY KEY 强制 NOT NULL，所以这一档只能由 UNIQUE 造——
  # 而这正是它防的失效模式：UNIQUE 允许多个 NULL，upsert 会静默退化成纯 INSERT。
  task=$(create_task "C5① 主键列可空" "$(c5_spec V1_C5_NULLABLE_PK)") || return 1
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C5① sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "C5① rejected on the primary key column" true \
    "$(jq '[.mapping_issues[] | select(.column == "ROW_ID" and (.rule | contains("主键列必须 NOT NULL")))] | length == 1' <<<"$API_BODY")" || return 1
  assert_eq "C5① target untouched" DISCARDED "$(jq -r '.target_table_effect' <<<"$API_BODY")" || return 1
  echo "C5① mapping issues: $(jq -c '.mapping_issues' <<<"$API_BODY")"

  # ② 被映射的非主键列可空 → 放行。同一趟顺带证第 3 分支的**放行**半边：
  # CREATE_TIME 未被映射、NOT NULL，但有 DEFAULT CURRENT_TIMESTAMP，照样跑通。
  task=$(create_task "C5② 非主键列可空放行" "$(c5_spec V1_C5_PASS)") || return 1
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C5② outcome" SUCCEEDED "$(jq -r '.outcome' <<<"$API_BODY")" || return 1
  assert_eq "C5② mapping issues" 0 "$(jq '.mapping_issues | length' <<<"$API_BODY")" || return 1
  assert_eq "C5② target rows" 2 "$(mysql_exec "SELECT COUNT(*) FROM V1_C5_PASS")" || return 1
  assert_eq "C5② unmapped column with a default was filled by MySQL" 0 \
    "$(mysql_exec "SELECT COUNT(*) FROM V1_C5_PASS WHERE CREATE_TIME IS NULL")" || return 1

  # ③ 未映射列既无 COLUMN_DEFAULT 也无 EXTRA（非 auto_increment）→ 拒。
  # 不拒的话它会搬到一半撞 ERROR 1364，那时暂存表已经写了一半。
  task=$(create_task "C5③ 未映射的非空无默认列" "$(c5_spec V1_C5_REQUIRED)") || return 1
  run_task_to_completion "$task" >/dev/null || return 1
  assert_eq "C5③ sink code" PRECHECK_FAILED "$(jq -r '.sink_code' <<<"$API_BODY")" || return 1
  assert_eq "C5③ rejected on the unmapped required column" true \
    "$(jq '[.mapping_issues[] | select(.column == "MUST_FILL" and (.rule | contains("未被映射且不允许留空")))] | length == 1' <<<"$API_BODY")" || return 1
  # 报告形态不变（ADR-0009 §8 逐列摆四栏），这一档的源列一栏写「（未映射）」。
  assert_eq "C5③ source column reads 未映射" "（未映射）" \
    "$(jq -r '[.mapping_issues[] | select(.column == "MUST_FILL")][0].source' <<<"$API_BODY")" || return 1
  assert_eq "C5③ no staging table left" 0 "$(mysql_staging_count V1_C5_REQUIRED)" || return 1
  echo "C5③ mapping issues: $(jq -c '.mapping_issues' <<<"$API_BODY")"
}

# C6 的宽表规格：源表是 M1 的 `t_m1_wide`（ADR-0040 §3.3 字面「同一张宽表」），
# 目标端另起 V1_WIDE。两档靠 ROW_ID 的常量上界切：`< 1` 是 0 行的基线档，
# `< 10001` 是一万行，`< 100001` 是十万行——同一张表、同一条投影，只有行数不同，
# 这是斜率判据成立的前提（换表或换列宽，比的就不是同一件事了）。
wide_spec() {
  local row_limit=$1
  jq -nc --arg row_limit "$row_limit" '{
    owner:"SPIKE", table:"T_M1_WIDE", target_table:"V1_WIDE",
    primary_key:["ROW_ID"],
    columns:(
      [{source:"ROW_ID", target:"ROW_ID"}, {source:"D_BIZ", target:"D_BIZ"}]
      + [range(1; 69) | (if . < 10 then "C0\(.)" else "C\(.)" end) | {source:., target:.}]
    ),
    conditions:[
      {column:"D_BIZ", operator:"eq", value_type:"date",
       parameter:"d_biz", value_source:"runtime", constant:""},
      {column:"ROW_ID", operator:"lt", value_type:"number",
       parameter:"row_limit", value_source:"constant", constant:$row_limit}
    ]
  }'
}

# 一档的测量：重启 sink → 跑 0 行基线 → 读两个基线 → 跑本档 → 读两个峰值。
#
# **基线是每档各测一次的同进程读数**，不是一个全局常数：sink 每档都是新进程，
# 它的固定开销（连接、缓冲池）只有在同一个进程里读才可比；source 是一次性进程，
# 基线只能由一趟真的跑起来、连上库、但一行都没搬的 run 给出。
#
# 证据打在 stderr、数打在 stdout：调用处要的是六个数，报告要的是全过程。
c6_tier() {
  local label=$1 baseline_task=$2 tier_task=$3 attempts=$4
  local sink_pid src_base sink_base src_peak sink_peak started elapsed rows params
  params=$(jq -nc --arg date "$BIZ_DATE" '{d_biz:$date}') || return 1

  start_sink >&2 || return 1
  sink_pid=$(compose exec -T client cat "$SINK_PID_FILE" 2>/dev/null | tr -d '\r') || return 1
  echo "C6 $label: sink 已重启，pid=${sink_pid}（VmHWM 跨 run 只增不减，不重启比值恒为 1）" >&2

  run_task_to_completion "$baseline_task" 400 "$params" >/dev/null || return 1
  [[ "$(jq -r '.outcome' <<<"$API_BODY")" == SUCCEEDED ]] ||
    { echo "C6 $label baseline run failed: $API_BODY" >&2; return 1; }
  [[ "$(jq -r '.source_rows' <<<"$API_BODY")" == 0 ]] ||
    { echo "C6 $label baseline moved rows, it is not a baseline: $API_BODY" >&2; return 1; }
  src_base=$(source_maxrss_bytes) || return 1
  sink_base=$(sink_vmhwm_kb) || return 1
  sink_base=$(( sink_base * 1024 ))
  echo "C6 $label baseline: source ru_maxrss=$src_base B, sink VmHWM=$sink_base B" >&2

  started=$SECONDS
  run_task_to_completion "$tier_task" "$attempts" "$params" >/dev/null || return 1
  elapsed=$(( SECONDS - started ))
  [[ "$(jq -r '.outcome' <<<"$API_BODY")" == SUCCEEDED ]] ||
    { echo "C6 $label tier run failed: $API_BODY" >&2; return 1; }
  rows=$(jq -r '.source_rows' <<<"$API_BODY") || return 1
  src_peak=$(source_maxrss_bytes) || return 1
  sink_peak=$(sink_vmhwm_kb) || return 1
  sink_peak=$(( sink_peak * 1024 ))
  echo "C6 $label peak: source ru_maxrss=$src_peak B, sink VmHWM=$sink_peak B, rows=$rows, ${elapsed}s" >&2

  printf '%s %s %s %s %s %s %s\n' \
    "$src_base" "$src_peak" "$sink_base" "$sink_peak" "$sink_pid" "$elapsed" "$rows"
}

ratio() {
  awk -v a="$1" -v b="$2" 'BEGIN { if (b == 0) print "n/a"; else printf "%.2f", a / b }'
}

C6_SUMMARY=""

scenario_c6() {
  local baseline_task tier10k_task tier100k_task
  local s10 s100 delta_src_10k delta_src_100k delta_sink_10k delta_sink_100k
  local src_base_10k src_peak_10k sink_base_10k sink_peak_10k pid_10k elapsed_10k rows_10k
  local src_base_100k src_peak_100k sink_base_100k sink_peak_100k pid_100k elapsed_100k rows_100k

  # C6 全程换成 wrapper 起的编排进程：`run_executable` 与它的环境在启动时定死，
  # 中途换不了，所以这一场景整段用另一份配置跑。
  start_source "$MEMORY_WRAPPER" || return 1
  : > "$MEMORY_LOG"
  mysql_exec "DELETE FROM V1_WIDE" >/dev/null || return 1

  baseline_task=$(create_task "C6 基线（0 行）" "$(wide_spec 1)") || return 1
  tier10k_task=$(create_task "C6 一万行" "$(wide_spec 10001)") || return 1
  tier100k_task=$(create_task "C6 十万行" "$(wide_spec 100001)") || return 1

  # 一万行档在前、十万行档在后，中间重启 sink。顺序不能反：`VmHWM` 只增不减，
  # 先跑大档的话小档在同一进程里读到的会是大档的残留（这里每档都重启，顺序无关，
  # 但报告里的两个比值仍按「小档在前」读，别把两档的数对调）。
  s10=$(c6_tier 10k "$baseline_task" "$tier10k_task" 12000) || return 1
  read -r src_base_10k src_peak_10k sink_base_10k sink_peak_10k pid_10k elapsed_10k rows_10k <<<"$s10"
  s100=$(c6_tier 100k "$baseline_task" "$tier100k_task" 36000) || return 1
  read -r src_base_100k src_peak_100k sink_base_100k sink_peak_100k pid_100k elapsed_100k rows_100k <<<"$s100"

  assert_eq "C6 tier rows (10k)" 10000 "$rows_10k" || return 1
  assert_eq "C6 tier rows (100k)" 100000 "$rows_100k" || return 1
  assert_eq "C6 target rows after both tiers" 100000 "$(mysql_exec "SELECT COUNT(*) FROM V1_WIDE")" || return 1

  # 重启这一步确实执行了（ADR-0040 §3.5 要求报告记下来）：两档的 sink pid 必须不同。
  assert_eq "C6 sink was restarted between tiers" true \
    "$([[ -n "$pid_10k" && -n "$pid_100k" && "$pid_10k" != "$pid_100k" ]] && echo true || echo false)" || return 1

  delta_src_10k=$(( src_peak_10k - src_base_10k ))
  delta_src_100k=$(( src_peak_100k - src_base_100k ))
  delta_sink_10k=$(( sink_peak_10k - sink_base_10k ))
  delta_sink_100k=$(( sink_peak_100k - sink_base_100k ))

  # 分母为零的话判据恒真——那是测量坏了，不是产品好。一条永远为真的断言比没有断言更坏。
  assert_gt "C6 source 10k delta is measurable" "$delta_src_10k" 0 || return 1
  assert_gt "C6 sink 10k delta is measurable" "$delta_sink_10k" 0 || return 1

  # 判据本体（ADR-0040 §3.3）：peak(100k) - baseline ≤ 2 × (peak(10k) - baseline)，
  # source 与 sink **各判一次，两条都绿才算 PASS**（两端是两条独立风险，合并会互相掩盖）。
  assert_le "C6 source slope" "$delta_src_100k" "$(( 2 * delta_src_10k ))" || return 1
  assert_le "C6 sink slope" "$delta_sink_100k" "$(( 2 * delta_sink_10k ))" || return 1

  # 四个绝对数 + 两组基线原样进报告：将来调系数时要有历史可比（ADR-0040 §3.3）。
  C6_SUMMARY=$(cat <<EOF
| 进程 | 档 | 基线 (B) | 峰值 (B) | 增量 (B) | 量法 |
|---|---|---:|---:|---:|---|
| source | 10k | $src_base_10k | $src_peak_10k | $delta_src_10k | wait4 ru_maxrss |
| source | 100k | $src_base_100k | $src_peak_100k | $delta_src_100k | wait4 ru_maxrss |
| sink | 10k | $sink_base_10k | $sink_peak_10k | $delta_sink_10k | /proc/<pid>/status VmHWM |
| sink | 100k | $sink_base_100k | $sink_peak_100k | $delta_sink_100k | /proc/<pid>/status VmHWM |

- **source 斜率**：$delta_src_100k / $delta_src_10k = **$(ratio "$delta_src_100k" "$delta_src_10k")**（判据 ≤ 2.00）
- **sink 斜率**：$delta_sink_100k / $delta_sink_10k = **$(ratio "$delta_sink_100k" "$delta_sink_10k")**（判据 ≤ 2.00）
- **sink 重启**：10k 档 pid=${pid_10k}，100k 档 pid=${pid_100k}，两档之间确实重启过
- **耗时（只记不判，ADR-0040 §1/§3）**：10k 档 ${elapsed_10k}s，100k 档 ${elapsed_100k}s
EOF
)
  echo "$C6_SUMMARY"
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

result_of() {
  local index
  index=$(scenario_index "$1") || { printf 'UNKNOWN'; return 0; }
  printf '%s' "${RESULTS[$index]:-FAIL}"
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
    echo "# 第一版 rig 验收报告（C1-C6）"
    echo
    echo "- Generated (UTC): $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Git commit: ${DB_QBS_GIT_COMMIT:-$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo "unknown")}"
    echo "- 判据依据：ADR-0040 §4（C 系列）与 §3（内存形状）。判据不在本报告里重新推导。"
    echo "- 第一版验收是**四份**台架（ADR-0040 §5.4）：M1 / M2 / M3 / 本入口。本报告只主张 C1-C6。"
    echo
    # 客户五条需求的兑现对照（所有者 2026-08-19 裁定：这份报告也是对客户交代第一版做完了的凭据，
    # 六项检查是按技术模块分的，与那五条不是一一对应，对照关系必须报告自己说清楚）。
    echo "## 客户五条需求的兑现对照（ADR-0034 / STRATEGY-V1）"
    echo
    echo "| # | 客户需求 | 兑现在哪 | 本次结果 |"
    echo "|---|---|---|---|"
    echo "| 1 | 界面上管数据源（Oracle 源 + MySQL 目标，含测试连接） | C1 | $(result_of C1-datasource-crud) |"
    echo "| 2 | 字段映射与目标字段/主键选择 | C2（映射跑通）、C5（配错拦住）、C4⑤（无唯一约束拒跑） | C2=$(result_of C2-column-mapping) / C5=$(result_of C5-precheck-branches) / C4=$(result_of C4-upsert-idempotence) |"
    echo "| 3 | 用户可填筛选条件 | C3 | $(result_of C3-user-conditions) |"
    echo "| 4 | 主键 upsert，不再整段删重刷 | C4 | $(result_of C4-upsert-idempotence) |"
    echo "| 5 | 单次 10 万行 / 约 100MB，行数对得上，内存不随数据量线性增长 | **分两处**：行数与行宽在 M1 的 \`wide-100k\`（本入口不跑，见同批次 m1-acceptance 报告的载荷记账行）；内存形状在 C6 | C6=$(result_of C6-memory-shape)；M1 那一半本入口未跑 |"
    echo
    echo "> 另有一条「不做自动建表」（目标表由人预建）是已生效的禁令（ADR-0033 §4），不设检查项。"
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
    echo "## C6 内存高水位（四个绝对数 + 两组基线）"
    echo
    if [[ -n "$C6_SUMMARY" ]]; then
      echo "$C6_SUMMARY"
    else
      echo "未测出：C6 没有跑到读数那一步，具体停在哪见下面 C6 的 assertion evidence。"
    fi
    echo
    echo "- 量的是内核维护的单调高水位（source: \`wait4\` 的 \`ru_maxrss\`；sink: \`/proc/<pid>/status\` 的 \`VmHWM\`），**不是轮询采样**——采样会漏掉峰值，漏掉之后判据假绿（ADR-0040 §3.1）。"
    echo "- 单位：source 侧 \`ru_maxrss\` 在 macOS 是字节、在 Linux 是 kB，wrapper 已归一到字节；sink 侧 \`VmHWM\` 是 kB，报告里已乘 1024。"
    echo "- **不设绝对上限**（ADR-0040 §3.4，现场独立服务器内存充裕）；**耗时只记不判**（夜间批量）。"
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
    echo
    echo "## 台架能证到哪儿（本报告不冒充证过的两处）"
    echo
    echo "- **C1②「测通才让存」**：服务端的 \`POST /api/datasources\` 本身不测连，这道门槛在数据源对话框上（ADR-0039 §3）。本入口证的是它依赖的事实——错凭据的测连确实失败、失败之后库里没多出一条；**对话框那一半归 X 走查**。"
    echo "- **C2②「默认预填同名」与 C3③「界面无手改 SQL 入口」**：预填与控件都在前端（ADR-0039 §5）。本入口证的是协议面——取列面回的列名齐备、任务定义收不下裸 SQL（\`deny_unknown_fields\` 400）；**渲染那一半归 X 走查**。"
    echo
    echo "## 三份视觉走查在本入口的触发情况"
    echo
    echo "- **V1-V25 / W1-W6 / X1-X8：本入口未跑。** 本票（#135）只动 \`docs/spikes/fixtures/local-rig/\` 下的脚本与 fixture，不碰设计系统、\`.precheck-reports\` 布局、\`DiagnosticTable\` 列结构，也不碰数据源屏与构建器——\`CLAUDE.md\` 那张表的三条触发条件一条都没命中。X1-X8 的复跑挂在第一版整体验收票（#136）上。"
  } > "$REPORT"
  chmod 600 "$REPORT"
}

prepare_rig() {
  local command
  for command in docker jq curl cargo lsof python3 awk; do
    command -v "$command" >/dev/null || { echo "missing required command: $command" >&2; return 1; }
  done
  [[ -n "${M2_ORACLE_CLIENT_LIB_DIR:-}" && -d "$M2_ORACLE_CLIENT_LIB_DIR" ]] || {
    echo "M2_ORACLE_CLIENT_LIB_DIR must point to the host Oracle Instant Client directory" >&2
    return 1
  }
  [[ -x "$MEMORY_WRAPPER" ]] || { echo "memory wrapper is not executable: $MEMORY_WRAPPER" >&2; return 1; }
  ./scripts/up.sh || return 1
  docker run --rm --platform linux/arm64 \
    -v "$REPO_ROOT:/workspace" -w /workspace \
    -v qbs-cargo-registry:/usr/local/cargo/registry \
    rust:1-bookworm cargo build --release --workspace || return 1
  stop_all_sinks || return 1
  docker cp "$REPO_ROOT/target/release/db-qbs-sink" qbs-client:"$SINK_BIN" || return 1
  if [[ -n "$HOST_TARGET" ]]; then
    cargo build --release --target "$HOST_TARGET" -p db-qbs-source || return 1
  else
    cargo build --release -p db-qbs-source || return 1
  fi
  # C6 的源表是 M1 的 t_m1_wide（10 万行），所以 M1 的 fixture 必须在位。
  # **只在缺席或行数不对时才重灌**：两张 10 万行表重建要好几分钟，而四份台架串行跑时
  # 它通常已经由 M1 那一份建好了。判据是行数——表在但行数不对，比表不在更坏。
  local wide_rows
  wide_rows=$(printf 'SET HEADING OFF\nSET FEEDBACK OFF\nSET PAGESIZE 0\nSELECT COUNT(*) FROM t_m1_wide;\nEXIT\n' |
    compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE 2>/dev/null | tr -d ' \r\n')
  if [[ "$wide_rows" != 100000 ]]; then
    echo "==> t_m1_wide 行数是「${wide_rows}」不是 100000，重灌 M1 fixture"
    compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE \
      @/workspace/docs/spikes/fixtures/local-rig/acceptance/oracle.sql || return 1
  else
    echo "==> t_m1_wide 已在位（100000 行），跳过 M1 fixture 重灌"
  fi
  compose exec -T client sqlplus -S spike/spike123@//oracle:1521/XE \
    @/workspace/docs/spikes/fixtures/local-rig/acceptance/oracle-v1.sql || return 1
  compose exec -T mysql mysql -uspike -pspike123 qbs < "$ACCEPTANCE_ROOT/mysql-v1.sql" || return 1
  start_sink
}

cleanup() {
  [[ -n "$CLEAN" ]] || return 0
  stop_source >/dev/null 2>&1 || true
  stop_sink >/dev/null 2>&1 || true
  rm -rf "$WORK_ROOT"
}
trap cleanup EXIT

# 跑完默认把台架留着（所有者 2026-08-19 裁定）：X1-X8 走查过半条目要的正是 C1/C2
# 建出来的两条数据源与那个不同名映射的任务，清干净等于让人再手工造一遍。
hand_over_rig() {
  [[ -z "$CLEAN" ]] || return 0
  start_source || return 1
  cat <<EOF

==> 台架保留（要清场重跑时加 --clean）
    web UI              : $SOURCE_URL
    数据源              : $ORACLE_DATASOURCE_NAME / $TARGET_DATASOURCE_NAME
    被引用的任务        : ${KEEP_TASK_NAME}（在数据源屏上删它的目标库应当 409）
    不同名映射的任务    : C2 不同名映射（SRC_NAME → DEST_NAME）
    source data/history : $SOURCE_DATA/db-qbs.sqlite3
    source log          : $SOURCE_LOG
    tear down with      : kill $SOURCE_PID; docker compose -f $RIG_ROOT/docker-compose.yml exec -T client sh -c 'test ! -f $SINK_PID_FILE || { kill -TERM "\$(cat $SINK_PID_FILE)"; rm -f $SINK_PID_FILE; }'; rm -rf $WORK_ROOT
EOF
  disown "$SOURCE_PID" 2>/dev/null || true
}

echo "==> prepare v1 acceptance rig"
prepare_rig || { echo "rig preparation failed" >&2; exit 1; }

run_scenario C1-datasource-crud scenario_c1
run_scenario C2-column-mapping scenario_c2
run_scenario C3-user-conditions scenario_c3
run_scenario C4-upsert-idempotence scenario_c4
run_scenario C5-precheck-branches scenario_c5
run_scenario C6-memory-shape scenario_c6

hand_over_rig || { echo "failed to hand the rig over" >&2; exit 1; }
write_report || { echo "failed to write report" >&2; exit 1; }
echo "report: $REPORT"

failed=0
for (( index = 0; index < ${#SCENARIOS[@]}; index++ )); do
  [[ "${RESULTS[$index]:-FAIL}" == PASS ]] || failed=$((failed + 1))
done
if (( failed > 0 )); then
  echo "v1 acceptance: FAIL ($failed/${#SCENARIOS[@]} scenarios)"
  exit 1
fi
echo "v1 acceptance: PASS (${#SCENARIOS[@]}/${#SCENARIOS[@]} scenarios)"
