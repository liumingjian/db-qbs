#!/usr/bin/env bash
# #153 —— 在演练台的两台「主机」容器上把 stunnel 双端隧道装起来、打通。
#
# 装的每一步都照 `packaging/stunnel/README.md` 的六步走（换 yum 源 → 装 stunnel →
# 出证书 → 填模板 → 起 → 自检）。**这里是那份说明的可执行副本，不是第二套装法**：
# 两边不一致的时候以 README 为准，并且回来改这里——手册是交付物，脚本是它的回放。
#
# 幂等：重复跑会先把上一轮的 stunnel 与桩 sink 收掉再起，安全。
# 判据跑 ./scripts/rehearsal-tunnel-check.sh。
#
# 前提：./scripts/up.sh（两个库）+ ./scripts/rehearsal-up.sh（两台主机）已经跑过。
#
# 两个开关，都是给 #155 / #156 的装机演练用的（默认值与 #153 落地时一个字不差）：
#
#   --side both|source|target   默认 both。装机演练要的是**只把对端准备好**，
#                               本端那一头必须由人照手册一条条敲出来——脚本代劳了，
#                               「手册是走过的记录」这句话就当场作废。
#   --sink stub|real            默认 stub（#153 的桩，只绑回环、回一行标记）。
#                               `real` 把 #151 编出来的 db-qbs-sink 装到目标端主机上，
#                               它对 /v1/runs/<不存在> 回的是产品自己的错误信封
#                               （code=RUN_UNKNOWN）——**源端自检 S8 判的就是那个码**，
#                               桩回不出来，所以 #155 的「自检全绿」要用 real。
#                               注意：`rehearsal-tunnel-check.sh` 的 T3/T5/T7 按桩的标记判，
#                               `real` 下那三条会红在标记上，那是预期的，不是隧道坏了。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"
TPL="$ROOT/packaging/stunnel"

SIDE=both
SINK_KIND=stub
while [[ $# -gt 0 ]]; do
  case "$1" in
    --side) [[ $# -ge 2 ]] || { echo "--side 要跟 both|source|target" >&2; exit 2; }
            SIDE="$2"; shift 2 ;;
    --sink) [[ $# -ge 2 ]] || { echo "--sink 要跟 stub|real" >&2; exit 2; }
            SINK_KIND="$2"; shift 2 ;;
    -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done
case "$SIDE" in both|source|target) ;; *) echo "--side 只认 both|source|target" >&2; exit 2 ;; esac
case "$SINK_KIND" in stub|real) ;; *) echo "--sink 只认 stub|real" >&2; exit 2 ;; esac
do_src() { [[ "$SIDE" == both || "$SIDE" == source ]]; }
do_dst() { [[ "$SIDE" == both || "$SIDE" == target ]]; }

SRC=qbs-host-source
DST=qbs-host-target
WHITELIST_PORT=15443            # 目标端唯一对外露出的端口，扮演客户侧白名单
SINK_PORT=8080                  # sink 的 listen —— config/sink.toml.example 里的默认值
TARGET_HOST=host.docker.internal  # 演练台上「公网」那一跳的落点；真机上是客户给的公网 IP
CERT_OUT="$TPL/out"             # 证书材料，.gitignore 挡着，不进版本库
# 桩 sink 写回的标记。判据脚本按它判「经隧道真的到达了回环上的那个服务」，
# 两处必须一致 —— test-rehearsal-tunnel.sh 会静态核对。
MARKER=QBS-TUNNEL-OK

sides=()
do_src && sides+=("$SRC")
do_dst && sides+=("$DST")
echo "==> 装哪一端：$SIDE   目标端落点：$SINK_KIND sink"
for c in "${sides[@]}"; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/rehearsal-up.sh"; exit 1; }
done

# ---------------------------------------------------------------- 0 + 1：换源、装 stunnel
# CentOS 7 已 EOL，mirrorlist 停服，不换源第一条 yum 就 404。这段与
# packaging/centos7/Dockerfile 顶部那条 RUN 是同一份换法（vault 存档源，gpgcheck 不关），
# **只有一处已经不同**：下面的 baseurl 加了两个后备镜像（成因见那几行注释）。
# Dockerfile 那份还是单源，而且它的 VAULT_BASE 按架构分（aarch64 在 altarch 下），
# 后备镜像的路径也得跟着分——那是 #151 的镜像，改它要连着重建一次才算数，本票不动它。
# 下次构建镜像撞上同一个 403 时，照这里的写法补过去。
# 演练台在这一点上**不比真机宽松**：客户那台机器同样装不上任何包。
install_stunnel() {  # $1=容器
  echo "==> [$1] 换 yum 源到 vault，装 stunnel + openssl"
  docker exec -i "$1" bash -s <<'SH'
set -euo pipefail
# `stunnel -version` 在 4.56 上**以 1 退出**（打完就当成用法错误），
# 在 `set -e` 下会把这一步判成失败。所有取版本的地方都得兜住。
version_line() { { stunnel -version 2>&1 || true; } | head -1; }
if command -v stunnel >/dev/null 2>&1; then
  echo "    已装过：$(version_line)"
  exit 0
fi
rm -f /etc/yum.repos.d/*.repo
keys=$(ls /etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7*)
test -n "$keys"
rpm --import $keys
gpgkey_line=$(printf 'file://%s ' $keys)
for repo in os:base:Base updates:updates:Updates extras:extras:Extras; do
  dir=${repo%%:*}; rest=${repo#*:}; id=${rest%%:*}; label=${rest#*:}
  {
    echo "[${id}]"
    echo "name=CentOS-7.9.2009 - ${label} - vault"
    # 三个源，yum 按 failovermethod=priority 顺序 failover。**不是冗余，是必需**：
    # 2026-08-20 下午 vault.centos.org 前面那层 CDN 对 `*.sqlite.bz2` 回了几个小时 403
    # （`*.xml.gz` 照常 200），而 yum 优先取 sqlite 元数据——那几个小时里这里装不上任何包；
    # 当晚复测它自己好了。**正因为它会自己好、也会自己再坏**，单源就是一颗随时引爆的雷。
    # 存档内容本身是冻结的，镜像之间不会各说各话；第一个仍是 vault，另两个只在它拿不到时才轮到。
    echo "baseurl=http://vault.centos.org/7.9.2009/${dir}/\$basearch/ https://linuxsoft.cern.ch/centos-vault/7.9.2009/${dir}/\$basearch/ https://archive.kernel.org/centos-vault/7.9.2009/${dir}/\$basearch/"
    # yum 的 failovermethod 默认是 roundrobin —— 从 URL 列表里**随机**挑起点。
    # 不写这一行的话「vault 优先，另两个才轮到」只是句愿望，排障时按它推断会走偏。
    echo 'failovermethod=priority'
    echo 'gpgcheck=1'
    echo "gpgkey=${gpgkey_line}"
    echo 'enabled=1'
    echo
  } >> /etc/yum.repos.d/CentOS-Vault.repo
done
yum -y install stunnel openssl >/dev/null
echo "    $(version_line)"
SH
}
for c in "${sides[@]}"; do install_stunnel "$c"; done

# ---------------------------------------------------------------- 2：证书
# 已经有就复用 —— gen-certs.sh 自己也拒绝覆盖（换掉正在用的钥匙，报出来的是握手失败，
# 排障会从网络查起）。要重出就先删掉 packaging/stunnel/out/。
if [[ -f "$CERT_OUT/source-side/source.key" ]]; then
  echo "==> 复用已有的证书材料：$CERT_OUT"
else
  echo "==> 出两端的证书材料"
  "$TPL/gen-certs.sh" --out "$CERT_OUT" >/dev/null
fi
openssl x509 -in "$CERT_OUT/target-side/target.crt" -noout -subject

# ---------------------------------------------------------------- 3：拷贝 + 填模板
deploy() {  # $1=容器 $2=side（source/target）
  echo "==> [$1] 铺证书与配置到 /etc/stunnel/db-qbs/"
  docker exec "$1" mkdir -p /etc/stunnel/db-qbs
  local f
  for f in "$CERT_OUT/$2-side"/*; do docker cp "$f" "$1:/etc/stunnel/db-qbs/"; done
  docker cp "$TPL/$2-side/stunnel-sink.conf" "$1:/etc/stunnel/db-qbs/stunnel-sink.conf"
  docker exec "$1" bash -c 'chmod 600 /etc/stunnel/db-qbs/*.key'
}
if do_src; then deploy "$SRC" source; fi
if do_dst; then deploy "$DST" target; fi

echo "==> 填占位符（演练台的值；真机上 @@TARGET_HOST@@ 换成客户给的公网 IP）"
if do_dst; then
  docker exec "$DST" sed -i \
    -e "s/@@WHITELIST_PORT@@/$WHITELIST_PORT/" \
    -e "s/@@SINK_PORT@@/$SINK_PORT/" /etc/stunnel/db-qbs/stunnel-sink.conf
fi
if do_src; then
  docker exec "$SRC" sed -i \
    -e "s/@@SINK_LOCAL_PORT@@/$SINK_PORT/" \
    -e "s/@@TARGET_HOST@@/$TARGET_HOST/" \
    -e "s/@@TARGET_PORT@@/$WHITELIST_PORT/" /etc/stunnel/db-qbs/stunnel-sink.conf
fi
# 一个占位符都不许留下：留着的话 stunnel 要么起不来，要么连到一个字面量主机名上，
# 报的是 DNS 失败，跟「隧道配错了」看着不是一回事。
# 注释行（`;` 开头）里那句「占位符全部要填」自己就带着 `@@...@@` 字样，按裸 `@@` 判会恒红。
LEFTOVER="grep -vE '^[[:space:]]*;' /etc/stunnel/db-qbs/stunnel-sink.conf | grep -nE '@@[A-Z_]+@@'"
for c in "${sides[@]}"; do
  out=$(docker exec "$c" bash -c "$LEFTOVER" 2>/dev/null || true)
  [[ -z "$out" ]] || { echo "!! [$c] 配置里还留着占位符"; echo "$out"; exit 1; }
done
echo "    源端 connect / 目标端 accept："
if do_src; then docker exec "$SRC" grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf | sed 's/^/      源端 /'; fi
if do_dst; then docker exec "$DST" grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf | sed 's/^/      目标端 /'; fi

# ---------------------------------------------------------------- 4：起
# 收上一轮：幂等靠它，端口被上一轮占着的话 stunnel 起不来（报的是 bind 失败）。
# 走 /proc 不走 pkill —— centos:7 的 procps 不保证在（判据脚本里同一条纪律）。
# 标记走环境变量传进去，别写进 `bash -c` 的字面量：否则这条命令自己的 cmdline 就含标记，一跑就自杀。
#
# **`/proc/<pid>/cmdline` 的分隔符是 NUL，不是空格**：带空格的标记（`stunnel /etc/stunnel/db-qbs`）
# 直接 grep 一条都匹配不上，「重复跑安全」这句话因此从来没成立过 —— 2026-08-20 在 #155 的
# 源端演练上撞到：上一轮的 stunnel 还占着 15443，这一轮 `bind: Address already in use`。
# 先把 NUL 换成空格再匹配。
kill_by_marker() {  # $1=容器 $2=cmdline 里要匹配的串
  docker exec -e M="$2" "$1" bash -c '
    self=$$
    for p in /proc/[0-9]*; do
      pid=${p##*/}
      [ "$pid" = "$self" ] && continue
      tr "\0" " " < "$p/cmdline" 2>/dev/null | grep -qa "$M" && kill "$pid" 2>/dev/null
    done; true' >/dev/null 2>&1 || true
}

# 收完等端口真的空出来：kill 是异步的，紧接着 bind 会撞上 TIME_WAIT / 还没退干净的旧进程，
# 报的是「端口被占」，看着跟「上一轮没收掉」是同一条错。
wait_port_free() {  # $1=容器 $2=端口
  local i
  for i in $(seq 1 15); do
    docker exec "$1" bash -c "timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$2' 2>/dev/null" >/dev/null 2>&1 || return 0
    sleep 1
  done
  echo "    !! [$1] $2 端口一直有人听，下面的 stunnel 多半会 bind 失败"
  return 0
}
for c in "${sides[@]}"; do kill_by_marker "$c" "stunnel /etc/stunnel/db-qbs"; done
if do_dst; then
  kill_by_marker "$DST" "$MARKER"
  kill_by_marker "$DST" "db-qbs-sink"
  wait_port_free "$DST" "$WHITELIST_PORT"
  wait_port_free "$DST" "$SINK_PORT"
fi
if do_src; then wait_port_free "$SRC" "$SINK_PORT"; fi

# 桩 sink —— **只绑回环**，和真 sink 的 config/sink.toml.example 一个样（127.0.0.1:8080）。
# 真 sink 要等 #156 装上来；本票要证的是隧道那一段，落点是不是真产品不影响判据，
# 但「只绑回环」这条必须一模一样 —— 判据 T4 判的就是它。
start_stub_sink() {
echo "==> [$DST] 起桩 sink（127.0.0.1:${SINK_PORT}，只绑回环）"
docker exec -d "$DST" python -c "
import SocketServer   # $MARKER —— 收尾按这个标记回收
class H(SocketServer.StreamRequestHandler):
    def handle(self):
        self.rfile.readline()
        body = '$MARKER\n'
        self.wfile.write('HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' % (len(body), body))
class S(SocketServer.ThreadingTCPServer):
    allow_reuse_address = True
S(('127.0.0.1', $SINK_PORT), H).serve_forever()
"
}

# 真 sink —— #151 编出来的那个 x86_64 产物，配置就是 config/sink.toml.example 那一行。
# **只有它回得出 `RUN_UNKNOWN`**，而源端自检 S8 判的正是那个错误码（#154）：
# 「有人应答」不等于「那头是 sink」。#155 的「源端自检全绿」靠这一档。
start_real_sink() {
  local bin="$ROOT/packaging/centos7/out/bin/linux-amd64/db-qbs-sink"
  [[ -x "$bin" ]] || { echo "!! 缺 $bin —— 先跑 packaging/centos7/build.sh --platform linux/amd64"; exit 1; }
  echo "==> [$DST] 装并起真 sink（127.0.0.1:${SINK_PORT}，只绑回环）"
  docker exec "$DST" mkdir -p /opt/db-qbs/bin /etc/db-qbs
  docker cp "$bin" "$DST:/opt/db-qbs/bin/db-qbs-sink"
  docker exec "$DST" bash -c "chmod 0755 /opt/db-qbs/bin/db-qbs-sink
    printf 'listen = \"127.0.0.1:%s\"\n' '$SINK_PORT' > /etc/db-qbs/sink.toml
    chmod 0600 /etc/db-qbs/sink.toml"
  docker exec -d "$DST" bash -c \
    '/opt/db-qbs/bin/db-qbs-sink --config /etc/db-qbs/sink.toml >> /var/log/db-qbs-sink.log 2>&1'
}

if do_dst; then
  case "$SINK_KIND" in
    stub) start_stub_sink ;;
    real) start_real_sink ;;
  esac
fi

# 起的顺序：目标端先（sink → stunnel 服务端），源端后。反过来源端也起得来
# （stunnel 客户端不预连），但第一次搬运会以连接被拒收场 —— README 第 4 步同一句话。
start_order=()
do_dst && start_order+=("$DST")
do_src && start_order+=("$SRC")
for c in "${start_order[@]}"; do
  echo "==> [$c] 起 stunnel"
  docker exec "$c" stunnel /etc/stunnel/db-qbs/stunnel-sink.conf \
    || { echo "!! [$c] stunnel 起不来，日志："; docker exec "$c" tail -20 /var/log/db-qbs-stunnel-sink.log 2>/dev/null; exit 1; }
done

# stunnel 是 fork 到后台的，起完立刻判会撞上「还没 bind 完」这条假红。轮询到端口在听为止。
echo "==> 等两端端口就位"
wait_port() {  # $1=容器 $2=地址 $3=端口
  local i
  for i in $(seq 1 20); do
    docker exec "$1" bash -c "timeout 2 bash -c 'exec 3<>/dev/tcp/$2/$3' 2>/dev/null" && return 0
    sleep 1
  done
  return 1
}
if do_dst; then
  wait_port "$DST" 127.0.0.1 "$SINK_PORT"     || { echo "!! 目标端 sink（${SINK_KIND}）没起来"; \
    docker exec "$DST" tail -20 /var/log/db-qbs-sink.log 2>/dev/null; exit 1; }
  wait_port "$DST" 127.0.0.1 "$WHITELIST_PORT" || { echo "!! 目标端 stunnel 服务端没起来"; exit 1; }
fi
if do_src; then
  wait_port "$SRC" 127.0.0.1 "$SINK_PORT" || { echo "!! 源端 stunnel 客户端没起来"; exit 1; }
fi

if [[ "$SIDE" == both ]]; then
  echo "== 隧道就位；判据跑 ./scripts/rehearsal-tunnel-check.sh =="
else
  echo "== ${SIDE} 端就位（另一端没动）；两端都齐了才跑 ./scripts/rehearsal-tunnel-check.sh =="
fi
