#!/usr/bin/env bash
# #154 —— 目标端自检「按 sink 的回答分档」那一段的判据（C1–C9）。**不需要台架、不碰 docker**，
# 只要 bash + python3，几秒钟跑完。
#
# 为什么单独有这一支：D4–D7 判的是开连接仪式的三项前提，而那三项是**问 sink 要的**，
# 按它的报错措辞分档。演练台上真 sink 要等 #156 才装得上来 —— 在那之前，
# 「连不上」以外的每一档在演练台判据里一次都没被走过（rehearsal-preflight-check.sh 的
# 干净容器阶段全都停在「sink 没应答」）。没走过的分支就是没有的分支。
#
# 仪式是**有先后的**：连上 → SET NAMES → SET sql_mode → 回读三项。**只有回读跑完那一档
# 才产生得了 PASS**：三条判词说的都是「回读回来是这个值」，仪式在回读之前停下时，
# 卡住的那一步记 FAIL，其余一律记「未判定」——「设过了」不等于「就是这个值」。
# **把未判定记成 PASS 是本票最危险的假绿**——
# 自检替环境作了一个它没验过的保证，而票面判据正是「自检说 OK 之后不该再出现环境类失败」。
# C3–C5 判的就是这个，C9 判的是同一件事的另一面：**认不出的回答也不许算合格**。
#
# 措辞由 preflight-sink-stub.py 从产品那两个文件抄来；产品改词的话
# test-rehearsal-preflight.sh 第 6 条先红。
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"
STUB="./scripts/preflight-sink-stub.py"
PREFLIGHT="$ROOT/packaging/preflight/preflight-target.sh"
PORT=${QBS_STUB_PORT:-18154}

pass=0; fail=0
report() {  # $1=编号 $2=期望 $3=实测 $4=说明
  if [[ "$3" == "$2" ]]; then
    printf '  %-4s PASS  %-40s 实测=%s\n' "$1" "$4" "$3"; pass=$((pass+1))
  else
    printf '  %-4s FAIL  %-40s 期望=%s 实测=%s\n' "$1" "$4" "$2" "$3"; fail=$((fail+1))
  fi
}

stub_pid=""
cleanup() { [[ -n "$stub_pid" ]] && kill "$stub_pid" 2>/dev/null; }
trap cleanup EXIT

run_case() {  # $1=桩的档 -> "D2=… D4=… D5=… D6=… D7=…"
  QBS_STUB_CASE=$1 python3 "$STUB" "$PORT" & stub_pid=$!
  local i
  for i in $(seq 1 30); do
    QP=$PORT timeout 1 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$QP' 2>/dev/null && break
    sleep 0.2
  done
  local out
  # 目标库那一跳（D1）在这里注定红——本支判的不是它。口令随手给一个，桩不看内容只看字段齐不齐。
  out=$(QBS_SINK_LISTEN=127.0.0.1:$PORT QBS_MYSQL_HOST=127.0.0.1 QBS_MYSQL_PORT=1 \
        QBS_MYSQL_USER=qbs QBS_MYSQL_PASSWORD='p"a\ss' QBS_MYSQL_DATABASE=dw_stage \
        QBS_HOST_IP=127.0.0.1 QBS_STUNNEL_CONF=/nonexistent QBS_STUNNEL_PIDFILE=/nonexistent \
        bash "$PREFLIGHT" 2>&1)
  kill "$stub_pid" 2>/dev/null; wait "$stub_pid" 2>/dev/null; stub_pid=""
  local id res=""
  for id in D2 D4 D5 D6 D7; do
    res+="$id=$(awk -v k="$id" '$1==k{print $2; exit}' <<<"$out") "
  done
  sed 's/ $//' <<<"$res"
}

# 口令里刻意带引号与反斜杠：JSON 转义漏了的话桩解不出请求体，D4 会以 400 收场。
# Content-Length 按字节算错也一样——库名与口令一旦有非 ASCII，字符数与字节数就不是一回事。
echo "==> C1–C9 目标端自检按 sink 的回答分档"
declare -a CASES=(
  "C1|ok|D2=PASS D4=PASS D5=PASS D6=PASS D7=PASS|全通"
  "C2|connect|D2=PASS D4=FAIL D5=FAIL D6=FAIL D7=FAIL|连不上：三项前提一律未判定"
  "C3|charset|D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL|卡在 SET NAMES：后两项未判定"
  "C4|sqlmode|D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL|卡在 sql_mode：另两项未判定"
  "C5|readback|D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL|回读失败：三项一律未判定"
  "C6|settings-all|D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL|三项都不合格，一次列全"
  "C7|settings-packet|D2=PASS D4=PASS D5=PASS D6=PASS D7=FAIL|只有 packet 不合格"
  "C8|settings-sqlmode|D2=PASS D4=PASS D5=PASS D6=FAIL D7=PASS|只有 sql_mode 不合格"
  "C9|bad-request|D2=PASS D4=FAIL D5=FAIL D6=FAIL D7=FAIL|认不出的回答：不许当成合格"
)
for row in "${CASES[@]}"; do
  IFS='|' read -r id case expect note <<<"$row"
  report "$id" "$expect" "$(run_case "$case")" "$note"
done

echo
echo "==== 分档判据：PASS=$pass FAIL=$fail ===="
(( fail == 0 ))
