#!/usr/bin/env bash
# 演练台三支脚本的静态自检 —— 与 test-m{1,2,3}-acceptance.sh / test-v1-acceptance.sh 同一职责：
# 不起台架、不碰 docker，只守住那些「跑起来才发现就太晚」的结构性约定。
#
# 为什么演练台也要有一份：四份既有台架入口各配了一份，演练台原先没有。判据脚本是**门禁**，
# 门禁自己没有门禁的话，改坏一条负判据（或把它连同正对照一起删掉）要等到下一次实跑才知道。
set -euo pipefail
cd "$(dirname "$0")/.."

check=./scripts/rehearsal-topology-check.sh
up=./scripts/rehearsal-up.sh
reset=./scripts/rehearsal-reset.sh

for s in "$check" "$up" "$reset"; do bash -n "$s"; done

# 1. 判据编号全集 —— 增删判据必须同步改这里，改不动就说明是误删。
expected="R0a R0b R0c R0d R10 R1a R1b R2 R3 R3b R3c R4 R5 R5b R5c R5d R6 R6a R6b R7 R7a R8 R8a R9a R9b R9c R9d R9r"
actual=$(grep -oE 'report R[0-9a-z]+' "$check" | awk '{print $2}' | sort -u | tr '\n' ' ' | sed 's/ $//')
[[ "$actual" == "$expected" ]] || {
  echo "判据编号集变了" >&2
  diff -u <(tr ' ' '\n' <<<"$expected") <(tr ' ' '\n' <<<"$actual") >&2 || true
  exit 1
}

# 2. 每条负判据都要有正对照 —— 光有「不通」不算证据（脚本头自己写的纪律）。
#    这里只守结构：负判据成对出现的那些编号，其正对照编号必须也在集合里。
for pair in "R3:R4" "R5:R2" "R3b:R4" "R5b:R2" "R6:R6a" "R8:R8a" "R3c:R7" "R5c:R5d" "R10:R7a"; do
  neg=${pair%%:*}; pos=${pair##*:}
  grep -q "report $neg " "$check" || { echo "负判据 $neg 不见了" >&2; exit 1; }
  grep -q "report $pos " "$check" || { echo "$neg 的正对照 $pos 不见了" >&2; exit 1; }
done

# 3. 切断是台架显式施加的，两层都得在：路由黑洞 + 端口级 DROP（绕过路由的那条路）。
grep -q 'ip route replace blackhole' "$up" || { echo "第 1 层（路由黑洞）没了" >&2; exit 1; }
grep -q 'ip6tables' "$up" || { echo "第 2 层（IPv6 端口级 DROP）没了——宿主网关那条路会重新漏" >&2; exit 1; }
grep -q 'cut_off qbs-host-source .* 3306' "$up" || { echo "源端 3306 出向没封" >&2; exit 1; }
grep -q 'cut_off qbs-host-target .* 1521' "$up" || { echo "目标端 1521 出向没封" >&2; exit 1; }

# 4. 破坏性动作只由显式开关触发，且不认识的参数要挡住（别把 `--noreset` 这种笔误当默认值收下）。
rc=0; "$check" --bogus >/dev/null 2>&1 || rc=$?
(( rc == 2 )) || { echo "不认识的参数应当以 2 退出，实际 $rc" >&2; exit 1; }

# 5. 重建那一半只有一份实现（reset 调 up，不许抄第二遍）。
grep -q 'rehearsal-up.sh' "$reset" || { echo "reset 没有复用 up，起的那一半被抄了第二遍？" >&2; exit 1; }

echo "rehearsal 静态自检 PASS"
