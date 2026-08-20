#!/usr/bin/env bash
# #154 —— 两支自检脚本与它们那支演练台判据的静态自检。与 test-rehearsal-tunnel.sh 同一职责：
# 不起台架、不碰 docker，只守住那些「跑起来才发现就太晚」的结构性约定。
#
# 本票的判据 3（检查项覆盖规格 #149 D.14 的两端清单）与判据 4（两个脚本进仓库）
# **都是静态事实**，判它们不需要台架，也不该等到实跑那一刻才知道 —— 所以判在这里。
#
# 这里还守着一条别处守不住的东西：自检有两处**跨文件耦合**——
# 目标端那三项前提是问 sink 要的（按它的报错措辞分档），sink 的指纹是它 404 的错误码。
# 产品那边改一个词，自检就会静默地把红判成绿。第 6/7 条按内容盯着那几处。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"

SRC_SH="$ROOT/packaging/preflight/preflight-source.sh"
DST_SH="$ROOT/packaging/preflight/preflight-target.sh"
check=./scripts/rehearsal-preflight-check.sh
classify=./scripts/test-preflight-classify.sh
stub=./scripts/preflight-sink-stub.py
PACKING="$ROOT/packaging/PACKING-LIST.md"

# 1. 三支脚本语法过得去、两支自检可执行、都进了版本库（判据 4）。
for s in "$SRC_SH" "$DST_SH" "$check" "$classify"; do bash -n "$s"; done
python3 -c 'import ast,sys; ast.parse(open(sys.argv[1]).read())' "$stub"
for s in "$SRC_SH" "$DST_SH"; do
  [[ -x "$s" ]] || { echo "自检脚本没有可执行位：$s" >&2; exit 1; }
done
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  for s in packaging/preflight/preflight-source.sh packaging/preflight/preflight-target.sh; do
    git -C "$ROOT" ls-files --error-unmatch "$s" >/dev/null 2>&1 \
      || { echo "自检脚本没进版本库：${s}（票面判据 4）" >&2; exit 1; }
  done
fi

# 2. 「一次列全」是本票的核心判据，而 `set -e` 会让它当场作废——撞到第一个失败就退，
#    后面的缺项一条都不打印。两支脚本都不许开。
for s in "$SRC_SH" "$DST_SH"; do
  grep -qx 'set -uo pipefail' "$s" || { echo "$s 的 set 行变了（开了 set -e 就会把「一次列全」作废）" >&2; exit 1; }
  grep -qE '^set .*-e[uo]*[[:space:]]|^set -e' "$s" && { echo "$s 开了 set -e" >&2; exit 1; }
done

# 3. 每条 FAIL 都要带一行处置 —— 自检不是报警器，是待办清单。
#    结构上由 report 函数保证：FAIL 分支里必须打印那一行。
for s in "$SRC_SH" "$DST_SH"; do
  grep -q '└ 处置：' "$s" || { echo "$s 的 report 不再打印处置行" >&2; exit 1; }
done

# 4. 判据编号全集 —— 增删检查项必须同步改这里，改不动就说明是误删。
expect_src="S1 S2 S3 S4 S5 S6 S7 S8"
expect_dst="D1 D2 D3 D4 D5 D6 D7 D8 D9"
ids() { grep -oE "report $1[0-9]+" "$2" | awk '{print $2}' | sort -uV | tr '\n' ' ' | sed 's/ $//'; }
[[ "$(ids S "$SRC_SH")" == "$expect_src" ]] || { echo "源端检查项编号集变了：$(ids S "$SRC_SH")" >&2; exit 1; }
[[ "$(ids D "$DST_SH")" == "$expect_dst" ]] || { echo "目标端检查项编号集变了：$(ids D "$DST_SH")" >&2; exit 1; }

# 5. 判据 3：检查项覆盖规格 #149 D.14 列的两端清单，不缺项。
#    左边是 D.14 的原话，右边是脚本里认得出它的那个串——缺一条就红。
while IFS='|' read -r item pattern; do
  [[ -z "$item" ]] && continue
  grep -qE "$pattern" "$SRC_SH" || { echo "源端自检缺 D.14 的这一项：$item" >&2; exit 1; }
done <<'ITEMS'
glibc 版本|glibc 版本 ≥
Instant Client 可加载|libclntsh
Oracle 连通|Oracle 监听口
隧道进程在位|stunnel 客户端进程在跑
隧道端口在位|隧道入口 .* 在听
经隧道到 sink 的连通性|经隧道摸得到目标端的 sink
ITEMS
while IFS='|' read -r item pattern; do
  [[ -z "$item" ]] && continue
  grep -qE "$pattern" "$DST_SH" || { echo "目标端自检缺 D.14 的这一项：$item" >&2; exit 1; }
done <<'ITEMS'
MySQL 连通|MySQL 监听口 .* 可达
开连接仪式 utf8mb4|会话字符集三项都是 utf8mb4
开连接仪式 STRICT_ALL_TABLES|sql_mode 设得成 STRICT_ALL_TABLES
开连接仪式 max_allowed_packet ≥ 64 MiB|max_allowed_packet ≥ 64 MiB
sink 起在回环|sink 没越出回环
stunnel 服务端在位|stunnel 服务端进程在跑
ITEMS

# 6. 跨文件耦合之一：目标端那三项前提是**问 sink 要的**，按它的报错措辞分档。
#    产品改一个词，自检就会把红判成绿——这几个串必须在产品那边仍然存在。
mysql_dest="$ROOT/crates/sink/src/mysql_destination.rs"
for phrase in '连接 MySQL 失败' '设置 utf8mb4 失败' 'sql_mode' 'max_allowed_packet' '回读会话变量' '环境配置错误'; do
  grep -q "$phrase" "$mysql_dest" \
    || { echo "sink 的开连接仪式不再说「${phrase}」，目标端自检的分档失效" >&2; exit 1; }
  grep -q "$phrase" "$DST_SH" \
    || { echo "目标端自检不再按「${phrase}」分档" >&2; exit 1; }
done
grep -q '64 \* 1024 \* 1024' "$mysql_dest" || { echo "sink 的 MIN_PACKET 变了" >&2; exit 1; }
grep -qx 'MIN_PACKET=67108864     # 64 MiB —— 与 crates/sink/src/mysql_destination.rs 的 MIN_PACKET 同值' "$DST_SH" \
  || { echo "目标端自检的 MIN_PACKET 与 sink 对不上了" >&2; exit 1; }

# 7. 跨文件耦合之二：两支脚本都按 sink 404 的错误码认「那头真是 sink」。
#    退化成「有人应答」的话，隧道通到别的服务上也会绿。
http="$ROOT/crates/sink/src/http.rs"
grep -q 'code: "RUN_UNKNOWN"' "$http" || { echo "sink 的 404 错误码不再是 RUN_UNKNOWN" >&2; exit 1; }
grep -q '"/v1/target/test-connection"' "$http" || { echo "sink 的 test-connection 端点没了" >&2; exit 1; }
for s in "$SRC_SH" "$DST_SH"; do
  grep -q 'RUN_UNKNOWN' "$s" || { echo "$s 不再按 RUN_UNKNOWN 认 sink" >&2; exit 1; }
done
grep -q '/v1/target/test-connection' "$DST_SH" || { echo "目标端自检不再走 sink 的 test-connection" >&2; exit 1; }

# 8. glibc 下界与 #151 的构建目标必须是同一个数。
grep -qx 'MIN_GLIBC=2.17          # ADR-0041 / #151：客户机是 CentOS 7，这是硬下界' "$SRC_SH" \
  || { echo "源端自检的 glibc 下界写法变了" >&2; exit 1; }
grep -q '2\.17' "$ROOT/packaging/centos7/README.md" || { echo "packaging/centos7 不再提 glibc 2.17" >&2; exit 1; }

# 9. 演练台判据里的期望表必须与脚本的编号全集对得上 —— 少一条就是漏判一条。
for pair in "CLEAN_SOURCE:$expect_src" "CLEAN_TARGET:$expect_dst"; do
  var=${pair%%:*}; want=${pair#*:}
  got=$(grep -oE "^$var=\".*\"" "$check" | sed "s/^$var=\"//; s/\"$//" \
        | tr ' ' '\n' | sed 's/=.*//' | sort -uV | tr '\n' ' ' | sed 's/ $//')
  [[ "$got" == "$want" ]] || { echo "$var 的期望表与编号全集对不上：[$got] vs [$want]" >&2; exit 1; }
done

# 10. 判据 4 的后半句：脚本要**列进行李清单**，不然它就还是「某台机器上的散件」。
[[ -f "$PACKING" ]] || { echo "行李清单不存在：$PACKING" >&2; exit 1; }
for s in preflight-source.sh preflight-target.sh; do
  grep -q "$s" "$PACKING" || { echo "行李清单里没有 $s" >&2; exit 1; }
done

# 11. `$var` 后面**紧挨着中文**必须加花括号。mac 上的 bash 是 3.2，它会把中文的第一个字节
#     当成变量名的一部分，报的是 `name?: unbound variable`——这条 2026-08-20 实跑时真炸过一次，
#     而炸的地方是判据脚本自己，等于把整份判据吞掉。既有台架脚本一直是加花括号的写法，
#     这里把那条不成文的规矩变成门禁。
for s in "$SRC_SH" "$DST_SH" "$check" "$classify" "${BASH_SOURCE[0]}"; do
  bad=$(LC_ALL=C grep -nE '\$[A-Za-z_][A-Za-z_0-9]*[^ -~]' "$s" || true)
  # 报错这句自己别踩上：句子里写出「美元符 + 名字 + 中文」这个形状，本条会把自己判红（实测过一次）。
  [[ -z "$bad" ]] || { echo "$s 里有「变量名后面紧挨中文」的写法，mac bash 3.2 会当场炸：" >&2; echo "$bad" >&2; exit 1; }
done

# 12. 分档判据（test-preflight-classify.sh）要把自检认得的每一档都走一遍。
#     少一档就是那一档没人走过 —— 而没走过的分支就是没有的分支。
stub_cases=$(sed -n '/^CASES = {/,/^}/p' "$stub" | grep -oE '^    "[a-z-]+"' | tr -d ' "' | sort -u | tr '\n' ' ' | sed 's/ $//')
used_cases=$(grep -oE '"C[0-9]+\|[a-z-]+\|' "$classify" | cut -d'|' -f2 | sort -u | tr '\n' ' ' | sed 's/ $//')
[[ "$stub_cases" == "$used_cases" ]] || {
  echo "桩的档与分档判据用到的档对不上" >&2
  diff -u <(tr ' ' '\n' <<<"$stub_cases") <(tr ' ' '\n' <<<"$used_cases") >&2 || true
  exit 1
}

echo "rehearsal preflight 静态自检 PASS"
