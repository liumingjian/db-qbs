#!/usr/bin/env bash
# #157 —— 终局演练编排脚本的静态自检。不起台架、不碰 docker。
#
# 与 test-rehearsal-{source,target}-install.sh 同一条纪律，但守的是**另一件事**：
# 那两支守「手册与它的回放说的是同一件事」；本支守「终局演练没有自己那套装法」——
# 编排脚本一旦自己敲起装机命令，实录证的就不再是两份手册，而演练的全部意义正在于此
# （ADR-0041 §6：判据是过程性的，「手册没写、临场解决」算未达成）。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"

FINAL=./scripts/rehearsal-final.sh
SRC_DRILL=./scripts/rehearsal-source-install.sh
DST_DRILL=./scripts/rehearsal-target-install.sh
FIXTURE=./acceptance/oracle-v2-final.sql
SRC_MANUAL="$ROOT/docs/install/source-centos7.md"
DST_MANUAL="$ROOT/docs/install/target-centos7.md"

for f in "$FINAL" "$SRC_DRILL" "$DST_DRILL" "$FIXTURE" "$SRC_MANUAL" "$DST_MANUAL"; do
  [[ -f "$f" ]] || { echo "缺文件：$f" >&2; exit 1; }
done
bash -n "$FINAL"

# 1. 工具进版本库（CLAUDE.md 视觉门禁通则 4 的同一条纪律：只躺在某台机器上的门禁，下一台会静默跳过）。
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  for f in docs/spikes/fixtures/local-rig/scripts/rehearsal-final.sh \
           docs/spikes/fixtures/local-rig/scripts/test-rehearsal-final.sh \
           docs/spikes/fixtures/local-rig/acceptance/oracle-v2-final.sql; do
    git -C "$ROOT" ls-files --error-unmatch "$f" >/dev/null 2>&1 \
      || { echo "$f 没进版本库" >&2; exit 1; }
  done
fi

# 2. **编排脚本自己不装任何东西。** 装机命令只有一份来源——两份手册，经它们各自的回放跑。
#    这一条是本支存在的理由：抄第二遍装法，等于给自己发一张「手册没写也能装完」的通行证。
for forbidden in 'yum -y install' 'yum -y localinstall' 'ldconfig' 'install -m 0755' \
                 'unzip -oq' 'vault.centos.org' 'stunnel /etc/stunnel' 'ld.so.conf.d' \
                 '@@SINK_PORT@@' '@@TARGET_HOST@@' '/root/dist/preflight' 'nohup /opt/db-qbs'; do
  grep -qF "$forbidden" "$FINAL" \
    && { echo "编排脚本里出现了装机命令 [$forbidden] —— 装法必须只出自两份手册的回放" >&2; exit 1; }
done
# 反面：两支回放都必须被它调起来，而且**先目标端、后源端**（源端自检 S8 要摸到目标端回环上的 sink）。
grep -q 'rehearsal-target-install.sh --defer-step10' "$FINAL" \
  || { echo "编排脚本没照手册跑目标端那一趟" >&2; exit 1; }
# **按行首的调用判、不按提到过**：脚本开头那段注释里也写着这个文件名，
# 按名字判的话把调用注释掉照样绿。
grep -q '^\./scripts/rehearsal-source-install.sh' "$FINAL" \
  || { echo "编排脚本没照手册跑源端那一趟（要的是行首那条调用，不是注释里提一句）" >&2; exit 1; }
dst_line=$(grep -n 'rehearsal-target-install.sh --defer-step10' "$FINAL" | head -1 | cut -d: -f1)
src_line=$(grep -n '^\./scripts/rehearsal-source-install.sh' "$FINAL" | head -1 | cut -d: -f1 || true)
[[ -n "$dst_line" && -n "$src_line" && "$dst_line" -lt "$src_line" ]] \
  || { echo "顺序错了：必须先目标端（第 $dst_line 行）后源端（第 $src_line 行）" >&2; exit 1; }

# 3. 目标端手册第 10 步那四条要在**照手册装出来的**源端上敲，所以它延后、装完源端再补。
#    两个开关必须真的被回放脚本认，否则编排跑起来只会得到一条 usage。
grep -q -- '--defer-step10' "$DST_DRILL" || { echo "目标端回放不认 --defer-step10" >&2; exit 1; }
grep -q -- '--only-step10'  "$DST_DRILL" || { echo "目标端回放不认 --only-step10" >&2; exit 1; }
grep -q 'rehearsal-target-install.sh --only-step10' "$FINAL" \
  || { echo "编排脚本没把目标端手册第 10 步补回来 —— 那四条会被整趟吞掉" >&2; exit 1; }
only10_line=$(grep -n 'rehearsal-target-install.sh --only-step10' "$FINAL" | head -1 | cut -d: -f1)
[[ "$only10_line" -gt "$src_line" ]] || { echo "第 10 步补得太早：源端还没装完" >&2; exit 1; }

# 4. 先红后绿这一半仍由两支回放自己判（本支不重复判），但编排必须把它们的退出码当判据，
#    不许「跑过就算」。
for rc in dst_rc src_rc; do
  grep -q "$rc=\$?" "$FINAL" || { echo "编排脚本没接住 $rc" >&2; exit 1; }
done
grep -q 'verdict "目标端装机演练' "$FINAL" || { echo "目标端那趟的结果没进总账" >&2; exit 1; }
grep -q 'verdict "源端装机演练'   "$FINAL" || { echo "源端那趟的结果没进总账" >&2; exit 1; }

# 5. 行李清单**逐项**核对（票面判据 5）：十一项一项不少，且第 8 项（离线 rpm）在演练台上
#    只许记「不适用」并说明理由——把它记成 OK 是假绿，记成缺又会挡住整趟。
for n in 1 2 3 4 5 6 7 9 10 11; do
  grep -qE "^pack $n |^pack $n$|pack $n " "$FINAL" || { echo "行李清单第 $n 项没核对" >&2; exit 1; }
done
grep -q '第 8  项 不适用' "$FINAL" || { echo "行李清单第 8 项既没核对也没写明为什么不适用" >&2; exit 1; }
items=$(grep -cE '^\| [0-9]+ \|' "$ROOT/packaging/PACKING-LIST.md" || true)
(( items == 11 )) || { echo "行李清单现在是 $items 项，编排脚本还按 11 项核对" >&2; exit 1; }

# 6. 一次真实搬运要走完票面点名的五件事：建任务、加过滤、发起、看进度、核对目标库数据。
#    **按产品自己的 API 判**（ADR-0028 §1：断言面是 /api/*，不是 DOM）。
grep -q 'api POST /api/tasks' "$FINAL"  || { echo "没建任务" >&2; exit 1; }
grep -q 'value_source:"runtime"' "$FINAL" || { echo "没有运行时填的过滤条件" >&2; exit 1; }
grep -q 'api POST /api/runs' "$FINAL"   || { echo "没发起运行" >&2; exit 1; }
grep -q 'api GET "/api/runs/' "$FINAL"  || { echo "没看进度（没轮询运行详情）" >&2; exit 1; }
grep -q 'values_match' "$FINAL"         || { echo "没核对目标库数据" >&2; exit 1; }
# 过滤生效**要有反面**：过滤外的那几行不许出现在目标表里。只数行数的话，
# 「整表搬了一遍又恰好只有五行」这种巧合会被记成绿。
grep -q 'leaked' "$FINAL" || { echo "没判「过滤外的行没被搬过去」——只数行数是假绿" >&2; exit 1; }
# 目标表由**产品生成的 DDL** 建（v1 手工建表），不是台架里另写一份 CREATE TABLE：
# 另写一份就等于绕开了「建表 SQL 生成器」，现场那一步没被演练到。
grep -q 'api POST /api/columns' "$FINAL" || { echo "目标表 DDL 不是产品生成的" >&2; exit 1; }
grep -qE 'CREATE +TABLE' "$FINAL" && { echo "编排脚本里自己写了 CREATE TABLE，绕开了建表 SQL 生成器" >&2; exit 1; }

# 7. fixture 只碰自己那批表（`T_V2_*`），前面几份台架的基线是常量（ADR-0040 §1）。
bad=$(grep -oiE '(CREATE|DROP) TABLE +[a-z0-9_]+' "$FIXTURE" | awk '{print toupper($3)}' | grep -v '^T_V2_' || true)
[[ -z "$bad" ]] || { echo "fixture 碰了不属于自己的表：$bad" >&2; exit 1; }
# 两个业务日期、行数不等量 —— 等量的话「过滤生效」与「整表搬了一遍」分不开。
# `grep -c` 数到 0 时以 1 退出，在 `set -e` 下会当场掐断整支自检 —— 下面那句诊断永远打不出来，
# 操作者只看到一个光秃秃的 rc=1。三处替换都要兜住。
d1=$(grep -c "DATE '2026-08-20'" "$FIXTURE" || true); d2=$(grep -c "DATE '2026-08-19'" "$FIXTURE" || true)
(( d1 > 0 && d2 > 0 && d1 != d2 )) || { echo "fixture 的两个业务日期行数等量或缺一个：$d1 / $d2" >&2; exit 1; }
# 编排脚本期望的行数要与 fixture 里那一天的行数对得上。
expected=$(grep -oE '^EXPECTED_ROWS=[0-9]+' "$FINAL" | cut -d= -f2 || true)
(( expected == d1 )) || { echo "EXPECTED_ROWS=$expected 与 fixture 里 2026-08-20 的 $d1 行对不上" >&2; exit 1; }

# 8. `$var` 后面紧挨中文要加花括号（mac bash 3.2 当场炸；test-rehearsal-preflight.sh 第 11 条的同一门禁）。
self=./scripts/test-rehearsal-final.sh
for s in "$FINAL" "$self"; do
  bad=$(LC_ALL=C grep -nE '\$[A-Za-z_][A-Za-z_0-9]*[^ -~]' "$s" || true)
  [[ -z "$bad" ]] || { echo "$s 里有「变量名后面紧挨中文」的写法：" >&2; echo "$bad" >&2; exit 1; }
done

# 9. 实录与两份手册同处一个文档区（票面判据 6）。放最后：它是这趟演练的产物。
# **按「进了版本库」判，不按「磁盘上有」判**（与第 1 条同一条纪律）：
# 一份没 `git add` 的实录在本机看得见、在别人那儿不存在，而票面判据 6 说的是「进仓库」。
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  git -C "$ROOT" ls-files --error-unmatch 'docs/install/records/rehearsal-final-*.md' >/dev/null 2>&1 \
    || { echo "终局演练实录没进版本库（docs/install/records/rehearsal-final-*.md）" >&2; exit 1; }
else
  ls "$ROOT/docs/install/records/"rehearsal-final-*.md >/dev/null 2>&1 \
    || { echo "docs/install/records/ 下没有终局演练实录" >&2; exit 1; }
fi
grep -q 'rehearsal-final' "$ROOT/docs/install/README.md" \
  || { echo "docs/install/README.md 没把终局演练指出来" >&2; exit 1; }

echo "rehearsal final 静态自检 PASS"
