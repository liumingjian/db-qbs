#!/usr/bin/env bash
# #154 —— 两端环境自检在演练台上的判据（P0–P11）。跑在 mac Docker 上（rexec 派发）。
#
# 要证的是票面那四条：
#   ① 干净源端容器上跑源端自检：缺项一次列全（先红），逐项可按输出处置（P1–P4）
#   ② 干净目标端容器上跑目标端自检：缺项一次列全（先红）（P5–P8）
#   ③ 检查项覆盖规格 #149 D.14 的两端清单，不缺项 —— **静态事实**，归
#      ./scripts/test-rehearsal-preflight.sh（那里按清单逐条对编号，不必起台架）
#   ④ 两个脚本进仓库 —— 同上，静态事实
#
# **「先红」不等于「全红」**：干净的 centos:7 上 glibc 就是 2.17（S1 本来就该绿），
# 演练台上两个库也确实够得着（S5 / D1 本来就该绿）。所以这里判的不是「一片红」，
# 而是**逐条与一张期望表对齐**——一张「全红」的表会把「脚本恒红」这种假绿放进来，
# 而恒红的自检和恒绿的门禁一样没用。
#
# 与之对偶的是 P9–P11：装上隧道之后**该转绿的转绿**（S6/S7、D8/D9），
# 证明先红不是脚本写死的。而 S8/D2 在这一阶段**仍然该红**——#153 的桩 sink 不是真 sink，
# 自检判的是「那头真是 sink（回 RUN_UNKNOWN）」而不是「有人应答」。真 sink 装上来是 #156 的事。
#
# `set -e` **刻意不开**：与四份既有判据脚本同一条纪律 —— 逐条判完再算总账。
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"

SRC=qbs-host-source
DST=qbs-host-target
PHASE=clean
case "${1:-}" in
  --phase) PHASE=${2:-clean} ;;
  "")      ;;
  *)       echo "用法：$0 [--phase clean|tunnel|both]" >&2; exit 2 ;;
esac
case "$PHASE" in clean|tunnel|both) ;; *) echo "--phase 只收 clean / tunnel / both" >&2; exit 2 ;; esac

pass=0; fail=0
report() {  # $1=编号 $2=期望 $3=实测 $4=说明
  if [[ "$3" == "$2" ]]; then
    printf '  %-5s PASS  %-52s 实测=%s\n' "$1" "$4" "$3"; pass=$((pass+1))
  else
    printf '  %-5s FAIL  %-52s 期望=%s 实测=%s\n' "$1" "$4" "$2" "$3"; fail=$((fail+1))
  fi
}
running() { docker inspect -f '{{.State.Status}}' "$1" 2>/dev/null | grep -qx running && echo running || echo 缺席; }

# 干净容器上每一条的期望判定。**改自检的判定就得改这里**，改不动说明那条判定变了含义。
CLEAN_SOURCE="S1=PASS S2=FAIL S3=FAIL S4=FAIL S5=PASS S6=FAIL S7=FAIL S8=FAIL"
CLEAN_TARGET="D1=PASS D2=FAIL D3=FAIL D4=FAIL D5=FAIL D6=FAIL D7=FAIL D8=FAIL D9=FAIL"
# 隧道装好之后该翻面的那几条（其余照旧——真 sink 与 Instant Client 要等 #155/#156）。
TUNNEL_SOURCE="S6=PASS S7=PASS S8=FAIL"
TUNNEL_TARGET="D8=PASS D9=PASS D2=FAIL"

# 自检脚本按 docker cp 搬进去 —— 两台主机刻意不挂仓库（README「装机演练台」），
# 现场也是这么带进去的。
deliver() {  # $1=容器 $2=脚本名
  docker cp "$ROOT/packaging/preflight/$2" "$1:/root/$2" >/dev/null \
    && docker exec "$1" chmod +x "/root/$2"
}

# 跑一次自检，把「原样输出」与退出码都留下来。**口令走 -e 传，不进 argv**。
run_source_preflight() {
  docker exec -e QBS_ORACLE_HOST=oracle -e QBS_ORACLE_PORT=1521 \
    "$SRC" bash /root/preflight-source.sh 2>&1
}
run_target_preflight() {
  docker exec -e QBS_MYSQL_HOST=mysql -e QBS_MYSQL_PORT=3306 \
    -e QBS_MYSQL_USER=spike -e QBS_MYSQL_PASSWORD=spike123 -e QBS_MYSQL_DATABASE=qbs \
    "$DST" bash /root/preflight-target.sh 2>&1
}

# 只认判定行：编号必须长得像 S3 / D7。自检末尾那句「上面每条 FAIL 各带一行处置」
# 第二列恰好也是 FAIL，按 `$2` 认会把它当成一条判定收进来（2026-08-20 实跑撞到过）。
verdicts() {  # stdin=自检输出 -> "S1=PASS S2=FAIL …"（按出现顺序）
  awk '$1 ~ /^[SD][0-9]+$/ && ($2=="PASS"||$2=="FAIL"){printf "%s=%s ", $1, $2}' | sed 's/ $//'
}
ids_of() { tr ' ' '\n' <<<"$1" | sed 's/=.*//' | tr '\n' ' ' | sed 's/ $//'; }

# 「一次列全」的判法：**逐条与期望表对齐**，不是数个数。
# 数个数放得过「八条全在但顺序/判定错了」，而顺序恰恰是处置顺序。
check_run() {  # $1=编号前缀 $2=期望表 $3=输出 $4=退出码 $5=名字
  local prefix=$1 expect=$2 output=$3 rc=$4 name=$5
  local actual; actual=$(verdicts <<<"$output")
  report "${prefix}a" "$(ids_of "$expect")" "$(ids_of "$actual")" "${name}：检查项一次列全（编号全集，按出现顺序）"
  report "${prefix}b" "$expect" "$actual" "${name}：逐条判定与期望表一致"
  report "${prefix}c" 1 "$rc" "${name}：有 FAIL 时退出码为 1"
  # 逐项可按输出处置：每条 FAIL 后面必须紧跟一行「处置」。
  local fails hints
  fails=$(grep -cE '^  [SD][0-9]+ +FAIL ' <<<"$output")
  hints=$(grep -c '└ 处置：' <<<"$output")
  report "${prefix}d" "$fails" "$hints" "${name}：每条 FAIL 都带一行处置（FAIL=${fails}）"
}

echo "==> 前置：两台主机在跑（在此之前一切判据都不成立）"
report P0a running "$(running "$SRC")" "源端主机 $SRC"
report P0b running "$(running "$DST")" "目标端主机 $DST"
if (( fail )); then
  echo; echo "!! 两台主机没起，先跑 ./scripts/up.sh 与 ./scripts/rehearsal-up.sh"; exit 1
fi

if [[ "$PHASE" == both ]]; then
  echo "==> --phase both：先把两台主机推回干净态（这一步是破坏性的，装过的东西全没）"
  ./scripts/rehearsal-reset.sh >/dev/null || { echo "!! 重置失败"; exit 1; }
fi

if [[ "$PHASE" == clean || "$PHASE" == both ]]; then
  echo "==> P1–P4 干净源端容器：缺项一次列全（先红）"
  deliver "$SRC" preflight-source.sh
  out=$(run_source_preflight); rc=$?
  sed 's/^/    │ /' <<<"$out"
  check_run P1 "$CLEAN_SOURCE" "$out" "$rc" 源端自检

  echo "==> P5–P8 干净目标端容器：缺项一次列全（先红）"
  deliver "$DST" preflight-target.sh
  out=$(run_target_preflight); rc=$?
  sed 's/^/    │ /' <<<"$out"
  check_run P5 "$CLEAN_TARGET" "$out" "$rc" 目标端自检
fi

if [[ "$PHASE" == both ]]; then
  echo "==> 装隧道（#153 的 rehearsal-tunnel-up.sh，一个字不改地复用）"
  ./scripts/rehearsal-tunnel-up.sh >/dev/null || { echo "!! 隧道没装起来"; exit 1; }
fi

if [[ "$PHASE" == tunnel || "$PHASE" == both ]]; then
  echo "==> P9–P11 装上隧道之后：该转绿的转绿，桩 sink 仍不算 sink"
  deliver "$SRC" preflight-source.sh
  deliver "$DST" preflight-target.sh
  src_out=$(run_source_preflight)
  dst_out=$(run_target_preflight)
  pick() { # $1=输出 $2="S6 S7 S8" -> "S6=PASS S7=PASS …"
    local id out=""
    for id in $2; do
      out+="$id=$(awk -v k="$id" '$1==k{print $2; exit}' <<<"$1") "
    done
    sed 's/ $//' <<<"$out"
  }
  report P9  "$TUNNEL_SOURCE" "$(pick "$src_out" "S6 S7 S8")" \
    "源端：stunnel 客户端与隧道入口转绿（先红不是写死的）"
  report P10 "$TUNNEL_TARGET" "$(pick "$dst_out" "D8 D9 D2")" \
    "目标端：stunnel 服务端与白名单口转绿"
  # S8/D2 仍红的**理由**也要对：判的是「那头真是 sink」，不是「有人应答」。
  # 桩 sink 应答了、但不是 sink —— 这正是那条判定不退化成端口探活的证据。
  report P11 应答不是sink \
    "$(grep -q '应答不是 sink' <<<"$src_out" && grep -q '应答的不是 sink' <<<"$dst_out" \
       && echo 应答不是sink || echo 理由对不上)" \
    "S8/D2 仍红的理由是「应答的不是 sink」（桩 sink 不是真 sink）"
fi

echo
echo "==== 两端自检判据：PASS=$pass FAIL=$fail ===="
(( fail == 0 ))
