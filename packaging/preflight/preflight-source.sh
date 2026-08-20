#!/usr/bin/env bash
# 源端环境自检（#154 / 规格 #149 D.13–D.15）——**上机第一件事跑它**。
#
# 判据是「自检说 OK 之后，现场不该再出现环境类失败」。所以这支脚本有三条纪律：
#
#   1. **一次列全，不撞到第一个就停**。`set -e` 刻意不开：每一条都判、都打印，
#      最后才算总账。装机现场最贵的是往返——「装到一半炸出下一个缺口」是本票要消灭的东西。
#   2. **每条 FAIL 都带一行「处置」**。自检不是报警器，是待办清单。
#   3. **前提没满足的条目照样出现在清单里**（记 FAIL，说明写「前提未满足」），
#      不许因为上一条红了就把后面几条整段吞掉——吞掉的那几条就是下一次往返。
#
# 依赖面：**只用 bash 4.2 + coreutils + glibc 自带的 ldd/getconf**。
# 干净的 CentOS 7 上不装任何东西就跑得起来（curl / nc / ip / mysql 一概不用，
# HTTP 与 TCP 都走 bash 的 /dev/tcp）——它要在「什么都还没装」的机器上先红一次。
#
# 与目标端那支 `preflight-target.sh` **刻意各自独立、不抽公共库**：
# 两支脚本是分别 scp 到两台机器上的单文件，共享库会变成「少带了一个文件」这种现场故障。
#
# 用法：./preflight-source.sh [--help]
# 取值优先级：环境变量 > source.toml 里读到的值 > 内置默认值。
set -uo pipefail

MIN_GLIBC=2.17          # ADR-0041 / #151：客户机是 CentOS 7，这是硬下界
STUNNEL_PIDFILE=${QBS_STUNNEL_PIDFILE:-/var/run/db-qbs-stunnel-sink.pid}
STUNNEL_CONF=${QBS_STUNNEL_CONF:-/etc/stunnel/db-qbs/stunnel-sink.conf}

usage() {
  cat <<'USAGE'
源端环境自检。上机第一件事跑，缺什么一次列全。

  ./preflight-source.sh

读得到的配置（环境变量优先，其次从 source.toml 里读）：
  QBS_SOURCE_CONFIG          source.toml 路径；默认依次找
                             /etc/db-qbs/source.toml、/opt/db-qbs/source.toml、./source.toml
  QBS_ORACLE_CLIENT_LIB_DIR  Instant Client 目录（source.toml 的 oracle_client_lib_dir）
  QBS_SINK_BASE_URL          source 眼里的 sink 地址（source.toml 的 sink_base_url）；
                             隧道形态下它指向**本机**的隧道入口口
  QBS_ORACLE_HOST            Oracle 监听地址。默认从 source.toml 的 oracle_connect_string 里拆，
                             但那个字段已退役（ADR-0037 §10），多半是空的——空的时候
                             S5 记「未判定」，**不猜 127.0.0.1**
  QBS_ORACLE_PORT            Oracle 监听端口，默认 1521
  QBS_STUNNEL_PIDFILE        stunnel 客户端 pid 文件，默认 /var/run/db-qbs-stunnel-sink.pid
  QBS_STUNNEL_CONF           stunnel 客户端配置，默认 /etc/stunnel/db-qbs/stunnel-sink.conf

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
# 地址与端口一律经环境变量传给内层 bash，不拼进 `bash -c` 的字面量：
# 这些值来自配置文件，拼字符串等于把配置文件变成一条可执行的命令。
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

# ---------------------------------------------------------------- 取配置
config=${QBS_SOURCE_CONFIG:-}
if [[ -z "$config" ]]; then
  for candidate in /etc/db-qbs/source.toml /opt/db-qbs/source.toml ./source.toml; do
    [[ -f "$candidate" ]] && { config=$candidate; break; }
  done
fi
# TOML 里取一个顶层字符串键。只认 `键 = "值"` 这一种写法——source.toml.example 就是这个形状，
# 自检不该为了解析一门配置语言而引入一个解析器。
toml_string() {  # $1=键名
  [[ -f "$config" ]] || return 0
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\\(.*\\)\"[[:space:]]*$/\\1/p" "$config" | head -1
}

if [[ -n "$config" && -f "$config" ]]; then
  echo "==> 读到 source.toml：$config"
else
  echo "==> 没找到 source.toml（还没装到这一步是正常的）；缺的值走环境变量与默认值"
  config=""
fi

client_dir=${QBS_ORACLE_CLIENT_LIB_DIR:-$(toml_string oracle_client_lib_dir)}
client_dir=${client_dir:-/opt/oracle/instantclient}
sink_url=${QBS_SINK_BASE_URL:-$(toml_string sink_base_url)}
sink_url=${sink_url:-http://127.0.0.1:8080}
# `//host:port/SERVICE` 是 Easy Connect 的写法。**source.toml 的 oracle_connect_string
# 已按 ADR-0037 §10 退役**（迁完就该删掉），真相源是数据源库里那条连接串，自检读不到
# （口令加密落盘、要解密）。所以这个字段经常是空的 —— 空的时候**不许悄悄退回 127.0.0.1**：
# 源端本机恰好有个 1521 在听，S5 就会为一个与产品无关的地址判绿，正是本票要消灭的假绿。
connect_string=$(toml_string oracle_connect_string)
oracle_host=${QBS_ORACLE_HOST:-$(sed -n 's|^//\([^:/]*\).*|\1|p' <<<"$connect_string")}
oracle_port=${QBS_ORACLE_PORT:-$(sed -n 's|^//[^:/]*:\([0-9]*\).*|\1|p' <<<"$connect_string")}
oracle_port=${oracle_port:-1521}

# sink_base_url 拆成主机与端口。产品只收 http（crates/source/src/protocol.rs），这里同判。
sink_scheme=${sink_url%%://*}
sink_authority=${sink_url#*://}
sink_authority=${sink_authority%%/*}
sink_host=${sink_authority%%:*}
sink_port=${sink_authority##*:}
[[ "$sink_port" == "$sink_host" ]] && sink_port=80

echo "    Instant Client=$client_dir   Oracle=${oracle_host:-未知}:$oracle_port   sink=$sink_url"
echo

# ---------------------------------------------------------------- S1 glibc
echo "==> S1 glibc 版本（客户机是 CentOS 7，二进制按 glibc $MIN_GLIBC 编）"
glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
[[ -z "$glibc" ]] && glibc=$(ldd --version 2>/dev/null | head -1 | awk '{print $NF}')
if [[ -z "$glibc" ]]; then
  report S1 FAIL "glibc 版本 ≥ $MIN_GLIBC" 取不到 \
    "getconf 与 ldd 都问不出版本，这台机器的 glibc 工具链不完整；先 yum reinstall glibc-common"
elif [[ "$(printf '%s\n%s\n' "$MIN_GLIBC" "$glibc" | sort -V | head -1)" == "$MIN_GLIBC" ]]; then
  report S1 PASS "glibc 版本 ≥ $MIN_GLIBC" "$glibc"
else
  report S1 FAIL "glibc 版本 ≥ $MIN_GLIBC" "$glibc" \
    "这台机器比 CentOS 7 还老，两个二进制启动即 GLIBC_2.xx not found；换机器或回 packaging/centos7 重编"
fi

# ---------------------------------------------------------------- S2–S4 Instant Client
echo "==> S2–S4 Instant Client（ODPI-C 是运行时 dlopen 它的，缺什么只在连库那一刻才炸）"
libclntsh=""
dangling=""
if [[ -d "$client_dir" ]]; then
  # 19c 的实际文件名是 libclntsh.so.19.1，libclntsh.so 是软链；两种都认。
  # **软链必须解得开**：手工建的 libclntsh.so 指错是这台机器上最常见的手滑，
  # 而 `ls` 照样列出悬空软链——只判「名字在」的话，S2 会为一个 dlopen 不了的库判绿。
  for candidate in "$client_dir"/libclntsh.so*; do
    if [[ -e "$candidate" && -r "$candidate" ]]; then libclntsh=$candidate; break; fi
    [[ -L "$candidate" || -f "$candidate" ]] && dangling=$candidate
  done
fi
if [[ -n "$libclntsh" ]]; then
  report S2 PASS "Instant Client 目录里有 libclntsh.so 且读得到" "$libclntsh"
elif [[ -n "$dangling" ]]; then
  report S2 FAIL "Instant Client 目录里有 libclntsh.so 且读得到" "$dangling 解不开或读不了" \
    "软链指空了或权限不对：ls -l ${client_dir}/libclntsh.so* 看一眼，重新指到实际的 libclntsh.so.19.1"
else
  report S2 FAIL "Instant Client 目录里有 libclntsh.so 且读得到" "$client_dir 里没有" \
    "把 Instant Client 19c Basic 包解到 ${client_dir}（行李清单里带着），并让 source.toml 的 oracle_client_lib_dir 指向它"
fi

# 架构对不对——带错架构的包解出来一样有 libclntsh.so，报的却是 dlopen 失败。
# 只用 od 读 ELF 头：e_ident[4]=类，e_machine（第 18–19 字节，小端）=架构。
machine=$(uname -m)
if [[ -n "$libclntsh" ]]; then
  hdr=$(od -An -tx1 -N20 "$libclntsh" 2>/dev/null | tr -s ' ' | tr -d '\n')
  read -r -a b <<<"$hdr"
  elf_machine="${b[19]:-}${b[18]:-}"   # 小端：高位在后，拼回 0x…
  case "$elf_machine" in
    003e) lib_arch=x86_64 ;;
    00b7) lib_arch=aarch64 ;;
    *)    lib_arch="未知(0x$elf_machine)" ;;
  esac
  if [[ "$lib_arch" == "$machine" ]]; then
    report S3 PASS "Instant Client 架构与本机一致" "$lib_arch"
  else
    report S3 FAIL "Instant Client 架构与本机一致" "库=$lib_arch 本机=$machine" \
      "带错架构了：重新下 $machine 版的 Instant Client 19c Basic"
  fi
else
  report S3 FAIL "Instant Client 架构与本机一致" "前提未满足（S2 先红）" "先按 S2 处置"
fi

# 动态依赖解析得开——两条最常见的：libaio.so.1 缺失（yum install libaio），
# 以及**同一个包里的 libnnz19.so 找不到**（Instant Client 目录没进 ldconfig）。
#
# **按产品自己的搜索路径判，别替它加 LD_LIBRARY_PATH。** ODPI-C 按全路径 dlopen
# libclntsh.so，其余几个兄弟库（libnnz19 / libclntshcore）由动态链接器按**它自己的**
# 搜索路径找——`ldconfig` 里没有那个目录时就找不到。自检若在查之前先把目录塞进
# LD_LIBRARY_PATH，查的就不是产品会遇到的那件事：2026-08-20 的源端装机演练上，
# S1–S8 全绿之后「测试连接」当场 `DPI-1047 ... libnnz19.so: cannot open shared object file`
# ——「自检说 OK 之后不该再出现环境类失败」那条判据当场破了（#155）。
if [[ -n "$libclntsh" ]]; then
  # `ldd` 的**退出码也要收**：它跑失败时 stdout 是空的，只看「有没有 not found」
  # 会把「压根没查成」判成「依赖都齐了」——那是给运维的一句假保证。
  #
  # **`env -u LD_LIBRARY_PATH`，不只是「本脚本不加」**：把
  # `export LD_LIBRARY_PATH=/opt/oracle/instantclient` 写进 root 的 profile 是这类机器上最常见的
  # 习惯，而 systemd 拉起来的 `db-qbs-source` 不继承任何 profile。留着继承来的那一份，
  # 自检查到的是「运维当前这个 shell 里能不能加载」，产品要的是「服务进程里能不能加载」。
  ldd_out=$(env -u LD_LIBRARY_PATH ldd "$libclntsh" 2>&1); ldd_rc=$?
  missing=$(awk '/not found/{print $1}' <<<"$ldd_out" | sort -u)
  # 缺的那几个里，哪些是「把 Instant Client 目录加进搜索路径就有了」——那部分的成因不是没装包，
  # 而是那个目录没进 ldconfig。两种成因的处置完全不同，**而且可能同时成立**：
  # 合成一句话就等于让运维清完一条再撞下一条，正是脚本头那条「一次列全」禁止的往返。
  with_dir_missing=""
  with_dir_rc=0
  if [[ -n "$missing" ]]; then
    with_dir_out=$(env LD_LIBRARY_PATH="$client_dir" ldd "$libclntsh" 2>&1); with_dir_rc=$?
    with_dir_missing=$(awk '/not found/{print $1}' <<<"$with_dir_out" | sort -u)
  fi
  # 「加上目录就有了」的那一批 = 两次结果之差。
  ldconfig_only=$(comm -23 <(printf '%s\n' $missing) <(printf '%s\n' $with_dir_missing) | paste -sd, -)
  still_missing=$(printf '%s' "$with_dir_missing" | paste -sd, -)
  all_missing=$(printf '%s' "$missing" | paste -sd, -)
  fix_ldconfig="Instant Client 目录没进动态链接器的搜索路径：echo $client_dir > /etc/ld.so.conf.d/oracle-instantclient.conf && ldconfig"
  fix_install="装上缺的库（CentOS 7 上多半是 yum install libaio），或把它们所在目录写进 ldconfig"
  if (( ldd_rc != 0 )) && [[ -z "$all_missing" ]]; then
    report S4 FAIL "Instant Client 的动态依赖全解析得开" "未判定（ldd 以 ${ldd_rc} 退出）" \
      "ldd 没查成，多半不是一个动态库或读不了：$(head -1 <<<"$ldd_out")"
  elif [[ -z "$all_missing" ]]; then
    report S4 PASS "Instant Client 的动态依赖全解析得开" 无缺失
  elif (( with_dir_rc != 0 )); then
    # 第二趟没查成，就分不出这几个缺的是哪种成因 —— **别猜**，两条处置一起给。
    report S4 FAIL "Instant Client 的动态依赖全解析得开" "缺=${all_missing}（成因未判定，第二趟 ldd 以 ${with_dir_rc} 退出）" \
      "两种成因都要排：${fix_install}；以及 ${fix_ldconfig}"
  elif [[ -z "$still_missing" ]]; then
    report S4 FAIL "Instant Client 的动态依赖全解析得开" "缺=${ldconfig_only}（加上 ${client_dir} 就都在）" \
      "$fix_ldconfig"
  elif [[ -z "$ldconfig_only" ]]; then
    report S4 FAIL "Instant Client 的动态依赖全解析得开" "缺=${still_missing}" "$fix_install"
  else
    # 两种成因同时成立 —— 一次列全，别让人清完一条再来撞下一条。
    report S4 FAIL "Instant Client 的动态依赖全解析得开" "缺=${still_missing}；另有 ${ldconfig_only} 加上 ${client_dir} 就都在" \
      "两条都要做：${fix_install}；以及 ${fix_ldconfig}"
  fi
else
  report S4 FAIL "Instant Client 的动态依赖全解析得开" "前提未满足（S2 先红）" "先按 S2 处置"
fi

# ---------------------------------------------------------------- S5 Oracle
echo "==> S5 Oracle 连通"
if [[ -z "$oracle_host" ]]; then
  report S5 FAIL "Oracle 监听口可达" "未判定（不知道 Oracle 在哪）" \
    "把地址给进 QBS_ORACLE_HOST（值取数据源界面上那条连接串里的主机）后重跑；source.toml 的 oracle_connect_string 已退役，读不到是正常的"
else
  oracle_tcp=$(tcp_open "$oracle_host" "$oracle_port")
  if [[ "$oracle_tcp" == 通 ]]; then
    report S5 PASS "Oracle 监听口 ${oracle_host}:${oracle_port} 可达" 通
  else
    report S5 FAIL "Oracle 监听口 ${oracle_host}:${oracle_port} 可达" 不通 \
      "确认 Oracle 起着、监听地址与端口对得上、源端到它这一跳的防火墙放行；地址从哪来见 --help 的 QBS_ORACLE_HOST"
  fi
fi
# 口令与服务名对不对，这支脚本证不了（Basic 包不带 sqlplus，本票也不给客户机加装它）。
# 那一档由界面的「测试连接」证——它走的是产品自己的 Oracle 连接路径（ADR-0037 §9）。
echo "       注：账号 / 口令 / 服务名对不对不在本项内，装完 source 后用界面的「测试连接」证一次"

# ---------------------------------------------------------------- S6–S8 隧道
echo "==> S6–S8 隧道（stunnel 客户端 → 目标端的 sink）"
stunnel_pid=$(cat "$STUNNEL_PIDFILE" 2>/dev/null)
if [[ -n "$stunnel_pid" && -d "/proc/$stunnel_pid" ]]; then
  report S6 PASS "stunnel 客户端进程在跑" "pid=$stunnel_pid"
else
  leftover=""
  [[ -f "$STUNNEL_CONF" ]] \
    && leftover=$(grep -vE '^[[:space:]]*;' "$STUNNEL_CONF" | grep -oE '@@[A-Z_]+@@' | sort -u | paste -sd, -)
  if [[ ! -f "$STUNNEL_CONF" ]]; then
    hint="配置还没铺：照 packaging/stunnel/README.md 把 source-side/ 那套装到 $STUNNEL_CONF"
  elif [[ -n "$leftover" ]]; then
    hint="配置里还留着占位符（${leftover}），填完再起 stunnel"
  else
    hint="配置在位但进程没起：systemctl start db-qbs-stunnel（或直接 stunnel ${STUNNEL_CONF}），日志看 /var/log/db-qbs-stunnel-sink.log"
  fi
  report S6 FAIL "stunnel 客户端进程在跑" "$STUNNEL_PIDFILE 指不到活进程" "$hint"
fi

if [[ "$sink_scheme" != http ]]; then
  report S7 FAIL "隧道入口 $sink_host:$sink_port 在听" "sink_base_url 的 scheme 是 $sink_scheme" \
    "产品只收 http 的 sink_base_url（明文只在回环上走，出机器之前已进 stunnel 的 TLS）；改回 http://"
  report S8 FAIL "经隧道摸得到目标端的 sink" "前提未满足（S7 先红）" "先按 S7 处置"
else
  tunnel_tcp=$(tcp_open "$sink_host" "$sink_port")
  if [[ "$tunnel_tcp" == 通 ]]; then
    report S7 PASS "隧道入口 $sink_host:$sink_port 在听" 通
  else
    report S7 FAIL "隧道入口 $sink_host:$sink_port 在听" 不通 \
      "stunnel 客户端的 accept 口没起来或端口对不上；核对 $STUNNEL_CONF 的 accept 与 source.toml 的 sink_base_url"
  fi

  # 摸的是 sink 自己的一个只读端点：回的必须是 sink 的错误信封（code=RUN_UNKNOWN）。
  # 只判「有 HTTP 应答」不够——隧道配歪了照样有人应答，回的却是另一个服务。
  if [[ "$tunnel_tcp" == 通 ]]; then
    body=$(http_get "$sink_host" "$sink_port" /v1/runs/__preflight__)
    if grep -q 'RUN_UNKNOWN' <<<"$body"; then
      report S8 PASS "经隧道摸得到目标端的 sink" "sink 应答 RUN_UNKNOWN"
    elif [[ -n "$body" ]]; then
      report S8 FAIL "经隧道摸得到目标端的 sink" "应答不是 sink（首行：$(head -1 <<<"$body" | tr -d '\r')）" \
        "隧道通到了别的服务：核对 $STUNNEL_CONF 的 connect 与目标端 accept 口、目标端 stunnel 的 connect 是不是落在 sink 的回环口上"
    else
      report S8 FAIL "经隧道摸得到目标端的 sink" "隧道口连得上但没有应答" \
        "多半是目标端那一头没通：先在目标端跑 preflight-target.sh（sink 与 stunnel 服务端是否都在位）"
    fi
  else
    report S8 FAIL "经隧道摸得到目标端的 sink" "前提未满足（S7 先红）" "先按 S7 处置"
  fi
fi

echo
echo "==== 源端自检：PASS=$pass FAIL=$fail ===="
if (( fail )); then
  echo "上面每条 FAIL 各带一行处置，逐条清完再重跑本脚本；不要装到一半再来。"
  exit 1
fi
echo "源端环境齐了。"
