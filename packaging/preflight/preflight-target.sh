#!/usr/bin/env bash
# 目标端环境自检（#154 / 规格 #149 D.13–D.15）——**上机第一件事跑它**。
#
# 三条纪律与源端那支 `preflight-source.sh` 一字不差：一次列全（`set -e` 刻意不开）、
# 每条 FAIL 带一行处置、前提没满足的条目照样留在清单里。两支脚本**刻意各自独立、不抽公共库**：
# 它们是分别 scp 到两台机器上的单文件，共享库会变成「少带了一个文件」这种现场故障。
#
# 依赖面：**只用 bash 4.2 + coreutils**。这台机器上不装 MySQL 客户端——
# 装了也没用：CentOS 7 base 源里的客户端是 5.x，对 MySQL 8.0 的 caching_sha2_password
# 认不了，红出来的是一条假故障。三项开连接仪式前提改由 **sink 自己**去问：
# `POST /v1/target/test-connection` 走的就是搬运那条链的开连接仪式
# （`crates/sink/src/mysql_destination.rs` 的 `run_connection_ritual`），
# 用的是同一个驱动、同一套会话设置——**自检问到的与搬运时用到的是同一件事**，
# 不是一个近似替身。代价是这几项要等 sink 装上来才判得了；干净机器上它们先红，正是本票要的。
#
# 用法：./preflight-target.sh [--help]
set -uo pipefail

MIN_PACKET=67108864     # 64 MiB —— 与 crates/sink/src/mysql_destination.rs 的 MIN_PACKET 同值
STUNNEL_PIDFILE=${QBS_STUNNEL_PIDFILE:-/var/run/db-qbs-stunnel-sink.pid}
STUNNEL_CONF=${QBS_STUNNEL_CONF:-/etc/stunnel/db-qbs/stunnel-sink.conf}

usage() {
  cat <<'USAGE'
目标端环境自检。上机第一件事跑，缺什么一次列全。

  QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass ./preflight-target.sh

读得到的配置（环境变量优先，其次从 sink.toml 里读）：
  QBS_SINK_CONFIG          sink.toml 路径；默认依次找
                           /etc/db-qbs/sink.toml、/opt/db-qbs/sink.toml、./sink.toml
  QBS_SINK_LISTEN          sink 的监听地址，默认取 sink.toml 的 listen，再默认 127.0.0.1:8080
  QBS_MYSQL_HOST/PORT      目标库地址，默认 127.0.0.1:3306
  QBS_MYSQL_USER           目标库账号
  QBS_MYSQL_PASSWORD       目标库口令（**优先用 QBS_MYSQL_PASSWORD_FILE**，别落进 shell 历史）
  QBS_MYSQL_PASSWORD_FILE  存着口令的文件，读第一行
  QBS_MYSQL_DATABASE       目标库库名
  QBS_HOST_IP              本机的非回环地址（判「sink 没越出回环」用），默认自动取
  QBS_STUNNEL_PIDFILE      stunnel 服务端 pid 文件，默认 /var/run/db-qbs-stunnel-sink.pid
  QBS_STUNNEL_CONF         stunnel 服务端配置，默认 /etc/stunnel/db-qbs/stunnel-sink.conf

退出码：全绿 0，有 FAIL 1。
USAGE
}
[[ "${1:-}" == "--help" || "${1:-}" == "-h" ]] && { usage; exit 0; }

# ---------------------------------------------------------------- 报表
pass=0; fail=0
report() {  # $1=编号 $2=PASS/FAIL $3=说明 $4=实测 $5=处置（FAIL 才用得上）
  if [[ "$2" == PASS ]]; then
    printf '  %-4s PASS  %-46s 实测=%s\n' "$1" "$3" "$4"; pass=$((pass+1))
  else
    printf '  %-4s FAIL  %-46s 实测=%s\n' "$1" "$3" "$4"; fail=$((fail+1))
    printf '       └ 处置：%s\n' "$5"
  fi
}

# ---------------------------------------------------------------- 探针
# 地址、端口与请求体一律经环境变量传给内层 bash，不拼进 `bash -c` 的字面量：
# 这些值来自配置文件与口令文件，拼字符串等于把它们变成一条可执行的命令。
tcp_open() {  # $1=主机 $2=端口 -> 通/不通
  QH=$1 QP=$2 timeout 6 bash -c 'exec 3<>/dev/tcp/$QH/$QP' >/dev/null 2>&1 \
    && echo 通 || echo 不通
}

http_get() {  # $1=主机 $2=端口 $3=路径 -> 整个响应；拿不到就是空串
  QH=$1 QP=$2 QQ=$3 timeout 10 bash -c '
    exec 3<>/dev/tcp/$QH/$QP || exit 1
    printf "GET %s HTTP/1.0\r\nHost: %s:%s\r\nConnection: close\r\n\r\n" "$QQ" "$QH" "$QP" >&3
    cat <&3' 2>/dev/null
}

http_post_json() {  # $1=主机 $2=端口 $3=路径 $4=请求体 -> 整个响应；拿不到就是空串
  # Content-Length 按**字节**算：库名与口令可能是非 ASCII，`${#s}` 数的是字符，会短。
  QH=$1 QP=$2 QQ=$3 QB=$4 QL=$(printf '%s' "$4" | wc -c) \
  timeout 20 bash -c '
    exec 3<>/dev/tcp/$QH/$QP || exit 1
    printf "POST %s HTTP/1.0\r\nHost: %s:%s\r\nContent-Type: application/json\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s" \
      "$QQ" "$QH" "$QP" "$QL" "$QB" >&3
    cat <&3' 2>/dev/null
}

json_escape() {  # 反斜杠与双引号，够用于连接参数；控制字符不在合法取值里
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# ---------------------------------------------------------------- 取配置
config=${QBS_SINK_CONFIG:-}
if [[ -z "$config" ]]; then
  for candidate in /etc/db-qbs/sink.toml /opt/db-qbs/sink.toml ./sink.toml; do
    [[ -f "$candidate" ]] && { config=$candidate; break; }
  done
fi
toml_string() {  # 只认 `键 = "值"`——sink.toml.example 就是这个形状
  [[ -f "$config" ]] || return 0
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\\(.*\\)\"[[:space:]]*$/\\1/p" "$config" | head -1
}

if [[ -n "$config" && -f "$config" ]]; then
  echo "==> 读到 sink.toml：$config"
else
  echo "==> 没找到 sink.toml（还没装到这一步是正常的）；缺的值走环境变量与默认值"
  config=""
fi

listen=${QBS_SINK_LISTEN:-$(toml_string listen)}
listen=${listen:-127.0.0.1:8080}
sink_host=${listen%:*}
sink_port=${listen##*:}

mysql_host=${QBS_MYSQL_HOST:-127.0.0.1}
mysql_port=${QBS_MYSQL_PORT:-3306}
mysql_user=${QBS_MYSQL_USER:-}
mysql_database=${QBS_MYSQL_DATABASE:-}
mysql_password=${QBS_MYSQL_PASSWORD:-}
if [[ -n "${QBS_MYSQL_PASSWORD_FILE:-}" && -r "${QBS_MYSQL_PASSWORD_FILE}" ]]; then
  mysql_password=$(head -1 "$QBS_MYSQL_PASSWORD_FILE")
fi

# 本机的非回环地址：只用 glibc 的 getent，不指望 iproute / net-tools 装着
host_ip=${QBS_HOST_IP:-}
if [[ -z "$host_ip" ]]; then
  host_ip=$(getent ahostsv4 "$(hostname)" 2>/dev/null | awk '$1 !~ /^127\./ {print $1; exit}')
fi

echo "    sink=$sink_host:$sink_port   MySQL=$mysql_host:$mysql_port   库=${mysql_database:-未提供}   本机地址=${host_ip:-取不到}"
echo

# ---------------------------------------------------------------- D1 MySQL 口可达
echo "==> D1 目标库这一跳"
if [[ "$(tcp_open "$mysql_host" "$mysql_port")" == 通 ]]; then
  report D1 PASS "MySQL 监听口 $mysql_host:$mysql_port 可达" 通
else
  report D1 FAIL "MySQL 监听口 $mysql_host:$mysql_port 可达" 不通 \
    "确认 MySQL 起着、bind-address 覆盖得到这台机器、这一跳的防火墙放行"
fi

# ---------------------------------------------------------------- D2–D3 sink 起在回环
echo "==> D2–D3 sink 起在回环（ADR-0024：sink 不做鉴权，靠只绑回环兜底）"
sink_body=""
if [[ "$(tcp_open "$sink_host" "$sink_port")" == 通 ]]; then
  sink_body=$(http_get "$sink_host" "$sink_port" /v1/runs/__preflight__)
fi
if grep -q 'RUN_UNKNOWN' <<<"$sink_body"; then
  report D2 PASS "sink 在 $sink_host:$sink_port 应答" "sink 应答 RUN_UNKNOWN"
elif [[ -n "$sink_body" ]]; then
  report D2 FAIL "sink 在 $sink_host:$sink_port 应答" "应答的不是 sink（首行：$(head -1 <<<"$sink_body" | tr -d '\r')）" \
    "这个口上是别的服务：换 sink.toml 的 listen 端口，或把占着口的服务停掉"
else
  report D2 FAIL "sink 在 $sink_host:$sink_port 应答" 没有应答 \
    "sink 没起：装好后 db-qbs-sink --config $([[ -n "$config" ]] && echo "$config" || echo /etc/db-qbs/sink.toml)，起完再跑本脚本"
fi

# 负判据配同址正对照（与 rehearsal-tunnel-check.sh 同一条纪律）：
# sink 没起的时候「外面摸不到」是真的，但它证不了「只绑回环」——所以那一档不判 PASS。
#
# **先判 listen 本身**：目标端是双网卡（对外白名单口 + 内网 MySQL），
# `listen = "10.0.0.5:8080"` 而 hostname 恰好解析到另一张网卡时，反向探针会「不通」，
# 这一条就为一个绑在可路由地址上的、没有鉴权的 sink 判绿。地址是白纸黑字的，先按它判。
if [[ ! "$sink_host" =~ ^(127\.|localhost$|::1$) ]]; then
  report D3 FAIL "sink 没越出回环" "listen 绑的是 ${sink_host}，不是回环" \
    "把 sink.toml 的 listen 改回 127.0.0.1:${sink_port} —— 它没有鉴权，露到网上等于把目标库交出去（ADR-0024）"
elif ! grep -q 'RUN_UNKNOWN' <<<"$sink_body"; then
  report D3 FAIL "sink 没越出回环" "前提未满足（D2 先红，摸不到不算证据）" "先按 D2 处置"
elif [[ -z "$host_ip" ]]; then
  report D3 FAIL "sink 没越出回环" "取不到本机非回环地址" \
    "把本机对外地址填进 QBS_HOST_IP 后重跑，这一条才判得了"
elif [[ "$(tcp_open "$host_ip" "$sink_port")" == 不通 ]]; then
  report D3 PASS "sink 没越出回环" "$host_ip:$sink_port 不通"
else
  report D3 FAIL "sink 没越出回环" "$host_ip:$sink_port 也连得上" \
    "sink 的 listen 绑到了 0.0.0.0：改回 127.0.0.1:$sink_port —— 它没有鉴权，露到网上等于把目标库交出去"
fi

# ---------------------------------------------------------------- D4–D7 开连接仪式
echo "==> D4–D7 经 sink 开连接仪式（与搬运那条链同一个驱动、同一套会话设置）"
#
# 开连接仪式是**有先后的**（crates/sink/src/mysql_destination.rs 的 run_connection_ritual）：
#   连上 → SET NAMES utf8mb4 → SET SESSION sql_mode → 回读三项会话变量并逐项判
# 卡在第 k 步，第 k+1 步之后的事**根本没发生过**。所以这里按「卡在哪一步」分档，
# 后面那几项一律记「未判定」而不是 PASS —— 记成 PASS 就是自检在替环境作没做过的保证，
# 正是本票要消灭的东西（判据：自检说 OK 之后不该再出现环境类失败）。
#
# stage 取值：ok / connect / charset / sqlmode / readback / settings / blocked
# rank：仪式的步序，与下面每一项的 rank 比大小得出该项的判定。
stage=blocked
detail=""
blocked_hint="先按 D2 处置，sink 起来之后这四条才判得了"
if ! grep -q 'RUN_UNKNOWN' <<<"$sink_body"; then
  detail="sink 没应答（D2 先红）"
elif [[ -z "$mysql_user" || -z "$mysql_database" ]]; then
  detail="QBS_MYSQL_USER / QBS_MYSQL_DATABASE 没给"
  blocked_hint="把目标库的账号、库名（口令走 QBS_MYSQL_PASSWORD_FILE）给全再跑"
else
  body="{\"host\":\"$(json_escape "$mysql_host")\",\"port\":$mysql_port,\"username\":\"$(json_escape "$mysql_user")\",\"password\":\"$(json_escape "$mysql_password")\",\"database\":\"$(json_escape "$mysql_database")\"}"
  answer=$(http_post_json "$sink_host" "$sink_port" /v1/target/test-connection "$body")
  # sink 的错误信息**不回显口令**（TargetConnection 的 Debug 是手写的，口令是 <redacted>）；
  # 这里照样只截 message 那一段，不把整个回答打出去。
  # 冒号后面的空格收着不放过：产品打的是紧凑 JSON，但中间隔一层代理就可能被重排。
  detail=$(sed -n 's/.*"message"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' <<<"$answer" | head -1)
  if grep -qE '"ok"[[:space:]]*:[[:space:]]*true' <<<"$answer"; then
    stage=ok
  elif [[ -z "$answer" ]]; then
    stage=blocked; detail="sink 收下请求后没回话"
  elif grep -q '连接 MySQL 失败' <<<"$answer"; then
    stage=connect
  elif grep -q '设置 utf8mb4 失败' <<<"$answer"; then
    stage=charset
  elif grep -q '设置 sql_mode 失败' <<<"$answer"; then
    stage=sqlmode
  elif grep -q '回读会话变量' <<<"$answer"; then
    stage=readback
  elif grep -q '环境配置错误' <<<"$answer"; then
    stage=settings   # 三项都读回来了，逐项判的结果在 message 里，一次列全
  else
    # 认不出来的回答（协议对不上、前面横着一层代理、版本不一致……）一律记未判定。
    # **不许掉进 settings 那一档**：那一档是「message 里没提到就算合格」，
    # 一个认不出的回答会让三项前提集体假绿。
    stage=unknown
  fi
  [[ -n "$detail" ]] || detail="sink 回了一个读不出 message 的响应"
fi

case "$stage" in
  ok)      report D4 PASS "sink 用给定凭据连得上目标库" 通 ;;
  blocked) report D4 FAIL "sink 用给定凭据连得上目标库" "未判定（${detail}）" "$blocked_hint" ;;
  unknown) report D4 FAIL "sink 用给定凭据连得上目标库" "认不出这个回答（${detail}）" \
             "这一头未必是本版的 sink：核对 sink 的版本，以及中间有没有代理在改响应" ;;
  connect) report D4 FAIL "sink 用给定凭据连得上目标库" "$detail" \
             "账号 / 口令 / 库名 / 授权对不上，或 sink 那台机器到 MySQL 这一跳不通" ;;
  *)       report D4 PASS "sink 用给定凭据连得上目标库" "连上了（卡在开连接仪式，见下）" ;;
esac

stage_rank() {  # 仪式卡在第几步
  case "$1" in charset) echo 1 ;; sqlmode) echo 2 ;; readback) echo 3 ;; *) echo 9 ;; esac
}
ritual_report() {  # $1=编号 $2=说明 $3=本项步序 $4=本项在 message 里的特征（扩展正则）$5=处置
  local rank; rank=$(stage_rank "$stage")
  case "$stage" in
    ok)      report "$1" PASS "$2" 合格 ;;
    blocked) report "$1" FAIL "$2" "未判定（${detail}）" "$blocked_hint" ;;
    unknown|connect)
             report "$1" FAIL "$2" "未判定（D4 先红，仪式没走到这一步）" "先按 D4 处置" ;;
    settings)
      if grep -qE "$4" <<<"$detail"; then report "$1" FAIL "$2" "$detail" "$5"
      else report "$1" PASS "$2" 合格; fi ;;
    readback)
      # **三项一个都没读到值**，所以三项一律未判定。前两步的 `SET` 没报错不算数：
      # 这三条判词说的是「回读回来是这个值」，产品自己也只认回读那一档
      # （run_connection_ritual 先设后读，判定在读回来的值上）。
      # 把「设过了」当成「就是这个值」，正是本票要消灭的那类假保证。
      report "$1" FAIL "$2" "未判定（${detail}）" \
        "sink 连上了却读不回会话变量：看 sink 的 stdout 日志，多半是账号缺 SELECT 权限或连接被中间层截断" ;;
    *)
      if   (( rank < $3 )); then
        report "$1" FAIL "$2" "未判定（仪式卡在前一步：${detail}）" "先清掉前一项，这一项才判得了"
      elif (( rank == $3 )); then
        report "$1" FAIL "$2" "$detail" "$5"
      else
        # 仪式卡在**后面**某一步，这一项的 `SET` 是没报错——但三条判词说的都是
        # 「回读回来是这个值」，而回读根本没发生。「设过了」不等于「就是这个值」
        # （中间层改写会话变量正是产品那道回读要防的事），照样只能记未判定。
        report "$1" FAIL "$2" "未判定（设过了但没读回来：${detail}）" "先清掉后一项，回读跑完这一项才判得了"
      fi ;;
  esac
}

# 三项的步序与 run_connection_ritual 的顺序一一对应：
# 1 = SET NAMES utf8mb4，2 = SET SESSION sql_mode，3 = 回读（三项会话变量一起读回来）。
ritual_report D5 "会话字符集三项都是 utf8mb4" 1 'character_set|utf8mb4' \
  "库或账号的默认字符集不是 utf8mb4：把服务端 character-set-server 设成 utf8mb4 后重启 MySQL"
ritual_report D6 "sql_mode 设得成 STRICT_ALL_TABLES" 2 'sql_mode' \
  "会话设 sql_mode 被拒或被改写：检查 MySQL 的 init_connect、代理层（ProxySQL 之类）有没有在改会话变量"
ritual_report D7 "max_allowed_packet ≥ 64 MiB" 3 'max_allowed_packet' \
  "把 my.cnf 的 max_allowed_packet 调到至少 ${MIN_PACKET} 字节（64M）后重启 MySQL；这是环境配置，不是业务数据问题"

# ---------------------------------------------------------------- D8–D9 stunnel 服务端
echo "==> D8–D9 stunnel 服务端（公网上露出来的只有这一个口）"
stunnel_pid=$(cat "$STUNNEL_PIDFILE" 2>/dev/null)
if [[ -n "$stunnel_pid" && -d "/proc/$stunnel_pid" ]]; then
  report D8 PASS "stunnel 服务端进程在跑" "pid=$stunnel_pid"
else
  leftover=""
  [[ -f "$STUNNEL_CONF" ]] \
    && leftover=$(grep -vE '^[[:space:]]*;' "$STUNNEL_CONF" | grep -oE '@@[A-Z_]+@@' | sort -u | paste -sd, -)
  if [[ ! -f "$STUNNEL_CONF" ]]; then
    hint="配置还没铺：照 packaging/stunnel/README.md 把 target-side/ 那套装到 $STUNNEL_CONF"
  elif [[ -n "$leftover" ]]; then
    hint="配置里还留着占位符（${leftover}），填完再起 stunnel"
  else
    hint="配置在位但进程没起：systemctl start db-qbs-stunnel（或直接 stunnel ${STUNNEL_CONF}），日志看 /var/log/db-qbs-stunnel-sink.log"
  fi
  report D8 FAIL "stunnel 服务端进程在跑" "$STUNNEL_PIDFILE 指不到活进程" "$hint"
fi

# 白名单口从配置里读，不写死：真机上那个端口由客户给，写死的话现场改一处就漏一处。
whitelist_port=""
[[ -f "$STUNNEL_CONF" ]] \
  && whitelist_port=$(sed -n 's/^[[:space:]]*accept[[:space:]]*=[[:space:]]*[^:]*:\([0-9]\+\).*/\1/p' "$STUNNEL_CONF" | head -1)
if [[ -z "$whitelist_port" ]]; then
  report D9 FAIL "白名单口在听" "$STUNNEL_CONF 里读不到 accept 端口" "先按 D8 处置（配置铺好、占位符填完）"
elif [[ "$(tcp_open 127.0.0.1 "$whitelist_port")" == 通 ]]; then
  report D9 PASS "白名单口 $whitelist_port 在听" 通
else
  report D9 FAIL "白名单口 $whitelist_port 在听" 不通 \
    "stunnel 服务端没 bind 上：看 /var/log/db-qbs-stunnel-sink.log，多半是端口被占或证书路径不对"
fi

echo
echo "==== 目标端自检：PASS=$pass FAIL=$fail ===="
if (( fail )); then
  echo "上面每条 FAIL 各带一行处置，逐条清完再重跑本脚本；不要装到一半再来。"
  exit 1
fi
echo "目标端环境齐了。"
