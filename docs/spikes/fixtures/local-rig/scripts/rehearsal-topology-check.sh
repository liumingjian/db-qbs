#!/usr/bin/env bash
# #152 —— 演练台两台「主机」容器的拓扑判据。跑在 mac Docker 上（rexec 派发）。
#
# 演练台扮演的是 ADR-0041 §8 的事实前提：两台 CentOS 7 主机，源端挨着 Oracle、
# 目标端挨着 MySQL，**两库之间网络不通**，只有 source → sink 一跳走公网、且只开白名单端口。
# 这里把那张拓扑逐条断言出来 —— 通的要通，**不通的更要不通**：
# 「源端摸不到 MySQL」「除白名单端口外摸不到目标端」这两条一旦悄悄成立不了，
# 演练就会在一张比客户现场宽松的网上跑完，手册里缺的那几步现场才炸。
#
# **每条负判据都配一条正对照**：负判据最容易假绿 —— 没人监听、容器没起、DNS 查不到，
# 都会得出「不通」。所以先在目标端把监听端起起来、并从目标端本机自连确认它真的活着
# （R7a/R8a），再去判外面摸不摸得到（R6/R7/R8）。没有正对照的「不通」不算证据。
#
# 判据（R0–R10）与实测原样打印，最后一行是总账。任何一条 FAIL → 退出码 1。
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=qbs-host-source
DST=qbs-host-target
WHITELIST_PORT=15443     # 目标端唯一对外暴露的端口，扮演客户侧白名单
BLOCKED_PORT=15444       # 目标端上同样在听、但没暴露 —— 白名单之外的对照
MARKER=QBS-REHEARSAL     # 探针进程的标记，收尾按它回收
SKIP_RESET=0
[[ "${1:-}" == "--no-reset" ]] && SKIP_RESET=1

pass=0; fail=0
report() {  # $1=编号 $2=期望 $3=实测 $4=说明
  if [[ "$3" == "$2" ]]; then
    printf '  %-4s PASS  %-56s 实测=%s\n' "$1" "$4" "$3"; pass=$((pass+1))
  else
    printf '  %-4s FAIL  %-56s 期望=%s 实测=%s\n' "$1" "$4" "$2" "$3"; fail=$((fail+1))
  fi
}

# 所有探针都必须「失败也要给得出一个值」——取不到值时返回「取不到」，
# 既不会让 set -e 把脚本掐断（那看起来像跑挂了，不像判据没过），也不会被当成「不通」蒙混过关。
probe() {  # $@=docker exec 的参数 -> 单行输出，取不到给「取不到」
  local out
  out=$(docker "$@" 2>/dev/null | tr -d '\r' | tail -1) || out=""
  [[ -n "$out" ]] && printf '%s' "$out" || printf '取不到'
}

running() {  # $1=容器名 -> running / 缺席
  local out
  out=$(docker inspect -f '{{.State.Status}}' "$1" 2>/dev/null) || out=""
  [[ "$out" == running ]] && echo running || echo 缺席
}

tcp() {  # $1=容器 $2=主机 $3=端口 -> 通 / 不通 —— 容器里没 nc，也不该为探针装东西（干净机器）
  probe exec "$1" bash -c "timeout 5 bash -c 'exec 3<>/dev/tcp/$2/$3' 2>/dev/null && echo 通 || echo 不通"
}

read_token() {  # $1=容器 $2=主机 $3=端口 -> 监听端写回的令牌，读不到给「无」
  # 三处都不能省：先把 fd 3 关掉（`docker exec` 会传进来一个已打开的 fd 3，实测是个 ELF），
  # 连不上时**显式**给「无」并退出（`exec 3<>` 失败并不会让 bash 退出，后面的 `read` 就会
  # 去读那个继承来的 fd，把「摸不到」读成一串二进制垃圾——负判据就是这么假绿的），
  # 读到空行同样算「无」。
  probe exec "$1" bash -c \
    "timeout 5 bash -c 'exec 3>&- 2>/dev/null; exec 3<>/dev/tcp/$2/$3 2>/dev/null || { echo 无; exit 0; }; read -r line <&3; echo \${line:-无}' 2>/dev/null || echo 无"
}

marker_state() {  # $1=容器 -> 有 / 无
  probe exec "$1" bash -c 'test -f /root/.rehearsal-dirty && echo 有 || echo 无'
}

listen_token() {  # $1=容器 $2=端口 $3=令牌 —— 在容器里起一个只吐一行的监听端
  docker exec -d "$1" python -c "
import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', $2)); s.listen(5)
while True:
    c, _ = s.accept(); c.sendall('$3\n'); c.close()
" >/dev/null 2>&1 || true
}

kill_probes() {  # 回收探针监听端 —— 不回收就会占着 15443，#153 的 stunnel 服务端起不来，
                 # 「跑完仍是干净机器」这句话也就成了假的。centos:7 的 pkill 不保证在，走 /proc。
  # 标记走环境变量传进去，别写进脚本正文 —— 否则这段脚本自己的 cmdline 就含标记，一跑就自杀。
  docker exec -e M="$MARKER" "$DST" bash -c '
    self=$$
    for p in /proc/[0-9]*; do
      pid=${p##*/}
      [ "$pid" = "$self" ] && continue
      grep -qa "$M" "$p/cmdline" 2>/dev/null && kill "$pid" 2>/dev/null
    done; true' >/dev/null 2>&1 || true
}
trap kill_probes EXIT

echo "==> 前置：这套判据的「公网一跳」靠 Docker Desktop 的 host.docker.internal 打到宿主回环"
host_os=$(docker info --format '{{.OperatingSystem}}' 2>/dev/null || echo 未知)
echo "    Docker OperatingSystem = $host_os"
case "$host_os" in
  *Desktop*) ;;
  *) echo "    !! 不是 Docker Desktop：Linux 上 host-gateway 指的是宿主网卡，而白名单口只绑在"
     echo "       宿主回环，R7 会恒 FAIL。演练台的落点是 mac Docker（ADR-0041 增补 1），"
     echo "       换机器跑请先把 docker-compose.yml 里那条 ports 的绑定地址改掉。" ;;
esac

echo "==> R1 两台主机容器在跑（在此之前一切判据都不成立）"
report R1a running "$(running "$SRC")" "源端主机 $SRC"
report R1b running "$(running "$DST")" "目标端主机 $DST"

echo "==> R0 两台主机跟客户机同架构、同 glibc 下界（#151 的构建目标就落在这上面）"
report R0a x86_64 "$(probe exec "$SRC" uname -m)" "源端主机架构"
report R0b x86_64 "$(probe exec "$DST" uname -m)" "目标端主机架构"
report R0c 2.17 "$(probe exec "$SRC" bash -c "ldd --version | head -1 | awk '{print \$NF}'")" \
  "源端主机 glibc（客户机的硬下界）"
report R0d 2.17 "$(probe exec "$DST" bash -c "ldd --version | head -1 | awk '{print \$NF}'")" \
  "目标端主机 glibc"

echo "==> R9 干净态：留痕迹 → 先确认痕迹真的留下了 → 一键推倒重建 → 痕迹应当消失"
if (( SKIP_RESET )); then
  echo "    （--no-reset：跳过重建，R9 不判）"
else
  docker exec "$SRC" touch /root/.rehearsal-dirty >/dev/null 2>&1 || true
  docker exec "$DST" touch /root/.rehearsal-dirty >/dev/null 2>&1 || true
  # 正对照：痕迹没留下的话，「重建后痕迹没了」什么也证明不了。
  report R9a 有 "$(marker_state "$SRC")" "重建前源端主机上的痕迹文件"
  report R9b 有 "$(marker_state "$DST")" "重建前目标端主机上的痕迹文件"
  echo "    推倒重建中（rehearsal-reset.sh）……"
  ./scripts/rehearsal-reset.sh >/dev/null
  report R9c 无 "$(marker_state "$SRC")" "重建后源端主机上的痕迹文件"
  report R9d 无 "$(marker_state "$DST")" "重建后目标端主机上的痕迹文件"
fi

echo "==> R2–R5 各自够得着自己那一侧的库，够不着对面那一侧"
report R2 通   "$(tcp "$SRC" oracle 1521)" "源端主机 → Oracle 1521"
report R3 不通 "$(tcp "$SRC" mysql  3306)" "源端主机 → MySQL 3306（两库不通，必须摸不到）"
report R4 通   "$(tcp "$DST" mysql  3306)" "目标端主机 → MySQL 3306"
report R5 不通 "$(tcp "$DST" oracle 1521)" "目标端主机 → Oracle 1521（两库不通，必须摸不到）"

echo "==> R7a/R8a 正对照：目标端两个监听端确实活着（负判据全靠它们才有意义）"
listen_token "$DST" "$WHITELIST_PORT" "$MARKER-WHITELIST"
listen_token "$DST" "$BLOCKED_PORT"   "$MARKER-BLOCKED"
# Rosetta 下 python 冷启动慢，轮询到能自连为止，别用定长 sleep 赌。
for _ in $(seq 1 15); do
  [[ "$(read_token "$DST" 127.0.0.1 "$WHITELIST_PORT")" == "$MARKER-WHITELIST" ]] && break
  sleep 1
done
report R7a "$MARKER-WHITELIST" "$(read_token "$DST" 127.0.0.1 "$WHITELIST_PORT")" \
  "目标端主机自连 ${WHITELIST_PORT}（监听端活着）"
report R8a "$MARKER-BLOCKED" "$(read_token "$DST" 127.0.0.1 "$BLOCKED_PORT")" \
  "目标端主机自连 ${BLOCKED_PORT}（监听端活着）"

echo "==> R6 跨容器直达被切断：监听端就在那儿听着，源端直连仍必须摸不到"
report R6 无 "$(read_token "$SRC" "$DST" "$WHITELIST_PORT")" \
  "源端主机 → ${DST}:${WHITELIST_PORT}（直达，拿不到令牌）"

echo "==> R7–R8 公网那一跳只能走暴露端口（白名单），别的端口摸不到"
report R7 "$MARKER-WHITELIST" "$(read_token "$SRC" host.docker.internal "$WHITELIST_PORT")" \
  "源端主机 → 宿主:${WHITELIST_PORT} → 目标端（白名单口）"
report R8 不通 "$(tcp "$SRC" host.docker.internal "$BLOCKED_PORT")" \
  "源端主机 → 宿主:${BLOCKED_PORT}（目标端在听但没暴露）"

echo "==> R10 收尾：探针监听端回收干净，${WHITELIST_PORT} 留给 #153 的 stunnel 服务端"
kill_probes
sleep 1
report R10 不通 "$(tcp "$DST" 127.0.0.1 "$WHITELIST_PORT")" "目标端主机自连 ${WHITELIST_PORT}（应已无人听）"

echo
echo "==== 拓扑判据：PASS=$pass FAIL=$fail ===="
(( fail == 0 ))
