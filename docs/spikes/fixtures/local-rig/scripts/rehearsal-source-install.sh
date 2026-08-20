#!/usr/bin/env bash
# #155 —— 在演练台的源端主机容器上，照 `docs/install/source-centos7.md` 从零装一遍源端。
#
# **这是那份手册的可执行回放，不是第二套装法**：每一段的命令与手册里那一步逐字对应，
# 两边不一致时以手册为准，并且回来改这里（与 rehearsal-tunnel-up.sh 对 packaging/stunnel/README.md
# 同一条纪律：手册是交付物，脚本是它的回放）。工具进仓库，否则换台机器这道演练就静默跳过。
#
# 判据是过程性的（ADR-0041 §6）：
#   1) 干净机器上自检**先红**，且是「逐条对期望表」的红，不是一片红；
#   2) 照手册装完之后自检 **S1–S8 全绿**；
#   3) source 起得来、经产品自己的 Oracle 连接路径连得上 Oracle（界面「测试连接」的等价命令）；
#   4) 全程没有一条「手册没写、临场解决」的命令。
#
# 前提（按顺序）：
#   ./scripts/up.sh                                   两个库
#   ./scripts/rehearsal-up.sh                         两台主机 + 切断
#   ./scripts/rehearsal-topology-check.sh --reset     拓扑判据（**装隧道之前跑**）
#   ./scripts/rehearsal-tunnel-up.sh --side target --sink real
#                                                     只把**对端**准备好：真 sink + stunnel 服务端。
#                                                     源端那一头由本脚本照手册一条条敲——脚本代劳了，
#                                                     「手册是走过的记录」这句话就当场作废。
#   packaging/centos7/build.sh --platform linux/amd64 两个二进制（行李清单第 1 项）
#
# 用法：./scripts/rehearsal-source-install.sh
#
# **没有「接着上次往下跑」这种开关**：第 1 步判的是干净机器上的先红形状，
# 在装过一半的机器上它必然不成立；第 6/8 步的 stunnel 与 source 还会撞上上一轮占着的端口，
# 而 S6/S7 会对着**上一轮那两个老进程**判绿。要重跑就先 ./scripts/rehearsal-reset.sh
# 推倒重建，再从 ./scripts/rehearsal-tunnel-up.sh --side target --sink real 走一遍。
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"
TPL="$ROOT/packaging/stunnel"

SRC=qbs-host-source
DST=qbs-host-target
SINK_PORT=8080
WHITELIST_PORT=15443
TARGET_HOST=host.docker.internal   # 真机上是客户给的公网 IP（手册真机差异 ④）
ORACLE_SERVICE=XE
ORACLE_USER=spike
ORACLE_PASSWORD=spike123
# Instant Client 19c Basic，x86_64。免 Oracle 账号的直链，与 Dockerfile.client 里 arm64 那条同源。
# 行李清单第 3 项写的「出发前下好」，在演练台上就是这份缓存。
IC_ZIP_NAME=instantclient-basic-linux.x64-19.32.0.0.0dbru.zip
IC_URL="https://download.oracle.com/otn_software/linux/instantclient/1932000/$IC_ZIP_NAME"
IC_CACHE="${IC_CACHE:-$HOME/.cache/db-qbs}"
BIN_DIR="$ROOT/packaging/centos7/out/bin/linux-amd64"

[[ $# -eq 0 ]] || { echo "用法：$0   （不带参数；要重跑先 ./scripts/rehearsal-reset.sh）"; exit 2; }

step() { echo; echo "######## $* ########"; }
x()    { echo "  \$ $*"; docker exec "$SRC" bash -lc "$*"; }        # 手册里的一条命令，原样跑
xq()   { docker exec "$SRC" bash -lc "$*"; }                        # 不回显（取值用）

# ---------------------------------------------------------------- 前提
for c in "$SRC" "$DST"; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/rehearsal-up.sh"; exit 1; }
done
for b in db-qbs-source db-qbs-source-run; do
  [[ -x "$BIN_DIR/$b" ]] || { echo "!! 缺 $BIN_DIR/$b —— 先跑 packaging/centos7/build.sh --platform linux/amd64"; exit 1; }
done
[[ -f "$TPL/out/source-side/source.key" ]] \
  || { echo "!! 缺 $TPL/out/source-side/ 的证书材料 —— 先跑 ./scripts/rehearsal-tunnel-up.sh --side target --sink real"; exit 1; }
# 对端在不在。不在的话第 9 步的 S8 转不了绿，而红的成因与源端无关——先说出来。
if ! docker exec "$DST" bash -c "timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/$WHITELIST_PORT'" 2>/dev/null; then
  echo "!! 目标端 $WHITELIST_PORT 没人听 —— 先跑 ./scripts/rehearsal-tunnel-up.sh --side target --sink real"
  exit 1
fi

# Oracle 在源端那张侧网上的地址。手册里这是「客户给的 Oracle 地址」。
# 按 **IP** 取、不按容器名：`oracle` 这个名字解析到的是 default 网那个 IP，而那张网被切断了
# （ADR-0041 增补 4/5），按名字连出来的失败与路由无关。
ORACLE_HOST=$(docker inspect -f '{{index .NetworkSettings.Networks "qbs-src-side" "IPAddress"}}' qbs-oracle11 2>/dev/null)
[[ -n "$ORACLE_HOST" ]] || { echo "!! 取不到 Oracle 在 qbs-src-side 上的 IP"; exit 1; }
echo "==> 演练台代入的现场参数"
echo "    Oracle       = $ORACLE_HOST:1521/${ORACLE_SERVICE}（手册：客户给的 Oracle 地址）"
echo "    目标端入口    = $TARGET_HOST:${WHITELIST_PORT}（手册：客户给的公网 IP + 白名单口）"

# ---------------------------------------------------------------- 第 0 步：把行李搬进机器
step "第 0 步：把行李搬进机器（真机是 U 盘 / scp，演练台是 docker cp）"
{
  mkdir -p "$IC_CACHE"
  if [[ -f "$IC_CACHE/$IC_ZIP_NAME" ]] && unzip -tq "$IC_CACHE/$IC_ZIP_NAME" >/dev/null 2>&1; then
    echo "  复用缓存的 Instant Client：$IC_CACHE/$IC_ZIP_NAME"
  else
    echo "  下 Instant Client 19c Basic（x86_64，84 MB）——行李清单第 3 项「出发前下好」的那一份"
    # 84 MB 的包偶发断流（Dockerfile.client 里踩过），带重试与断点续传，解压前先验完整性：
    # 半截 zip 解出来是个「装得上、连库那一刻才炸」的目录，比下载失败难查得多。
    curl -fSL --retry 5 --retry-all-errors --retry-delay 3 -C - -o "$IC_CACHE/$IC_ZIP_NAME" "$IC_URL" \
      && unzip -tq "$IC_CACHE/$IC_ZIP_NAME" >/dev/null \
      || { echo "!! Instant Client 下载或校验失败"; exit 1; }
  fi
  docker exec "$SRC" rm -rf /root/dist
  docker exec "$SRC" mkdir -p /root/dist
  docker cp "$IC_CACHE/$IC_ZIP_NAME"            "$SRC:/root/dist/"
  docker cp "$BIN_DIR/db-qbs-source"            "$SRC:/root/dist/"
  docker cp "$BIN_DIR/db-qbs-source-run"        "$SRC:/root/dist/"
  docker cp "$ROOT/packaging/preflight/preflight-source.sh" "$SRC:/root/dist/"
  docker cp "$TPL/source-side/stunnel-sink.conf" "$SRC:/root/dist/"
  for f in "$TPL/out/source-side"/*; do docker cp "$f" "$SRC:/root/dist/"; done
  x 'ls /root/dist'
}

# ---------------------------------------------------------------- 第 1 步：自检先红
step "第 1 步：上机第一件事——跑自检，让它先红"
x 'chmod +x /root/dist/preflight-source.sh'
echo "  \$ QBS_ORACLE_HOST=<Oracle 地址> /root/dist/preflight-source.sh"
first_out=$(docker exec -e QBS_ORACLE_HOST="$ORACLE_HOST" "$SRC" bash -lc '/root/dist/preflight-source.sh' 2>&1)
first_rc=$?
echo "$first_out"
echo "  （退出码 $first_rc —— 干净机器上就该是 1）"

# 干净机器上的期望形状。**不是「一片红」**：一张全红的期望表会把「脚本恒红」这种假绿放进来
# （#154 的 P1b 判过同一件事）。这里逐条对齐，与手册第 1 步那张表同一份。
step "第 1 步的判定：先红的形状要与手册那张表逐条对齐"
expect_first=(S1:PASS S2:FAIL S3:FAIL S4:FAIL S5:PASS S6:FAIL S7:FAIL S8:FAIL)
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
docker exec -i "$SRC" bash -s <<'SH'
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
# 光看上面那段的退出码不够 —— 它曾经因为 `docker exec` 没接 stdin 而整段静默跳过，退出码照样是 0。
# 事后核对：repo 文件在、且 yum 真的列得出源。
xq 'test -f /etc/yum.repos.d/CentOS-Vault.repo && yum -q repolist >/dev/null' || yum_rc=1

# ---------------------------------------------------------------- 第 3 步：四个包
step "第 3 步：装 libaio / stunnel / openssl / unzip / curl / iproute"
x 'yum -y install libaio stunnel openssl unzip curl iproute >/dev/null && rpm -q libaio stunnel openssl unzip curl iproute'
pkg_rc=$?

# ---------------------------------------------------------------- 第 4 步：Instant Client
step "第 4 步：铺 Oracle Instant Client 19c"
x 'uname -m'
x 'mkdir -p /opt/oracle'
x "unzip -oq /root/dist/$IC_ZIP_NAME -d /opt/oracle"
x 'ln -sfn /opt/oracle/instantclient_19_32 /opt/oracle/instantclient'
x 'ls -l /opt/oracle/instantclient/libclntsh.so*'
# 注册进 ldconfig —— 手册第 4 步后半段。少了它自检照样全绿，而「测试连接」当场
# DPI-1047 libnnz19.so（2026-08-20 第一趟演练的实况，回写手册后重走）。
x 'echo /opt/oracle/instantclient > /etc/ld.so.conf.d/oracle-instantclient.conf'
x 'ldconfig'
x "ldconfig -p | grep -c 'libclntsh.so'"
x "ldd /opt/oracle/instantclient/libclntsh.so | grep -c 'not found'"

# ---------------------------------------------------------------- 第 5 步：两个二进制
step "第 5 步：铺两个二进制"
x 'mkdir -p /opt/db-qbs/bin'
x 'install -m 0755 /root/dist/db-qbs-source /root/dist/db-qbs-source-run /opt/db-qbs/bin/'
x '/opt/db-qbs/bin/db-qbs-source; echo "exit=$?"'

# ---------------------------------------------------------------- 第 6 步：stunnel 客户端
step "第 6 步：装 stunnel 客户端"
x 'mkdir -p /etc/stunnel/db-qbs'
x 'cp /root/dist/source.crt /root/dist/source.key /root/dist/target.crt /etc/stunnel/db-qbs/'
x 'cp /root/dist/stunnel-sink.conf /etc/stunnel/db-qbs/stunnel-sink.conf'
x 'chmod 600 /etc/stunnel/db-qbs/*.key'
x "sed -i 's/@@SINK_LOCAL_PORT@@/$SINK_PORT/; s|@@TARGET_HOST@@|$TARGET_HOST|; s/@@TARGET_PORT@@/$WHITELIST_PORT/' /etc/stunnel/db-qbs/stunnel-sink.conf"
x "grep -vE '^[[:space:]]*;' /etc/stunnel/db-qbs/stunnel-sink.conf | grep -nE '@@[A-Z_]+@@' && echo '还有没填的！' || echo '占位符已填完'"
x "grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf"
x 'openssl x509 -in /etc/stunnel/db-qbs/target.crt -noout -fingerprint -sha256'
echo "  （对端那张证书在目标端上的指纹，两边必须一致）"
docker exec "$DST" bash -lc 'openssl x509 -in /etc/stunnel/db-qbs/target.crt -noout -fingerprint -sha256' 2>/dev/null \
  || echo "  （目标端上没有 openssl，指纹核对这一档演练台上跳过）"
x 'stunnel /etc/stunnel/db-qbs/stunnel-sink.conf'
x 'sleep 1; cat /var/run/db-qbs-stunnel-sink.pid'
x 'tail -5 /var/log/db-qbs-stunnel-sink.log'

# ---------------------------------------------------------------- 第 7 步：source.toml
step "第 7 步：写 source.toml"
x 'mkdir -p /etc/db-qbs /var/lib/db-qbs-source'
docker exec -i "$SRC" bash -s <<'SH'
set -euo pipefail
cat > /etc/db-qbs/source.toml <<'EOF'
oracle_client_lib_dir = "/opt/oracle/instantclient"
sink_base_url = "http://127.0.0.1:8080"
listen = "127.0.0.1:8088"
data_dir = "/var/lib/db-qbs-source"
EOF
chmod 0600 /etc/db-qbs/source.toml
cat /etc/db-qbs/source.toml
SH

# ---------------------------------------------------------------- 第 8 步：起 source
step "第 8 步：起 source"
x 'nohup /opt/db-qbs/bin/db-qbs-source --config /etc/db-qbs/source.toml >> /var/log/db-qbs-source.log 2>&1 & sleep 2; tail -20 /var/log/db-qbs-source.log'
# 起没起来轮询判，别用定长 sleep 赌（Rosetta 下冷启动慢）。
for _ in $(seq 1 20); do
  xq "timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/8088'" >/dev/null 2>&1 && break
  sleep 1
done

# ---------------------------------------------------------------- 第 9 步：自检全绿
step "第 9 步：再跑一遍自检——这次要全绿"
echo "  \$ QBS_ORACLE_HOST=<Oracle 地址> /root/dist/preflight-source.sh; echo \"exit=\$?\""
second_out=$(docker exec -e QBS_ORACLE_HOST="$ORACLE_HOST" "$SRC" bash -lc '/root/dist/preflight-source.sh' 2>&1)
second_rc=$?
echo "$second_out"
echo "  （退出码 ${second_rc}）"

# ---------------------------------------------------------------- 第 10 步：连 Oracle
step "第 10 步：连上 Oracle（界面「测试连接」的等价命令）"
draft="{\"name\":\"演练 Oracle\",\"kind\":\"oracle\",\"connect_string\":\"//$ORACLE_HOST:1521/$ORACLE_SERVICE\",\"username\":\"$ORACLE_USER\",\"password\":\"$ORACLE_PASSWORD\"}"
echo "  \$ curl -sS -X POST http://127.0.0.1:8088/api/datasources/test-connection -d '<草稿>'"
test_out=$(docker exec -e BODY="$draft" "$SRC" bash -lc \
  'curl -sS -X POST http://127.0.0.1:8088/api/datasources/test-connection -H "Content-Type: application/json" -d "$BODY"' 2>&1)
echo "  $test_out"

# ---------------------------------------------------------------- 收尾核对
step "收尾核对"
x "ss -ltnp | grep -E '8080|8088'"
x 'ls -l /etc/db-qbs/source.toml /etc/stunnel/db-qbs/source.key'

# ---------------------------------------------------------------- 总账
step "总账"
green=$(grep -cE '^  S[0-9] +PASS' <<<"$second_out")
red=$(grep -cE '^  S[0-9] +FAIL' <<<"$second_out")
ok=0
verdict() { # $1=说明 $2=期望 $3=实测
  if [[ "$3" == "$2" ]]; then printf '  PASS  %-44s 实测=%s\n' "$1" "$3"
  else printf '  FAIL  %-44s 期望=%s 实测=%s\n' "$1" "$2" "$3"; ok=1; fi
}
verdict "第 1 步：干净机器上的先红逐条对齐期望表" 0 "$pre_fail"
verdict "第 2 步：yum 源换成了（vault 三源）" 0 "$yum_rc"
verdict "第 3 步：那几个包都装上了" 0 "$pkg_rc"
verdict "第 9 步：自检 S1–S8 全绿" "8/0" "$green/$red"
verdict "第 9 步：自检退出码" 0 "$second_rc"
verdict "第 10 步：测试连接（产品自己的 Oracle 连接路径）" 通过 \
  "$(grep -q '"ok":true' <<<"$test_out" && echo 通过 || echo 未通过)"
echo
if (( ok )); then
  echo "==== 源端装机演练：未达成（上面 FAIL 那几条就是手册还欠的地方）===="
  exit 1
fi
echo "==== 源端装机演练：达成（自检 S1–S8 全绿、Oracle 连得上、经隧道摸得到 sink）===="
