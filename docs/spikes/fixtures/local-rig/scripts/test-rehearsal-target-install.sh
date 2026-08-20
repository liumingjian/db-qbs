#!/usr/bin/env bash
# #156 —— 目标端装机手册与它的回放脚本的静态自检。不起台架、不碰 docker。
#
# 与 test-rehearsal-source-install.sh 同一职责、同一条纪律：守住「手册与回放脚本说的是同一件事」。
# 两边各说各话时，实录证的就不是手册了，而演练的全部意义正是「手册是走过的记录」（ADR-0041 §6）。
# 这里**刻意不与源端那支合并成一支**：两份手册各自独立演进，一支合并的自检会在兄弟票的分支上无辜变红。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"

MANUAL="$ROOT/docs/install/target-centos7.md"
DRILL=./scripts/rehearsal-target-install.sh
TUNNEL_CHECK=./scripts/rehearsal-tunnel-check.sh
PACKING="$ROOT/packaging/PACKING-LIST.md"
PREFLIGHT="$ROOT/packaging/preflight/preflight-target.sh"
DST_CONF="$ROOT/packaging/stunnel/target-side/stunnel-sink.conf"

for f in "$MANUAL" "$DRILL" "$TUNNEL_CHECK" "$PACKING" "$PREFLIGHT" "$DST_CONF"; do
  [[ -f "$f" ]] || { echo "缺文件：$f" >&2; exit 1; }
done
bash -n "$DRILL"
bash -n "$TUNNEL_CHECK"

# 1. 手册与工具进了版本库。**这一条不是形式**：门禁的工具只躺在某台机器上，下一台机器会静默跳过
#    （CLAUDE.md 视觉门禁通则 4 的同一条纪律，#153/#154/#155 已经各还过一次）。
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  for f in docs/install/README.md docs/install/target-centos7.md \
           docs/spikes/fixtures/local-rig/scripts/rehearsal-target-install.sh \
           docs/spikes/fixtures/local-rig/scripts/test-rehearsal-target-install.sh; do
    git -C "$ROOT" ls-files --error-unmatch "$f" >/dev/null 2>&1 \
      || { echo "$f 没进版本库" >&2; exit 1; }
  done
fi

# 2. 手册从行李清单开头，行李清单也认得这份手册（清单是一份，手册引它，不各抄一份）。
grep -q 'packaging/PACKING-LIST.md' "$MANUAL" || { echo "手册没引行李清单" >&2; exit 1; }
grep -q 'target-centos7.md' "$PACKING" || { echo "行李清单里没有目标端手册的去处" >&2; exit 1; }
# 源端手册与目标端手册互相指着对方：两台机器各自有各自的剧本，但顺序（先目标端、后源端）要两边都说。
grep -q 'source-centos7.md' "$MANUAL" || { echo "目标端手册没指向源端那一份" >&2; exit 1; }

# 3. 自检的检查项**一条不落**地出现在手册里。少写一条，现场就会有一条红没人知道怎么处置。
ids=$(grep -oE '^  report (D[0-9]+) ' "$PREFLIGHT" | awk '{print $2}' | sort -u)
[[ -n "$ids" ]] || { echo "从 preflight-target.sh 里一条检查项都抽不出来" >&2; exit 1; }
for id in $ids; do
  grep -q "$id" "$MANUAL" || { echo "手册里没有 $id" >&2; exit 1; }
done
# D4–D7 是**问 sink 要的**（目标端不装 MySQL 客户端），手册必须把这件事说出来，
# 否则现场会有人去 yum 一个 5.x 的 mysql 客户端，对 8.0 的 caching_sha2_password 红出一条假故障。
grep -q 'caching_sha2_password' "$MANUAL" || { echo "手册没说明「为什么不装 mysql 客户端」" >&2; exit 1; }
grep -q '/v1/target/test-connection' "$MANUAL" || { echo "手册没给出 D4–D7 的等价命令（sink 的 test-connection）" >&2; exit 1; }

# 4. 真机差异必须显式标出（规格 #149 User Story 4 点名的五类：root / 防火墙 / SELinux / 已装包 / yum 源），
#    目标端另加两类本端独有的：白名单端口由客户给（唯一外部阻塞项）、MySQL 是客户的库（三前提 + 账号）。
#    **按内容判**：容器与真机的差异不写出来，等于假装它不存在。
marks=$(grep -c '⚠ \*\*真机差异' "$MANUAL" || true)
(( marks >= 7 )) || { echo "手册里的真机差异标记只有 $marks 处，少于点名的五类 + 目标端独有的两类" >&2; exit 1; }
for kw in SELinux firewall systemd 'yum 源' 'rpm -q' 'max_allowed_packet' '白名单'; do
  grep -q "$kw" "$MANUAL" || { echo "手册没提到真机差异里的「${kw}」" >&2; exit 1; }
done

# 5. 手册与回放脚本的关键值必须是同一个：端口、路径、口令文件。对不上的时候实录证的是脚本，不是手册。
for v in '/etc/db-qbs/sink.toml' '/etc/stunnel/db-qbs/stunnel-sink.conf' '/opt/db-qbs/bin' \
         '/var/run/db-qbs-stunnel-sink.pid' '/var/log/db-qbs-sink.log' '/root/.qbs-mysql-pass' \
         'listen = "127.0.0.1:8080"' '@@WHITELIST_PORT@@' '@@SINK_PORT@@'; do
  grep -qF "$v" "$MANUAL" || { echo "手册里没有 $v" >&2; exit 1; }
  grep -qF "$v" "$DRILL"  || { echo "回放脚本里没有 $v" >&2; exit 1; }
done
# 回放脚本填的占位符必须正好是目标端模板里那两个（test-rehearsal-tunnel.sh 第 4 条盯 up 脚本，这里盯回放）。
ph_tpl=$(grep -ohE '@@[A-Z_]+@@' "$DST_CONF" | sort -u | tr '\n' ' ' | sed 's/ $//')
ph_drill=$(grep -ohE 's/@@[A-Z_]+@@/' "$DRILL" | sed 's|^s/||; s|/$||' | sort -u | tr '\n' ' ' | sed 's/ $//')
[[ "$ph_tpl" == "$ph_drill" ]] || {
  echo "占位符集合对不上（目标端模板 vs 回放脚本填的）：[$ph_tpl] vs [$ph_drill]" >&2; exit 1; }

# 6. 手册不许把已退役的两个字段写进 sink.toml —— 它们一个字都不被读（ADR-0037 §2），
#    写了只会让 sink 启动时多打一条 warn，而现场看到 warn 的人会以为配置出了问题。
for dead in mysql_dsn database; do
  grep -qE "^${dead} *=" "$MANUAL" && { echo "手册把已退役的 $dead 写进了配置" >&2; exit 1; }
done
grep -q '不要写 `mysql_dsn`' "$MANUAL" || { echo "手册没点名「别写那两个退役字段」" >&2; exit 1; }

# 7. 产品零改动那几处内容，本票同样不许动（与 test-rehearsal-tunnel.sh 第 9 条同一条判据，
#    这里只守手册这一侧：手册写进 sink.toml 的 listen 必须还是示例配置里的那个值，
#    目标端模板的 connect 必须还落回环）。
grep -qx 'listen = "127.0.0.1:8080"' "$ROOT/config/sink.toml.example" \
  || { echo "sink.toml.example 的 listen 变了，手册第 5 步跟着就得改" >&2; exit 1; }
grep -qE '^connect *= *127\.0\.0\.1:@@SINK_PORT@@' "$DST_CONF" \
  || { echo "目标端模板的 connect 不落回环——手册「sink 只绑回环」那一节当场作废" >&2; exit 1; }

# 8. 三项开连接仪式前提的数值口径三处必须一致：sink（MIN_PACKET）、自检（MIN_PACKET=…）、手册（给 DBA 的纸条）。
grep -q '64 \* 1024 \* 1024' "$ROOT/crates/sink/src/mysql_destination.rs" || { echo "sink 的 MIN_PACKET 变了" >&2; exit 1; }
grep -q '^MIN_PACKET=67108864' "$PREFLIGHT" || { echo "目标端自检的 MIN_PACKET 变了" >&2; exit 1; }
grep -q '67108864' "$MANUAL" || { echo "手册里给 DBA 的 max_allowed_packet 数值与 sink 的 MIN_PACKET 对不上" >&2; exit 1; }
for kw in 'character-set-server' 'STRICT_ALL_TABLES' 'init_connect'; do
  grep -q "$kw" "$MANUAL" || { echo "手册给 DBA 的纸条少了「${kw}」" >&2; exit 1; }
done

# 9. 「只有经 stunnel 能到达 sink」那一档的证据两处都要有：手册第 10 步（从公网侧核一眼）与
#    rehearsal-tunnel-check.sh 的 --sink real（T0–T11 按产品的 RUN_UNKNOWN 认落点）。
#    少了后者，真 sink 上的隧道取证就又只剩 #153 那份桩 sink 的实录。
grep -q 'RUN_UNKNOWN' "$MANUAL" || { echo "手册没把 RUN_UNKNOWN 当作「那头是 sink」的指纹" >&2; exit 1; }
grep -q 's_client' "$MANUAL" || { echo "手册第 10 步没有从公网侧握手的那条命令" >&2; exit 1; }
grep -q -- '--sink' "$TUNNEL_CHECK" || { echo "rehearsal-tunnel-check.sh 不认 --sink real，真 sink 上 T3/T5/T7 会红在桩的标记上" >&2; exit 1; }
grep -q 'RUN_UNKNOWN' "$TUNNEL_CHECK" || { echo "rehearsal-tunnel-check.sh 的 real 落点不按 RUN_UNKNOWN 认" >&2; exit 1; }
grep -q 'code: "RUN_UNKNOWN"' "$ROOT/crates/sink/src/http.rs" || { echo "sink 的 404 错误码不再是 RUN_UNKNOWN" >&2; exit 1; }

# 10. vault 换源那一段与构建镜像里的必须指同一个存档、同一批后备镜像。这段现在有多份实现
#     （Dockerfile / build.sh / rehearsal-tunnel-up.sh / 两份手册 / 两支回放脚本各自内嵌一份）。
#     **手册与本票的回放脚本都要盯**：回放脚本的那份 heredoc 若和手册飘了，实录证的就不是手册。
dockerfile="$ROOT/packaging/centos7/Dockerfile"
vault_docker=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$dockerfile" | head -1)
for f in "$MANUAL" "$DRILL"; do
  v=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$f" | head -1)
  [[ -n "$vault_docker" && "$vault_docker" == "$v" ]] \
    || { echo "vault 存档源对不上：Dockerfile=[$vault_docker] $f=[$v]" >&2; exit 1; }
done
mirror_set() { sed -e 's|\${vault_leg}|7.9.2009|g' -e 's|/altarch/7\.9\.2009|/7.9.2009|g' "$1" \
                 | grep -ohE 'https://[a-z0-9.-]+/[a-z0-9./_-]*7\.9\.2009' \
                 | grep -v 'vault\.centos\.org' | sort -u | tr '\n' ' ' | sed 's/ $//'; }
m_docker=$(mirror_set "$dockerfile")
for f in "$MANUAL" "$DRILL"; do
  m=$(mirror_set "$f")
  [[ -n "$m_docker" && "$m_docker" == "$m" ]] || {
    echo "后备镜像集合对不上" >&2
    printf '  Dockerfile: %s\n  %s: %s\n' "$m_docker" "$f" "$m" >&2; exit 1; }
  grep -q 'failovermethod=priority' "$f" \
    || { echo "$f 少了 failovermethod=priority —— yum 默认 roundrobin 是随机挑起点" >&2; exit 1; }
done

# 11. `$var` 后面**紧挨着中文**必须加花括号（mac 的 bash 3.2 会当场炸，test-rehearsal-preflight.sh 第 11 条的同一条门禁）。
#     回放脚本与隧道判据脚本都跑在 mac 上，本支自己也是。
#     本支自己那一条按 `./scripts/...` 相对路径列（脚本开头已 cd 到 local-rig，恒对）——
#     用 `${BASH_SOURCE[0]}` 的话它相对**调用时**的 cwd 解析，从别处调进来就 grep 报错、被 `|| true` 吞掉，
#     自扫等于没扫（代码评审 2026-08-20 逮到的那条）。
self=./scripts/test-rehearsal-target-install.sh
for s in "$DRILL" "$TUNNEL_CHECK" "$self"; do
  bad=$(LC_ALL=C grep -nE '\$[A-Za-z_][A-Za-z_0-9]*[^ -~]' "$s" || true)
  [[ -z "$bad" ]] || { echo "$s 里有「变量名后面紧挨中文」的写法，mac bash 3.2 会当场炸：" >&2; echo "$bad" >&2; exit 1; }
done

# 12. 实录与手册同处一个文档区（规格 #149 E.17）。放在最后：它是这趟演练的产物，
#     其余各条不该因为实录还没落而没被判到。
ls "$ROOT/docs/install/records/"rehearsal-target-*.md >/dev/null 2>&1 \
  || { echo "docs/install/records/ 下没有目标端演练实录 —— 手册成了「照着想象写的」" >&2; exit 1; }

echo "rehearsal target install 静态自检 PASS"
