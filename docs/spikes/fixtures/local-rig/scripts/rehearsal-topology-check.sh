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
# R6 尤其：它要证的是**路由被切断**，而按容器名连时失败首先发生在名字解析——
# 「DNS 查不到」正是上面点名的成因之一。所以 R6 按目标端在 `qbs-dst-side` 上的 **IP** 直连，
# 并配 R6a（目标端自己经同一个 IP 连得到令牌）把「IP 取错」这个成因也排掉。
#
# 「不通」还得覆盖**绕过路由的那条路**：`host.docker.internal`（Docker Desktop 给的 IPv6 网关）
# 后面挂着宿主上 `1521:1521` / `3306:3306` 两个发布端口，源端经它连 MySQL 曾经是通的。
# R3c/R5c 专判这条，正对照是同一个网关上的白名单口必须仍然通（R7 / R5d）——
# 只判「不通」而网关整条路已经断了的话，R3c 会为了错误的理由变绿（ADR-0041 增补 5）。
#
# 判据（R0–R10）与实测原样打印，末尾两行是总账：**R0 是 #151 的构建目标**（同架构、同 glibc），
# 不是 #152 的拓扑判据，单独记一笔账，别混进拓扑那一笔里。任何一条 FAIL → 退出码 1。
#
# `set -e` **刻意不开**：与既有四份判据脚本（run-m1/m2/m3/v1-acceptance.sh）同一条纪律——
# 逐条判完再算总账。半路掐断的话，后面的判据一条都不打印，退出码也不再是这里自述的那个 1。
set -uo pipefail
cd "$(dirname "$0")/.."

SRC=qbs-host-source
DST=qbs-host-target
WHITELIST_PORT=15443     # 目标端唯一对外暴露的端口，扮演客户侧白名单
BLOCKED_PORT=15444       # 目标端上同样在听、但没暴露 —— 白名单之外的对照
MARKER=QBS-REHEARSAL     # 探针进程的标记，收尾按它回收
# 默认**不**推倒重建：演练进行到一半来复核拓扑是常态，不能顺手把已装好的 source / stunnel
# 抹掉。R9（干净态）要判就显式给 `--reset`，破坏性动作由开关触发、不由默认值触发。
# `--no-reset` 作为旧写法继续收，语义与默认值相同。
DO_RESET=0
case "${1:-}" in
  --reset)    DO_RESET=1 ;;
  --no-reset|"") ;;
  *) echo "用法：$0 [--reset]   （--reset 才判 R9 干净态，会推倒重建两台主机）"; exit 2 ;;
esac

pass=0; fail=0; pre_pass=0; pre_fail=0
report() {  # $1=编号 $2=期望 $3=实测 $4=说明 —— R0* 记前置那笔账，其余记拓扑那笔
  if [[ "$3" == "$2" ]]; then
    printf '  %-4s PASS  %-56s 实测=%s\n' "$1" "$4" "$3"
    [[ "$1" == R0* ]] && pre_pass=$((pre_pass+1)) || pass=$((pass+1))
  else
    printf '  %-4s FAIL  %-56s 期望=%s 实测=%s\n' "$1" "$4" "$2" "$3"
    [[ "$1" == R0* ]] && pre_fail=$((pre_fail+1)) || fail=$((fail+1))
  fi
}

# 所有探针都必须「失败也要给得出一个值」——取不到值时返回「取不到」，
# 既不会让某一条判据的失败看起来像脚本跑挂了，也不会被当成「不通」蒙混过关。
docker_line() {  # $@=docker 子命令的参数 -> 末行输出，取不到给「取不到」
  # 兜底只有一处：管道末端是 `tail`，它几乎恒零退出，`|| out=""` 挡不住任何东西——
  # 真正把「命令挂了」翻译成一个值的是下面那条空串判断。
  local out
  out=$(docker "$@" 2>/dev/null | tr -d '\r' | tail -1)
  [[ -n "$out" ]] && printf '%s' "$out" || printf '取不到'
}

running() {  # $1=容器名 -> running / 缺席
  local out
  out=$(docker inspect -f '{{.State.Status}}' "$1" 2>/dev/null) || out=""
  [[ "$out" == running ]] && echo running || echo 缺席
}

# 下面两个探针把 $2/$3 直接拼进 `bash -c` 的字符串里：**实参只许是字面量的主机名/IP 与端口**
# （本脚本传的全是 docker inspect 取来的地址与写死的端口号）。要接外部输入得先改成 `-e` 传参。
tcp() {  # $1=容器 $2=主机 $3=端口 -> 通 / 不通 —— 容器里没 nc，也不该为探针装东西（干净机器）
  docker_line exec "$1" bash -c "timeout 5 bash -c 'exec 3<>/dev/tcp/$2/$3' 2>/dev/null && echo 通 || echo 不通"
}

read_token() {  # $1=容器 $2=主机 $3=端口 -> 监听端写回的令牌，读不到给「无」
  # 三处都不能省：先把 fd 3 关掉（`docker exec` 会传进来一个已打开的 fd 3，实测是个 ELF），
  # 连不上时**显式**给「无」并退出（`exec 3<>` 失败并不会让 bash 退出，后面的 `read` 就会
  # 去读那个继承来的 fd，把「摸不到」读成一串二进制垃圾——负判据就是这么假绿的），
  # 读到空行同样算「无」。
  docker_line exec "$1" bash -c \
    "timeout 5 bash -c 'exec 3>&- 2>/dev/null; exec 3<>/dev/tcp/$2/$3 2>/dev/null || { echo 无; exit 0; }; read -r line <&3; echo \${line:-无}' 2>/dev/null || echo 无"
}

ip_on() {  # $1=容器 $2=网络名 -> 该容器在那张网上的 IP，取不到给「取不到」
  docker_line inspect -f "{{index .NetworkSettings.Networks \"$2\" \"IPAddress\"}}" "$1"
}

marker_state() {  # $1=容器 -> 有 / 无
  docker_line exec "$1" bash -c 'test -f /root/.rehearsal-dirty && echo 有 || echo 无'
}

listen_token() {  # $1=容器 $2=端口 $3=令牌 —— 在容器里起一个只吐一行的监听端
  # 依赖 centos:7 自带的 python2。起不来时**打一行出来**：正对照 R7a/R8a 会跟着红，
  # 但红在「监听端没起」还是红在「网络不通」，不打这一行就得靠猜。
  docker exec -d "$1" python -c "
import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', $2)); s.listen(5)
while True:
    c, _ = s.accept(); c.sendall('$3\n'); c.close()
" >/dev/null 2>&1 || echo "    !! 监听端起不来（$1:$2）——容器里没有 python？下面的正对照会跟着红"
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
report R0a x86_64 "$(docker_line exec "$SRC" uname -m)" "源端主机架构"
report R0b x86_64 "$(docker_line exec "$DST" uname -m)" "目标端主机架构"
report R0c 2.17 "$(docker_line exec "$SRC" bash -c "ldd --version | head -1 | awk '{print \$NF}'")" \
  "源端主机 glibc（客户机的硬下界）"
report R0d 2.17 "$(docker_line exec "$DST" bash -c "ldd --version | head -1 | awk '{print \$NF}'")" \
  "目标端主机 glibc"

echo "==> R9 干净态：留痕迹 → 先确认痕迹真的留下了 → 一键推倒重建 → 痕迹应当消失"
if (( ! DO_RESET )); then
  echo "    （默认不重建：R9 不判。要判就跑 $0 --reset —— 它会抹掉两台主机上已装的东西）"
else
  docker exec "$SRC" touch /root/.rehearsal-dirty >/dev/null 2>&1 || true
  docker exec "$DST" touch /root/.rehearsal-dirty >/dev/null 2>&1 || true
  # 正对照：痕迹没留下的话，「重建后痕迹没了」什么也证明不了。
  report R9a 有 "$(marker_state "$SRC")" "重建前源端主机上的痕迹文件"
  report R9b 有 "$(marker_state "$DST")" "重建前目标端主机上的痕迹文件"
  echo "    推倒重建中（rehearsal-reset.sh）……"
  if ./scripts/rehearsal-reset.sh >/dev/null 2>&1; then reset_rc=成功; else reset_rc=失败; fi
  # 重建本身是 R9 的手段，手段没成的话 R9c/R9d 的「痕迹没了」什么也证明不了（容器压根不在）。
  report R9r 成功 "$reset_rc" "rehearsal-reset.sh 跑通（R9c/R9d 的前提）"
  report R9c 无 "$(marker_state "$SRC")" "重建后源端主机上的痕迹文件"
  report R9d 无 "$(marker_state "$DST")" "重建后目标端主机上的痕迹文件"
fi

echo "==> R2–R5 各自够得着自己那一侧的库，够不着对面那一侧"
# 一律按 **IP** 判，不按容器名：按名字连时失败首先发生在名字解析——`oracle` 这个名字在
# dst-side 上本来就解析不到，那样的「不通」证明不了任何路由上的事（ADR-0041 增补 4）。
# 每条负判据配的正对照就是它的镜像：同一个 IP、同一个端口，从**该去的**那一侧连必须通。
DEFAULT_NET=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{"\n"}}{{end}}' qbs-oracle11 2>/dev/null | grep -v -- -side | head -1)
ora_ip=$(ip_on qbs-oracle11 qbs-src-side)
sql_ip=$(ip_on qbs-mysql8  qbs-dst-side)
ora_def=$(ip_on qbs-oracle11 "$DEFAULT_NET")
sql_def=$(ip_on qbs-mysql8  "$DEFAULT_NET")
echo "    oracle: src-side=$ora_ip default=$ora_def    mysql: dst-side=$sql_ip default=$sql_def"

report R2  通   "$(tcp "$SRC" "$ora_ip" 1521)" "源端主机 → Oracle ${ora_ip}:1521（也是 R5 的正对照）"
report R4  通   "$(tcp "$DST" "$sql_ip" 3306)" "目标端主机 → MySQL ${sql_ip}:3306（也是 R3 的正对照）"
report R3  不通 "$(tcp "$SRC" "$sql_ip" 3306)" "源端主机 → MySQL ${sql_ip}:3306（两库不通）"
report R5  不通 "$(tcp "$DST" "$ora_ip" 1521)" "目标端主机 → Oracle ${ora_ip}:1521（两库不通）"
# 两个库在 default 网上各还有一个 IP。只挡侧网那一个等于没挡——绕一下就摸到了。
report R3b 不通 "$(tcp "$SRC" "$sql_def" 3306)" "源端主机 → MySQL ${sql_def}:3306（default 网那个 IP）"
report R5b 不通 "$(tcp "$DST" "$ora_def" 1521)" "目标端主机 → Oracle ${ora_def}:1521（default 网那个 IP）"

echo "==> R7a/R8a 正对照：目标端两个监听端确实活着（负判据全靠它们才有意义）"
# 15443 已经有人听的话，下面的探针监听端 bind 不上，R6a/R7/R7a 会红成「监听端不活」、
# R10 会红成「还有人听」—— 四条红的真实成因是同一个，而且都不是拓扑出了问题。
# 最常见的占用者是 #153 的 stunnel 服务端（隧道装完就归它了）。先说出来，别让人去查网络。
if [[ "$(tcp "$DST" 127.0.0.1 "$WHITELIST_PORT")" == 通 ]]; then
  echo "    !! ${WHITELIST_PORT} 已经有人在听 —— 多半是 #153 的 stunnel 服务端（隧道已装）。"
  echo "       探针监听端 bind 不上，R6a/R7/R7a/R10 会红在这个成因上，跟拓扑无关。"
  echo "       顺序是**先跑拓扑判据、再装隧道**；要现在复核拓扑，先 ./scripts/rehearsal-reset.sh"
  echo "       推倒重建（隧道随可写层一起归零），再跑本脚本。"
fi
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

echo "==> R6 跨容器直达被切断：监听端就在那儿听着，源端按 IP 直连仍必须摸不到"
dst_ip=$(ip_on "$DST" qbs-dst-side)
echo "    目标端在 qbs-dst-side 上的 IP = $dst_ip"
# 正对照：这个 IP 必须是真连得到令牌的那一个。取错了 IP（或压根没取到），
# 「源端连不上」的成因就成了「地址不存在」，跟「路由被切断」是两码事。
report R6a "$MARKER-WHITELIST" "$(read_token "$DST" "$dst_ip" "$WHITELIST_PORT")" \
  "目标端主机经自己的 side-net IP 自连 ${WHITELIST_PORT}"
report R6 无 "$(read_token "$SRC" "$dst_ip" "$WHITELIST_PORT")" \
  "源端主机 → ${dst_ip}:${WHITELIST_PORT}（按 IP 直达，拿不到令牌）"
# 按容器名同样摸不到 —— 这条留着有意义（客户现场也是按名字配的），但它单独不构成「切断」的证据。
report R6b 无 "$(read_token "$SRC" "$DST" "$WHITELIST_PORT")" \
  "源端主机 → ${DST}:${WHITELIST_PORT}（按容器名，名字也不该解析到）"

echo "==> R7–R8 公网那一跳只能走暴露端口（白名单），别的端口摸不到"
report R7 "$MARKER-WHITELIST" "$(read_token "$SRC" host.docker.internal "$WHITELIST_PORT")" \
  "源端主机 → 宿主:${WHITELIST_PORT} → 目标端（白名单口）"
report R8 不通 "$(tcp "$SRC" host.docker.internal "$BLOCKED_PORT")" \
  "源端主机 → 宿主:${BLOCKED_PORT}（目标端在听但没暴露）"

echo "==> R3c/R5c 宿主网关那条路：白名单口能过，两个库的发布端口不能过"
# 两个库在宿主上各发布了一个端口（`1521:1521` / `3306:3306`），而「公网一跳」的落点正是宿主。
# 于是「两库之间网络不通」还欠一条：源端经宿主网关摸不摸得到 MySQL。2026-08-20 实测**摸得到**
# ——路由黑洞挡不住它（另一个地址，且必须按端口区分：同一个网关上的 15443 就是白名单那一跳）。
# 现在由 `rehearsal-up.sh` 的第 2 层（端口级 DROP，IPv4/IPv6 两张表）挡住，这里判它是否真挡住了。
gw_addr=$(docker_line exec "$SRC" getent hosts host.docker.internal)
echo "    源端主机看到的 host.docker.internal = ${gw_addr}"
report R3c 不通 "$(tcp "$SRC" host.docker.internal 3306)" \
  "源端主机 → 宿主:3306（宿主上 MySQL 的发布端口；正对照是 R7）"
report R5d 通   "$(tcp "$DST" host.docker.internal "$WHITELIST_PORT")" \
  "目标端主机 → 宿主:${WHITELIST_PORT}（网关这条路对目标端也是活的）"
report R5c 不通 "$(tcp "$DST" host.docker.internal 1521)" \
  "目标端主机 → 宿主:1521（宿主上 Oracle 的发布端口；正对照是 R5d）"

echo "==> R10 收尾：探针监听端回收干净，${WHITELIST_PORT} 留给 #153 的 stunnel 服务端"
kill_probes
# 轮询到「没人听」为止，别用定长 sleep 赌进程退出——慢机上那是一条假红（脚本自己在 R7a
# 处就写了这条纪律）。超时了就照实报最后一次实测，让它红在真实成因上。
for _ in $(seq 1 15); do
  [[ "$(tcp "$DST" 127.0.0.1 "$WHITELIST_PORT")" == 不通 ]] && break
  sleep 1
done
report R10 不通 "$(tcp "$DST" 127.0.0.1 "$WHITELIST_PORT")" "目标端主机自连 ${WHITELIST_PORT}（应已无人听）"

echo
echo "==== 前置 R0（#151 的构建目标，不是 #152 的拓扑判据）：PASS=$pre_pass FAIL=$pre_fail ===="
echo "==== 拓扑判据：PASS=$pass FAIL=$fail ===="
(( fail == 0 && pre_fail == 0 ))
