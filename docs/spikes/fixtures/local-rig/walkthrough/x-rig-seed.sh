#!/usr/bin/env bash
# X1–X8 对着**活台架**跑时的补数据：把数据源补到 5 条。
#
# X2 的判据是「录满 5 条（Oracle 与 MySQL 各若干）看列表」，而
# `run-v1-acceptance.sh` 只建 2 条（C1/C2 要用的那两条）。差的 3 条在这里补——
# 走的是与人手同一个入口 `POST /api/datasources`，不是往库里塞行。
#
# **补的这几条不要求连得通**：POST 本身不测连（ADR-0039 §3 把「测通才让存」放在对话框上），
# X2 只读列表的列与取值。要测连的是 X3，那条是真的。
set -uo pipefail
BASE=${BASE:-http://127.0.0.1:18088}

count() { curl -sf "$BASE/api/datasources" | jq 'length'; }

add() {
  local payload=$1 name
  name=$(jq -r '.name' <<<"$payload")
  if curl -sf "$BASE/api/datasources" | jq -e --arg n "$name" 'any(.[]; .name == $n)' >/dev/null; then
    echo "已存在，跳过：$name"
    return 0
  fi
  local status
  status=$(curl -s -o /tmp/x-rig-seed.out -w '%{http_code}' -X POST "$BASE/api/datasources" \
    -H 'content-type: application/json' -d "$payload")
  echo "POST $name -> $status $(cat /tmp/x-rig-seed.out)"
  [[ "$status" == 201 ]]
}

before=$(count) || { echo "取不到数据源列表：$BASE" >&2; exit 1; }
echo "补前 $before 条"

# MySQL 数据源**必须绑一台已注册的 agent**（ADR-0044 §1），所以补数据之前先取一台。
# 台架上那台是 `sink_base_url` 首启迁移出来的「默认」（§5），不必现注册。
AGENT_ID=$(curl -sf "$BASE/api/agents" | jq -r '.[0].agent_id // empty')
if [[ -z "$AGENT_ID" ]]; then
  echo "注册表里一台 agent 都没有——X 系列的 MySQL 数据源建不出来。" >&2
  echo "先起 sink，再 POST /api/agents（或让 source 首启迁移 sink_base_url）。" >&2
  exit 1
fi
echo "用 agent：$AGENT_ID"

add "$(jq -nc '{name:"财务库（走查）", kind:"oracle",
  connect_string:"//oracle-fa:1521/FAPDB", username:"fa_reader", password:"fa"}')"
add "$(jq -nc --arg a "$AGENT_ID" '{name:"集市 MySQL（走查）", kind:"mysql", agent_id:$a,
  host:"10.0.0.13", port:3307, username:"mart", password:"mart", database:"dw_mart"}')"
add "$(jq -nc --arg a "$AGENT_ID" '{name:"备用 MySQL（走查）", kind:"mysql", agent_id:$a,
  host:"10.0.0.14", port:3306, username:"spare", password:"", database:"dw_spare"}')"

after=$(count)
echo "补后 $after 条"
