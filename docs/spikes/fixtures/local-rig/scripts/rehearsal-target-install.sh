#!/usr/bin/env bash
# #156 —— 在演练台的目标端主机容器上，照 `docs/install/target-centos7.md` 从零装一遍目标端。
#
# **这是那份手册的可执行回放，不是第二套装法**：每一段的命令与手册里那一步逐字对应，
# 两边不一致时以手册为准，并且回来改这里（与 rehearsal-source-install.sh 对 source-centos7.md
# 同一条纪律：手册是交付物，脚本是它的回放）。工具进仓库，否则换台机器这道演练就静默跳过。
#
# 判据是过程性的（ADR-0041 §6），对应票面四条：
#   1) 干净机器上自检**先红**，且是「逐条对期望表」的红，不是一片红；照手册装完之后 **D1–D9 全绿**；
#   2) sink 只绑回环；从「公网」侧只有经 stunnel 服务端（白名单端口扮演）能到达它——
#      手册第 10 步那四条从源端主机上打，完整版是 rehearsal-tunnel-check.sh --sink real（T0–T11）；
#   3) MySQL 连通且开连接仪式三前提满足——D4–D7 是问真 sink 要的，本趟是它们第一次在真 sink 上转绿；
#   4) 全程没有一条「手册没写、临场解决」的命令。
#
# 前提（按顺序）：
#   ./scripts/up.sh                                   两个库
#   ./scripts/rehearsal-up.sh                         两台主机 + 切断
#   ./scripts/rehearsal-topology-check.sh --reset     拓扑判据（**装隧道之前跑**）
#   ./scripts/rehearsal-tunnel-up.sh --side source    只把**对端**准备好：源端的 stunnel 客户端 + 证书 + openssl。
#                                                     目标端那一头由本脚本照手册一条条敲——脚本代劳了，
#                                                     「手册是走过的记录」这句话就当场作废。
#   packaging/centos7/build.sh --platform linux/amd64 sink 二进制（行李清单第 2 项）
#
# 用法：./scripts/rehearsal-target-install.sh
#
# **没有「接着上次往下跑」这种开关**：第 1 步判的是干净机器上的先红形状，
# 在装过一半的机器上它必然不成立；第 6/8 步的 sink 与 stunnel 还会撞上上一轮占着的端口，
# 而 D2/D8/D9 会对着**上一轮那两个老进程**判绿。要重跑就先 ./scripts/rehearsal-reset.sh
# 推倒重建，再从 ./scripts/rehearsal-tunnel-up.sh --side source 走一遍。
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"
TPL="$ROOT/packaging/stunnel"

SRC=qbs-host-source
DST=qbs-host-target
SINK_PORT=8080
WHITELIST_PORT=15443               # 真机上是客户给的白名单端口（手册真机差异 ⑦）
GW=host.docker.internal            # 「公网」那一跳的落点；真机上是客户给的公网 IP
MYSQL_USER=spike
MYSQL_PASSWORD=spike123
MYSQL_DATABASE=qbs
BIN_DIR="$ROOT/packaging/centos7/out/bin/linux-amd64"

[[ $# -eq 0 ]] || { echo "用法：$0   （不带参数；要重跑先 ./scripts/rehearsal-reset.sh）"; exit 2; }

step() { echo; echo "######## $* ########"; }
x()    { echo "  \$ $*"; docker exec "$DST" bash -lc "$*"; }        # 手册里的一条命令，原样跑
xq()   { docker exec "$DST" bash -lc "$*"; }                        # 不回显（取值用）
xs()   { echo "  [源端] \$ $*"; docker exec "$SRC" bash -lc "$*"; }  # 手册第 10 步：在源端那台上敲
# 自检的口令走文件，账号 / 库名 / 地址走环境变量（手册第 1 步原话）。口令不进 argv。
preflight() {
  docker exec -e QBS_MYSQL_HOST="$MYSQL_HOST" -e QBS_MYSQL_USER="$MYSQL_USER" \
    -e QBS_MYSQL_DATABASE="$MYSQL_DATABASE" -e QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass \
    "$DST" bash -lc '/root/dist/preflight-target.sh' 2>&1
}

# ---------------------------------------------------------------- 前提
for c in "$SRC" "$DST"; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/rehearsal-up.sh"; exit 1; }
done
[[ -x "$BIN_DIR/db-qbs-sink" ]] || { echo "!! 缺 $BIN_DIR/db-qbs-sink —— 先跑 packaging/centos7/build.sh --platform linux/amd64"; exit 1; }
[[ -f "$TPL/out/target-side/target.key" ]] \
  || { echo "!! 缺 $TPL/out/target-side/ 的证书材料 —— 先跑 ./scripts/rehearsal-tunnel-up.sh --side source（它顺手出证书）"; exit 1; }
# 对端在不在：源端的 stunnel 客户端要在它自己的回环 8080 上听着，第 10 步的第 4 条才判得了；
# 它没起的话那条红的成因与目标端无关——先说出来。
if ! docker exec "$SRC" bash -c "timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$SINK_PORT'" 2>/dev/null; then
  echo "!! 源端 127.0.0.1:${SINK_PORT} 没人听 —— 先跑 ./scripts/rehearsal-tunnel-up.sh --side source"
  exit 1
fi

# MySQL 在目标端那张侧网上的地址。手册里这是「客户给的 MySQL 地址」。
# 按 **IP** 取、不按容器名（与 rehearsal-source-install.sh 取 Oracle 同一条纪律，ADR-0041 增补 4/5）。
MYSQL_HOST=$(docker inspect -f '{{index .NetworkSettings.Networks "qbs-dst-side" "IPAddress"}}' qbs-mysql8 2>/dev/null)
[[ -n "$MYSQL_HOST" ]] || { echo "!! 取不到 MySQL 在 qbs-dst-side 上的 IP"; exit 1; }
echo "==> 演练台代入的现场参数"
echo "    MySQL        = $MYSQL_HOST:3306/${MYSQL_DATABASE}，账号 ${MYSQL_USER}（手册：客户给的 MySQL 地址 / 账号 / 库名）"
echo "    白名单口      = ${WHITELIST_PORT}，「公网」落点 = ${GW}（手册：客户给的白名单端口 / 公网 IP）"

# ---------------------------------------------------------------- 第 0 步：把行李搬进机器
step "第 0 步：把行李搬进机器（真机是 U 盘 / scp，演练台是 docker cp）"
{
  docker exec "$DST" rm -rf /root/dist
  docker exec "$DST" mkdir -p /root/dist
  docker cp "$BIN_DIR/db-qbs-sink"                            "$DST:/root/dist/"
  docker cp "$ROOT/packaging/preflight/preflight-target.sh"  "$DST:/root/dist/"
  docker cp "$TPL/target-side/stunnel-sink.conf"             "$DST:/root/dist/"
  docker cp "$TPL/target-side/db-qbs-stunnel.service"        "$DST:/root/dist/"
  for f in "$TPL/out/target-side"/*; do docker cp "$f" "$DST:/root/dist/"; done
  x 'ls /root/dist'
}

# ---------------------------------------------------------------- 第 1 步：自检先红
step "第 1 步：上机第一件事——跑自检，让它先红"
x 'chmod +x /root/dist/preflight-target.sh'
echo "  \$ umask 077; printf '%s\\n' '<MySQL 口令>' > /root/.qbs-mysql-pass"
docker exec -i -e PW="$MYSQL_PASSWORD" "$DST" bash -c 'umask 077; printf "%s\n" "$PW" > /root/.qbs-mysql-pass'
echo "  \$ QBS_MYSQL_HOST=<MySQL 地址> QBS_MYSQL_USER=<账号> QBS_MYSQL_DATABASE=<库名> QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass /root/dist/preflight-target.sh"
first_out=$(preflight)
first_rc=$?
echo "$first_out"
echo "  （退出码 ${first_rc} —— 干净机器上就该是 1）"

# 干净机器上的期望形状。**不是「一片红」**：一张全红的期望表会把「脚本恒红」这种假绿放进来
# （#154 的 P5b 判过同一件事）。这里逐条对齐，与手册第 1 步那张表同一份。
step "第 1 步的判定：先红的形状要与手册那张表逐条对齐"
expect_first=(D1:PASS D2:FAIL D3:FAIL D4:FAIL D5:FAIL D6:FAIL D7:FAIL D8:FAIL D9:FAIL)
pre_fail=0
for e in "${expect_first[@]}"; do
  id=${e%%:*}; want=${e##*:}
  got=$(grep -oE "^  $id +(PASS|FAIL)" <<<"$first_out" | awk '{print $2}' | head -1)
  got=${got:-取不到}
  if [[ "$got" == "$want" ]]; then printf '  %-3s PASS  干净机器上应为 %-4s 实测=%s\n' "$id" "$want" "$got"
  else printf '  %-3s FAIL  干净机器上应为 %-4s 实测=%s\n' "$id" "$want" "$got"; pre_fail=$((pre_fail+1)); fi
done

# ---------------------------------------------------------------- 第 2 步：换 yum 源
step "第 2 步：换 yum 源到 vault 存档"
x 'cp -a /etc/yum.repos.d /root/yum.repos.d.bak'
x 'rm -f /etc/yum.repos.d/*.repo'
docker exec -i "$DST" bash -s <<'SH'
set -euo pipefail
keys=$(ls /etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7*)
test -n "$keys"
rpm --import $keys
gpgkey_line=$(printf 'file://%s ' $keys)
VAULT_BASE=http://vault.centos.org/7.9.2009
VAULT_MIRRORS="https://linuxsoft.cern.ch/centos-vault/7.9.2009 https://archive.kernel.org/centos-vault/7.9.2009"
for repo in os:base:Base updates:updates:Updates extras:extras:Extras; do
  dir=${repo%%:*}; rest=${repo#*:}; id=${rest%%:*}; label=${rest#*:}
  urls="$VAULT_BASE/$dir/\$basearch/"
  for m in $VAULT_MIRRORS; do urls="$urls $m/$dir/\$basearch/"; done
  {
    echo "[$id]"
    echo "name=CentOS-7.9.2009 - $label - vault"
    echo "baseurl=$urls"
    echo 'failovermethod=priority'
    echo 'gpgcheck=1'
    echo "gpgkey=$gpgkey_line"
    echo 'enabled=1'
    echo
  } >> /etc/yum.repos.d/CentOS-Vault.repo
done
yum -y makecache fast >/dev/null && echo "yum 源就位"
SH
yum_rc=$?
# 光看上面那段的退出码不够 —— `docker exec` 没接 stdin 时整段会静默跳过、退出码照样是 0（#155 撞过）。
# 事后核对：repo 文件在、且 yum 真的列得出源。
xq 'test -f /etc/yum.repos.d/CentOS-Vault.repo && yum -q repolist >/dev/null' || yum_rc=1

# ---------------------------------------------------------------- 第 3 步：四个包
step "第 3 步：装 stunnel / openssl / curl / iproute"
x 'yum -y install stunnel openssl curl iproute >/dev/null && rpm -q stunnel openssl curl iproute'
pkg_rc=$?

# ---------------------------------------------------------------- 第 4 步：sink 二进制
step "第 4 步：铺 sink 二进制"
x 'uname -m'
x 'mkdir -p /opt/db-qbs/bin'
x 'install -m 0755 /root/dist/db-qbs-sink /opt/db-qbs/bin/'
x '/opt/db-qbs/bin/db-qbs-sink; echo "exit=$?"'

# ---------------------------------------------------------------- 第 5 步：sink.toml
step "第 5 步：写 sink.toml"
x 'mkdir -p /etc/db-qbs'
docker exec -i "$DST" bash -s <<'SH'
set -euo pipefail
cat > /etc/db-qbs/sink.toml <<'EOF'
listen = "127.0.0.1:8080"
EOF
chmod 0600 /etc/db-qbs/sink.toml
cat /etc/db-qbs/sink.toml
SH

# ---------------------------------------------------------------- 第 6 步：起 sink
step "第 6 步：起 sink"
x 'nohup /opt/db-qbs/bin/db-qbs-sink --config /etc/db-qbs/sink.toml >> /var/log/db-qbs-sink.log 2>&1 & sleep 1; tail -5 /var/log/db-qbs-sink.log'
# 起没起来轮询判，别用定长 sleep 赌（Rosetta 下冷启动慢）。
for _ in $(seq 1 20); do
  xq "timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$SINK_PORT'" >/dev/null 2>&1 && break
  sleep 1
done
echo "  \$ curl -sS http://127.0.0.1:8080/v1/runs/__probe__; echo"
local_probe=$(xq 'curl -sS http://127.0.0.1:8080/v1/runs/__probe__; echo' 2>&1)
echo "  $local_probe"

# ---------------------------------------------------------------- 第 7 步：MySQL 那一头
step "第 7 步：MySQL 那一头的三前提（给 DBA 的纸条——这台机器上没有命令可敲，第 9 步的 D4–D7 去判）"
echo "  演练台上的 MySQL 是 compose 起的：--character-set-server=utf8mb4、max_allowed_packet 是 8.0 的默认 64M、"
echo "  账号 ${MYSQL_USER} 对库 ${MYSQL_DATABASE} 全权（手册真机差异 ⑥：真机上这三项每一项都要问 DBA）。"

# ---------------------------------------------------------------- 第 8 步：stunnel 服务端
step "第 8 步：装 stunnel 服务端"
x 'mkdir -p /etc/stunnel/db-qbs'
x 'cp /root/dist/target.crt /root/dist/target.key /root/dist/source.crt /etc/stunnel/db-qbs/'
x 'cp /root/dist/stunnel-sink.conf /etc/stunnel/db-qbs/stunnel-sink.conf'
x 'chmod 600 /etc/stunnel/db-qbs/*.key'
x "sed -i 's/@@WHITELIST_PORT@@/$WHITELIST_PORT/; s/@@SINK_PORT@@/$SINK_PORT/' /etc/stunnel/db-qbs/stunnel-sink.conf"
x "grep -vE '^[[:space:]]*;' /etc/stunnel/db-qbs/stunnel-sink.conf | grep -nE '@@[A-Z_]+@@' && echo '还有没填的！' || echo '占位符已填完'"
x "grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf"
x 'openssl x509 -in /etc/stunnel/db-qbs/target.crt -noout -fingerprint -sha256'
x 'openssl x509 -in /etc/stunnel/db-qbs/source.crt -noout -fingerprint -sha256'
echo "  （同两张证书在源端那台上的指纹，两边必须一致）"
docker exec "$SRC" bash -lc 'for c in target source; do printf "  源端 %s.crt  " "$c"; openssl x509 -in /etc/stunnel/db-qbs/$c.crt -noout -fingerprint -sha256; done' 2>/dev/null \
  || echo "  （源端上没有 openssl 或证书，指纹核对这一档演练台上跳过）"
x 'stunnel /etc/stunnel/db-qbs/stunnel-sink.conf'
x 'sleep 1; cat /var/run/db-qbs-stunnel-sink.pid'
x 'tail -5 /var/log/db-qbs-stunnel-sink.log'
for _ in $(seq 1 20); do
  xq "timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$WHITELIST_PORT'" >/dev/null 2>&1 && break
  sleep 1
done

# ---------------------------------------------------------------- 第 9 步：自检全绿
step "第 9 步：再跑一遍自检——这次要全绿"
echo "  \$ QBS_MYSQL_HOST=<MySQL 地址> QBS_MYSQL_USER=<账号> QBS_MYSQL_DATABASE=<库名> QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass /root/dist/preflight-target.sh; echo \"exit=\$?\""
second_out=$(preflight)
second_rc=$?
echo "$second_out"
echo "  （退出码 ${second_rc}）"

# ---------------------------------------------------------------- 第 10 步：从公网侧核一眼
step "第 10 步：从「公网」侧核一眼——只有经隧道才到得了 sink（在源端那台上敲）"
xs "curl -sS --max-time 8 http://$GW:$WHITELIST_PORT/v1/runs/__probe__; echo \"exit=\$?\""
plain=$(docker exec "$SRC" bash -lc "curl -s --max-time 8 http://$GW:$WHITELIST_PORT/v1/runs/__probe__ 2>/dev/null | grep -o -m1 RUN_UNKNOWN"; true)
xs "printf 'GET /v1/runs/__probe__ HTTP/1.0\\r\\n\\r\\n' | openssl s_client -connect $GW:$WHITELIST_PORT -CAfile /etc/stunnel/db-qbs/target.crt -cert /etc/stunnel/db-qbs/source.crt -key /etc/stunnel/db-qbs/source.key -quiet 2>/dev/null | grep -o RUN_UNKNOWN"
with_cert=$(docker exec "$SRC" bash -lc "printf 'GET /v1/runs/__probe__ HTTP/1.0\r\n\r\n' | timeout 12 openssl s_client -connect $GW:$WHITELIST_PORT -CAfile /etc/stunnel/db-qbs/target.crt -cert /etc/stunnel/db-qbs/source.crt -key /etc/stunnel/db-qbs/source.key -quiet 2>/dev/null | grep -o -m1 RUN_UNKNOWN"; true)
echo "  ${with_cert:-无}"
xs "printf 'GET /v1/runs/__probe__ HTTP/1.0\\r\\n\\r\\n' | openssl s_client -connect $GW:$WHITELIST_PORT -CAfile /etc/stunnel/db-qbs/target.crt -quiet 2>/dev/null | grep -c RUN_UNKNOWN"
no_cert=$(docker exec "$SRC" bash -lc "printf 'GET /v1/runs/__probe__ HTTP/1.0\r\n\r\n' | timeout 12 openssl s_client -connect $GW:$WHITELIST_PORT -CAfile /etc/stunnel/db-qbs/target.crt -quiet 2>/dev/null | grep -c RUN_UNKNOWN"; true)
echo "  ${no_cert:-0}"
xs 'curl -sS http://127.0.0.1:8080/v1/runs/__probe__; echo'
via_tunnel=$(docker exec "$SRC" bash -lc "curl -s --max-time 8 http://127.0.0.1:$SINK_PORT/v1/runs/__probe__ 2>/dev/null | grep -o -m1 RUN_UNKNOWN"; true)

# ---------------------------------------------------------------- 收尾核对
step "收尾核对"
x "ss -ltnp | grep -E '8080|15443'"
ss_out=$(xq "ss -ltn | grep -E ':8080[[:space:]]' || true")
x 'ls -l /etc/db-qbs/sink.toml /etc/stunnel/db-qbs/target.key /root/.qbs-mysql-pass'

# ---------------------------------------------------------------- 总账
step "总账"
green=$(grep -cE '^  D[0-9] +PASS' <<<"$second_out")
red=$(grep -cE '^  D[0-9] +FAIL' <<<"$second_out")
ok=0
verdict() { # $1=说明 $2=期望 $3=实测
  if [[ "$3" == "$2" ]]; then printf '  PASS  %-50s 实测=%s\n' "$1" "$3"
  else printf '  FAIL  %-50s 期望=%s 实测=%s\n' "$1" "$2" "$3"; ok=1; fi
}
verdict "第 1 步：干净机器上的先红逐条对齐期望表" 0 "$pre_fail"
verdict "第 2 步：yum 源换成了（vault 三源）" 0 "$yum_rc"
verdict "第 3 步：那几个包都装上了" 0 "$pkg_rc"
verdict "第 6 步：sink 在回环上应答（认 RUN_UNKNOWN）" RUN_UNKNOWN "$(grep -o -m1 RUN_UNKNOWN <<<"$local_probe" || echo 无)"
verdict "第 9 步：自检 D1–D9 全绿（含 D4–D7 三前提）" "9/0" "$green/$red"
verdict "第 9 步：自检退出码" 0 "$second_rc"
verdict "第 10 步①：明文打白名单口拿不到 sink" 无 "${plain:-无}"
verdict "第 10 步②：带客户端证书握手拿到 RUN_UNKNOWN" RUN_UNKNOWN "${with_cert:-无}"
verdict "第 10 步③：不带客户端证书被拒" 0 "${no_cert:-0}"
verdict "第 10 步④：经源端隧道入口拿到 RUN_UNKNOWN" RUN_UNKNOWN "${via_tunnel:-无}"
# 「只绑回环」按 ss 的实际监听地址判：8080 只许出现在 127.0.0.1 上。
verdict "收尾：8080 只绑回环（ss 实测）" 只在回环 \
  "$( { grep -q '127.0.0.1:8080' <<<"$ss_out" && ! grep -qE '(0\.0\.0\.0|\*|\[::\]):8080' <<<"$ss_out"; } && echo 只在回环 || echo 不是)"
echo
if (( ok )); then
  echo "==== 目标端装机演练：未达成（上面 FAIL 那几条就是手册还欠的地方）===="
  exit 1
fi
echo "==== 目标端装机演练：达成（自检 D1–D9 全绿、三前提经真 sink 判过、公网侧只有经隧道到得了 sink）===="
