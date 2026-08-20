#!/usr/bin/env bash
# #153 —— stunnel 双端隧道在演练台上的判据（T0–T11）。跑在 mac Docker 上（rexec 派发）。
#
# 要证的是票面那四条：
#   ① 经隧道可达目标端**回环上**的服务（T3/T5）
#   ② 「公网」那一跳走的是**加密**流量（T6/T6b/T7/T7b/T8）
#   ③ 目标端除白名单口外不暴露服务，回环之外摸不到 sink（T4/T9/T10/T11）
#   ④ 产品代码零改动 —— **不在这里判**，它是静态事实，归 ./scripts/test-rehearsal-tunnel.sh
#
# **每条负判据都配一条同址正对照**（与 rehearsal-topology-check.sh 同一条纪律）：
# 「拿不到」最容易假绿 —— 没人监听、进程没起、地址取错，得出的都是「拿不到」。
#   T4 ← T3（同一个 8080，从回环连必须拿得到）
#   T6 ← T7（同一个宿主:15443，握了手就必须拿得到 —— 否则「明文拿不到」可能只是那儿没人听）
#   T8 ← T7（同一条 TLS 通道，带证书必须成、不带必须败）
#   T10 ← T10a（同一个 IP、同一个端口，目标端自己连必须通）
#   T11 ← T5（同一个 8080，从回环连必须拿得到）
#
# 落点是什么，按 `--sink` 认（默认值与 #153 落地时一个字不差）：
#   --sink stub   #153 的桩 sink：落点应答一行标记 QBS-TUNNEL-OK，T3/T5/T7 认它。
#   --sink real   #156 起目标端装的是真 db-qbs-sink：它没有健康检查端点，路由全集是 /v1/runs* 与 /v1/target/*，
#                 所以打一个不存在的 run，认它 404 里的产品错误码 RUN_UNKNOWN（源端自检 S8 / 目标端自检 D2
#                 认的同一个指纹）。「有人应答」不算——隧道通到别的服务上也会有人应答。
#                 落点种类与 rehearsal-tunnel-up.sh 的 --sink 要对上，对不上 T3/T5/T7 会红在一个假成因上。
#
# `set -e` **刻意不开**：与四份既有判据脚本同一条纪律 —— 逐条判完再算总账。
set -uo pipefail
cd "$(dirname "$0")/.."

SINK_KIND=stub
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sink) [[ $# -ge 2 ]] || { echo "--sink 要跟 stub|real" >&2; exit 2; }
            SINK_KIND="$2"; shift 2 ;;
    -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "未知参数：$1（只认 --sink stub|real）" >&2; exit 2 ;;
  esac
done
case "$SINK_KIND" in stub|real) ;; *) echo "--sink 只认 stub|real" >&2; exit 2 ;; esac

SRC=qbs-host-source
DST=qbs-host-target
WHITELIST_PORT=15443
SINK_PORT=8080
GW=host.docker.internal
# 与 rehearsal-tunnel-up.sh 里桩 sink 写回的标记必须一致；test-rehearsal-tunnel.sh 静态核对。
MARKER=QBS-TUNNEL-OK
CERT_DIR=/etc/stunnel/db-qbs
# 落点的指纹（T3/T5/T7 要在应答里看到的那个串）与打哪个路径去要它。
case "$SINK_KIND" in
  stub) LANDING=$MARKER;    PROBE_PATH=/ ;;
  real) LANDING=RUN_UNKNOWN; PROBE_PATH=/v1/runs/__tunnel-probe__ ;;
esac

pass=0; fail=0
report() {  # $1=编号 $2=期望 $3=实测 $4=说明
  if [[ "$3" == "$2" ]]; then
    printf '  %-5s PASS  %-58s 实测=%s\n' "$1" "$4" "$3"; pass=$((pass+1))
  else
    printf '  %-5s FAIL  %-58s 期望=%s 实测=%s\n' "$1" "$4" "$2" "$3"; fail=$((fail+1))
  fi
}

# 所有探针「失败也要给得出一个值」——取不到时返回一个明确的词，
# 既不会让判据的失败看起来像脚本跑挂了，也不会被当成「不通」蒙混过关。
docker_line() {
  local out
  out=$(docker "$@" 2>/dev/null | tr -d '\r' | tail -1)
  [[ -n "$out" ]] && printf '%s' "$out" || printf '取不到'
}

running() { docker inspect -f '{{.State.Status}}' "$1" 2>/dev/null | grep -qx running && echo running || echo 缺席; }

# 下面几个探针把 $2/$3 拼进 `bash -c` 的字符串：**实参只许是字面量的主机名/IP 与端口**
# （本脚本传的全是 docker inspect 取来的地址与写死的端口号）。要接外部输入得改成 -e 传参。
alive() {  # $1=容器 -> stunnel 进程在不在（顺带验 pid 文件——真机的 systemd unit 靠它）
  docker_line exec "$1" bash -c \
    'p=$(cat /var/run/db-qbs-stunnel-sink.pid 2>/dev/null); { [ -n "$p" ] && [ -d "/proc/$p" ] && echo 在跑; } || echo 没跑'
}

tcp() {  # $1=容器 $2=主机 $3=端口 -> 通/不通
  docker_line exec "$1" bash -c "timeout 5 bash -c 'exec 3<>/dev/tcp/$2/$3' 2>/dev/null && echo 通 || echo 不通"
}

http_marker() {  # $1=容器 $2=主机 $3=端口 -> 明文 HTTP 的应答里落点的指纹，拿不到给「无」
  # 桩回的是一行标记；真 sink 对不存在的 run 回 404 的 JSON、里面带 RUN_UNKNOWN。
  # 两种都只认指纹本身：`curl -s` 不带 -f，404 的正文照样取得到。
  docker_line exec "$1" bash -c \
    "o=\$(curl -s --max-time 8 http://$2:$3$PROBE_PATH 2>/dev/null | tr -d '\r' | grep -o -m1 '$LANDING'); echo \${o:-无}"
}

wire_probe() {  # $1=容器 $2=主机 $3=端口 -> 「判定|首8字节十六进制」
  # 直接对着白名单口发一行明文 HTTP，把对端**回过来的头几个字节**原样取出来。
  # TLS 服务端收到垃圾会回一条 alert 记录（0x15 0x03 …）或者干脆闭嘴；
  # 若这里回的是 `48 54 54 50`（"HTTP"），那这一跳过的就是明文，本票的判据当场不成立。
  # 「闭嘴」也判非明文 —— 它的假绿成因（那儿压根没人听）由同址正对照 T7 排掉。
  docker_line exec "$1" bash -c "
    h=\$(timeout 8 bash -c 'exec 3>&- 2>/dev/null
      exec 3<>/dev/tcp/$2/$3 2>/dev/null || exit 1
      printf \"GET / HTTP/1.0\r\n\r\n\" >&3
      head -c 8 <&3 | od -An -tx1 | tr -s \" \" | tr -d \"\n\"' ) || { echo '连不上|-'; exit 0; }
    h=\$(echo \$h)
    case \"\$h\" in
      '48 54 54 50'*) echo \"明文HTTP|\${h:-空}\" ;;
      *)              echo \"非明文|\${h:-空（对端未回字节）}\" ;;
    esac"
}

tls_marker() {  # $1=容器 $2=主机 $3=端口 $4=带证书(1)/不带(0) -> 应用层应答里落点的指纹，拿不到给「无」
  local certargs=""
  (( $4 )) && certargs="-cert $CERT_DIR/source.crt -key $CERT_DIR/source.key"
  docker_line exec "$1" bash -c "
    o=\$(printf 'GET $PROBE_PATH HTTP/1.0\r\n\r\n' | timeout 12 openssl s_client -connect $2:$3 \
          -CAfile $CERT_DIR/target.crt $certargs -quiet 2>/dev/null | tr -d '\r' | grep -o -m1 '$LANDING')
    echo \${o:-无}"
}

tls_proto() {  # $1=容器 $2=主机 $3=端口 -> 「协议/套件」，取不到给「取不到」
  # T6b 的实测常常是「对端未回字节」——TLS 服务端对着垃圾闭嘴是合规行为，
  # 但那条证据只说明「不是明文 HTTP」，没说明**是什么**。这一条把它补上：
  # 同一个地址上真正协商出来的协议与套件。
  docker_line exec "$1" bash -c "
    o=\$(timeout 12 openssl s_client -connect $2:$3 -CAfile $CERT_DIR/target.crt \
          -cert $CERT_DIR/source.crt -key $CERT_DIR/source.key </dev/null 2>/dev/null \
        | awk -F': *' '/^ *Protocol *:/{p=\$2} /^ *Cipher *:/{c=\$2} END{if(p!=\"\")printf \"%s/%s\", p, c}')
    echo \${o:-取不到}"
}

peer_cn() {  # $1=容器 $2=主机 $3=端口 -> 对端证书的 CN
  docker_line exec "$1" bash -c "
    s=\$(timeout 12 openssl s_client -connect $2:$3 -CAfile $CERT_DIR/target.crt \
          -cert $CERT_DIR/source.crt -key $CERT_DIR/source.key </dev/null 2>/dev/null \
        | grep -m1 '^subject=' | sed 's/.*CN *= *//; s/.*CN=//; s/[ ,\/].*//')
    echo \${s:-取不到}"
}

ip_on() { docker_line inspect -f "{{index .NetworkSettings.Networks \"$2\" \"IPAddress\"}}" "$1"; }

echo "==> 落点：${SINK_KIND} sink（T3/T5/T7 认的指纹=${LANDING}，打 ${PROBE_PATH}）"
echo "==> 前置：两台主机在跑（在此之前一切判据都不成立）"
report T0a running "$(running "$SRC")" "源端主机 $SRC"
report T0b running "$(running "$DST")" "目标端主机 $DST"

echo "==> T1–T2 两端 stunnel 在位（pid 文件也验掉——真机的 systemd unit 靠它守进程）"
report T1 在跑 "$(alive "$SRC")" "源端 stunnel 客户端"
report T2 在跑 "$(alive "$DST")" "目标端 stunnel 服务端"

src_ip=$(ip_on "$SRC" qbs-src-side)
dst_ip=$(ip_on "$DST" qbs-dst-side)
echo "    源端 src-side IP=$src_ip   目标端 dst-side IP=$dst_ip"

echo "==> T3–T4 sink 只绑回环（ADR-0024 的兜底形态原样成立）"
report T3 "$LANDING" "$(http_marker "$DST" 127.0.0.1 "$SINK_PORT")" \
  "目标端自连 127.0.0.1:${SINK_PORT}（回环上的服务活着）"
report T4 不通 "$(tcp "$DST" "$dst_ip" "$SINK_PORT")" \
  "目标端经自己的 ${dst_ip}:${SINK_PORT}（回环之外摸不到 sink）"

echo "==> T5 主判据：源端经本机隧道口一跳到达目标端回环上的服务"
report T5 "$LANDING" "$(http_marker "$SRC" 127.0.0.1 "$SINK_PORT")" \
  "源端 curl http://127.0.0.1:${SINK_PORT}${PROBE_PATH}（product 的 sink_base_url 原样）"

echo "==> T6–T8 「公网」那一跳走的是加密流量，且认人"
wire=$(wire_probe "$SRC" "$GW" "$WHITELIST_PORT")
report T6  无 "$(http_marker "$SRC" "$GW" "$WHITELIST_PORT")" \
  "源端明文 HTTP 打宿主:${WHITELIST_PORT}（拿不到东西）"
report T6b 非明文 "${wire%%|*}" \
  "同一跳的对端首字节实测=${wire#*|}"
report T7  "$LANDING" "$(tls_marker "$SRC" "$GW" "$WHITELIST_PORT" 1)" \
  "源端带客户端证书握手同一地址（T6/T6b/T8 的正对照）"
report T7b db-qbs-target "$(peer_cn "$SRC" "$GW" "$WHITELIST_PORT")" \
  "对端证书 CN（钉的是这一张，不是任何公共 CA）"
proto=$(tls_proto "$SRC" "$GW" "$WHITELIST_PORT")
report T7c TLSv1.2 "${proto%%/*}" \
  "协商出来的协议（套件=${proto#*/}）"
report T8  无 "$(tls_marker "$SRC" "$GW" "$WHITELIST_PORT" 0)" \
  "源端不带客户端证书握手（verify=2 双向认证在生效）"

echo "==> T9–T11 露在外面的只有白名单那一个口"
report T9 "$WHITELIST_PORT/tcp" \
  "$(docker port "$DST" 2>/dev/null | sed 's/ .*//' | sort -u | paste -sd, - | sed 's/^$/取不到/')" \
  "目标端容器对外发布的端口全集"
report T10a 通 "$(tcp "$DST" "$dst_ip" "$WHITELIST_PORT")" \
  "目标端经自己的 ${dst_ip}:${WHITELIST_PORT} 自连（T10 的正对照）"
report T10  不通 "$(tcp "$SRC" "$dst_ip" "$WHITELIST_PORT")" \
  "源端按 IP 直达 ${dst_ip}:${WHITELIST_PORT}（跨容器直达仍被切断）"
report T11  不通 "$(tcp "$SRC" "$src_ip" "$SINK_PORT")" \
  "源端经自己的 ${src_ip}:${SINK_PORT}（隧道入口只绑回环）"

echo
echo "==== 隧道判据（落点=${SINK_KIND} sink）：PASS=$pass FAIL=$fail ===="
(( fail == 0 ))
