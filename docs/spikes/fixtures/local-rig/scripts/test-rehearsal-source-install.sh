#!/usr/bin/env bash
# #155 —— 源端装机手册与它的回放脚本的静态自检。不起台架、不碰 docker。
#
# 与 test-rehearsal-topology.sh / test-rehearsal-tunnel.sh / test-rehearsal-preflight.sh
# 同一职责：守住那些「跑起来才发现就太晚」的约定。本票要守的是**手册与回放脚本说的是同一件事**——
# 两边各说各话时，实录证的就不是手册了，而演练的全部意义正是「手册是走过的记录」。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"

MANUAL="$ROOT/docs/install/source-centos7.md"
DRILL=./scripts/rehearsal-source-install.sh
PACKING="$ROOT/packaging/PACKING-LIST.md"
PREFLIGHT="$ROOT/packaging/preflight/preflight-source.sh"

for f in "$MANUAL" "$DRILL" "$PACKING" "$PREFLIGHT"; do
  [[ -f "$f" ]] || { echo "缺文件：$f" >&2; exit 1; }
done
bash -n "$DRILL"

# 1. 手册进了版本库。**这一条不是形式**：门禁的工具只躺在某台机器上，下一台机器会静默跳过
#    （CLAUDE.md 视觉门禁通则 4 的同一条纪律，#153/#154 已经各还过一次）。
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  for f in docs/install/README.md docs/install/source-centos7.md \
           docs/spikes/fixtures/local-rig/scripts/rehearsal-source-install.sh \
           docs/spikes/fixtures/local-rig/scripts/test-rehearsal-source-install.sh; do
    git -C "$ROOT" ls-files --error-unmatch "$f" >/dev/null 2>&1 \
      || { echo "$f 没进版本库" >&2; exit 1; }
  done
  # 实录与手册同处一个文档区（规格 #149 E.17）。
  ls "$ROOT/docs/install/records/"*.md >/dev/null 2>&1 \
    || { echo "docs/install/records/ 下没有演练实录 —— 手册成了「照着想象写的」" >&2; exit 1; }
fi

# 2. 手册从行李清单开头，行李清单也认得这两份手册（清单是一份，手册引它，不各抄一份）。
grep -q 'packaging/PACKING-LIST.md' "$MANUAL" || { echo "手册没引行李清单" >&2; exit 1; }
grep -q 'source-centos7.md' "$PACKING" || { echo "行李清单里没有源端手册的去处" >&2; exit 1; }

# 3. 自检的检查项**一条不落**地出现在手册里。少写一条，现场就会有一条红没人知道怎么处置。
ids=$(grep -oE '^  report (S[0-9]+) ' "$PREFLIGHT" | awk '{print $2}' | sort -u)
[[ -n "$ids" ]] || { echo "从 preflight-source.sh 里一条检查项都抽不出来" >&2; exit 1; }
for id in $ids; do
  grep -q "$id" "$MANUAL" || { echo "手册里没有 $id" >&2; exit 1; }
done

# 4. 真机差异必须显式标出（规格 #149 User Story 4 点名的五类：root / 防火墙 / SELinux / 已装包 / yum 源）。
#    **按内容判**：容器与真机的差异不写出来，等于假装它不存在。
marks=$(grep -c '⚠ \*\*真机差异' "$MANUAL" || true)
(( marks >= 5 )) || { echo "手册里的真机差异标记只有 $marks 处，少于点名的五类" >&2; exit 1; }
for kw in SELinux firewall systemd 'yum 源' 'rpm -q'; do
  grep -q "$kw" "$MANUAL" || { echo "手册没提到真机差异里的「$kw」" >&2; exit 1; }
done

# 5. 手册与回放脚本的关键值必须是同一个：端口、路径、Instant Client 包名。
#    对不上的时候实录证的是脚本，不是手册。
for v in '/opt/oracle/instantclient' '/etc/db-qbs/source.toml' '/etc/stunnel/db-qbs/stunnel-sink.conf' \
         '/opt/db-qbs/bin' 'instantclient-basic-linux.x64-19.32.0.0.0dbru.zip'; do
  grep -qF "$v" "$MANUAL" || { echo "手册里没有 $v" >&2; exit 1; }
  grep -qF "$v" "$DRILL"  || { echo "回放脚本里没有 $v" >&2; exit 1; }
done

# 6. **ldconfig 那一步两边都要有。** 少了它自检照样全绿，而产品在「测试连接」那一刻
#    报 DPI-1047 libnnz19.so —— 2026-08-20 的演练实录里那条，这是它的门禁。
for f in "$MANUAL" "$DRILL"; do
  grep -q 'ld.so.conf.d' "$f" || { echo "$f 少了 ldconfig 注册那一步（DPI-1047 libnnz19.so 会回来）" >&2; exit 1; }
done
# 自检 S4 必须按**产品自己的**搜索路径判，而且要**显式抹掉继承来的** LD_LIBRARY_PATH：
# 把 export LD_LIBRARY_PATH=... 写进 root 的 profile 是这类机器上最常见的习惯，
# 而 systemd 拉起来的 db-qbs-source 不继承 profile —— 只是「本脚本不加」不够。
grep -q 'env -u LD_LIBRARY_PATH ldd' "$PREFLIGHT" \
  || { echo "preflight 的 S4 不再按产品自己的搜索路径判 —— ldconfig 缺失会重新变成假绿" >&2; exit 1; }
# 两种成因（缺包 / 没进 ldconfig）可能同时成立，必须一次列全，不许只报一条。
grep -q '两条都要做' "$PREFLIGHT" \
  || { echo "preflight 的 S4 不再把两种成因一次列全 —— 又变成一次多余的现场往返" >&2; exit 1; }

# 7. 手册不许把已退役的三个字段写进 source.toml —— 写了会凭空多出一条没人建过的数据源
#    （ADR-0037 §10 的迁移分支）。
for dead in oracle_connect_string oracle_username oracle_password; do
  grep -qE "^${dead} *=" "$MANUAL" && { echo "手册把已退役的 $dead 写进了配置" >&2; exit 1; }
done
grep -q '不要写 `oracle_connect_string`' "$MANUAL" \
  || { echo "手册没点名「别写那三个退役字段」" >&2; exit 1; }

# 8. 产品零改动那三处内容，本票同样不许动（与 test-rehearsal-tunnel.sh 第 9 条同一条判据，
#    这里只守手册这一侧：手册填的端口必须还是示例配置里的那两个值）。
grep -qx 'sink_base_url = "http://127.0.0.1:8080"' "$ROOT/config/source.toml.example" \
  || { echo "source.toml.example 的 sink_base_url 变了，手册第 7 步跟着就得改" >&2; exit 1; }
grep -q 'sink_base_url = "http://127.0.0.1:8080"' "$MANUAL" \
  || { echo "手册第 7 步的 sink_base_url 与示例配置对不上" >&2; exit 1; }

# 9. vault 换源那一段与构建镜像里的必须指同一个存档、同一批后备镜像。
#    这段现在有四份实现（Dockerfile / build.sh / rehearsal-tunnel-up.sh / 手册），
#    源地址一变要改四处，没门禁就会各走各的（test-rehearsal-tunnel.sh 第 11 条盯前三处）。
dockerfile="$ROOT/packaging/centos7/Dockerfile"
vault_docker=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$dockerfile" | head -1)
vault_manual=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$MANUAL" | head -1)
[[ -n "$vault_docker" && "$vault_docker" == "$vault_manual" ]] \
  || { echo "vault 存档源对不上：Dockerfile=[$vault_docker] 手册=[$vault_manual]" >&2; exit 1; }
mirror_set() { sed -e 's|\${vault_leg}|7.9.2009|g' -e 's|/altarch/7\.9\.2009|/7.9.2009|g' "$1" \
                 | grep -ohE 'https://[a-z0-9.-]+/[a-z0-9./_-]*7\.9\.2009' \
                 | grep -v 'vault\.centos\.org' | sort -u | tr '\n' ' ' | sed 's/ $//'; }
m_docker=$(mirror_set "$dockerfile"); m_manual=$(mirror_set "$MANUAL")
[[ -n "$m_docker" && "$m_docker" == "$m_manual" ]] || {
  echo "后备镜像集合对不上" >&2
  printf '  Dockerfile: %s\n  手册:       %s\n' "$m_docker" "$m_manual" >&2; exit 1; }
grep -q 'failovermethod=priority' "$MANUAL" \
  || { echo "手册少了 failovermethod=priority —— yum 默认 roundrobin 是随机挑起点" >&2; exit 1; }

echo "rehearsal source install 静态自检 PASS"
