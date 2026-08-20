#!/usr/bin/env bash
# #153 —— 隧道那两支脚本与两份配置模板的静态自检。与 test-rehearsal-topology.sh 同一职责：
# 不起台架、不碰 docker，只守住那些「跑起来才发现就太晚」的结构性约定。
#
# 顺带**在这里**判本票的第四条判据「产品代码零改动」——它是一条静态事实，
# 判它不需要台架，也不该等到实跑那一刻才知道（见文末那一节）。
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(cd ../../../.. && pwd)"
TPL="$ROOT/packaging/stunnel"

up=./scripts/rehearsal-tunnel-up.sh
check=./scripts/rehearsal-tunnel-check.sh
src_conf="$TPL/source-side/stunnel-sink.conf"
dst_conf="$TPL/target-side/stunnel-sink.conf"

for s in "$up" "$check" "$TPL/gen-certs.sh"; do bash -n "$s"; done
for f in "$src_conf" "$dst_conf" "$TPL/README.md" \
         "$TPL/source-side/db-qbs-stunnel.service" "$TPL/target-side/db-qbs-stunnel.service"; do
  [[ -f "$f" ]] || { echo "模板缺文件：$f" >&2; exit 1; }
done

# 1. 判据编号全集 —— 增删判据必须同步改这里，改不动就说明是误删。
expected="T0a T0b T1 T10 T10a T11 T2 T3 T4 T5 T6 T6b T7 T7b T7c T8 T9"
actual=$(grep -oE 'report T[0-9a-z]+' "$check" | awk '{print $2}' | sort -u | tr '\n' ' ' | sed 's/ $//')
[[ "$actual" == "$expected" ]] || {
  echo "判据编号集变了" >&2
  diff -u <(tr ' ' '\n' <<<"$expected") <(tr ' ' '\n' <<<"$actual") >&2 || true
  exit 1
}

# 2. 每条负判据都要有同址正对照 —— 光有「拿不到」不算证据（脚本头自己写的纪律）。
for pair in "T4:T3" "T6:T7" "T6b:T7" "T8:T7" "T10:T10a" "T11:T5"; do
  neg=${pair%%:*}; pos=${pair##*:}
  grep -q "report $neg " "$check" || { echo "负判据 $neg 不见了" >&2; exit 1; }
  grep -q "report $pos " "$check" || { echo "$neg 的正对照 $pos 不见了" >&2; exit 1; }
done

# 3. 桩 sink 写回的标记两处必须一致 —— 不一致的话 T3/T5/T7 会全红在一个假成因上。
mk_up=$(grep -oE '^MARKER=\S+' "$up" | head -1)
mk_ck=$(grep -oE '^MARKER=\S+' "$check" | head -1)
[[ -n "$mk_up" && "$mk_up" == "$mk_ck" ]] || { echo "MARKER 两处不一致：$mk_up vs $mk_ck" >&2; exit 1; }

# 4. 模板里的占位符与 up 脚本填掉的那批必须是同一个集合。
#    新增一个占位符而没人填，要等实跑（甚至等到现场）才发现。
ph_tpl=$(grep -ohE '@@[A-Z_]+@@' "$src_conf" "$dst_conf" | sort -u | tr '\n' ' ' | sed 's/ $//')
ph_up=$(grep -ohE 's/@@[A-Z_]+@@/' "$up" | sed 's|^s/||; s|/$||' | sort -u | tr '\n' ' ' | sed 's/ $//')
[[ "$ph_tpl" == "$ph_up" ]] || {
  echo "占位符集合对不上（模板 vs up 脚本填的）" >&2
  diff -u <(tr ' ' '\n' <<<"$ph_tpl") <(tr ' ' '\n' <<<"$ph_up") >&2 || true
  exit 1
}
# up 脚本自己也在实跑时兜一道：填完还留着 @@ 就当场停。
grep -q "@@\[A-Z_\]+@@'" "$up" || { echo "up 脚本不再核对残留占位符" >&2; exit 1; }

# 5. 隧道必须**既加密又认人**：少了 verify/CAfile，中间人换一张自签证书照样接得下来。
for f in "$src_conf" "$dst_conf"; do
  grep -qE '^verify *= *2' "$f" || { echo "$f 少了 verify = 2（只加密不认人）" >&2; exit 1; }
  grep -qE '^CAfile *= *' "$f"  || { echo "$f 少了 CAfile（钉住对端那张证书）" >&2; exit 1; }
  grep -qE '^cert *= *'   "$f"  || { echo "$f 少了 cert" >&2; exit 1; }
  grep -qE '^(sslVersion|sslVersionMin) *= *TLSv1\.2' "$f" || { echo "$f 的 TLS 下界不是 1.2" >&2; exit 1; }
done

# 6. 落点形态：目标端隧道口对外、落回环；源端隧道入口只绑回环（进去就是明文）。
grep -qE '^accept *= *0\.0\.0\.0:@@WHITELIST_PORT@@'  "$dst_conf" || { echo "目标端 accept 形态变了" >&2; exit 1; }
grep -qE '^connect *= *127\.0\.0\.1:@@SINK_PORT@@'    "$dst_conf" || { echo "目标端 connect 不落回环——ADR-0024 的兜底当场作废" >&2; exit 1; }
grep -qE '^accept *= *127\.0\.0\.1:@@SINK_LOCAL_PORT@@' "$src_conf" || { echo "源端 accept 不只绑回环" >&2; exit 1; }

# 7. 私钥与证书材料一个字节都不进版本库。
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  leaked=$(git -C "$ROOT" ls-files 'packaging/stunnel' | grep -E '\.key$|\.crt$|^packaging/stunnel/out/' || true)
  [[ -z "$leaked" ]] || { echo "证书材料进了版本库：$leaked" >&2; exit 1; }
fi

# 8. 演练台上目标端只发布白名单那一个口（compose 里就该只有这一条）。
pub=$(awk '/^  host-target:/{f=1} f&&/^  [a-z]/&&!/^  host-target:/{f=0} f' docker-compose.yml \
      | grep -oE '"[0-9.]*:?[0-9]+:[0-9]+"' | tr -d '"' | tr '\n' ' ' | sed 's/ $//')
[[ "$pub" == "127.0.0.1:15443:15443" ]] || { echo "目标端发布的端口变了：[$pub]" >&2; exit 1; }

# 9. 本票第四条判据：**产品代码零改动**。
#    隧道买的就是「产品一行不改」，这条判据没有台架能替它作证 —— 台架只看得见跑起来的样子，
#    看不见有没有为了跑起来偷偷放开 scheme 校验。
#
#    **按内容判，不按分支的 diff 判。** 按 diff 判有两处会当场坏掉：一支公共静态自检会在
#    兄弟票（比如动 web/ 的一键重跑）的分支上无辜变红；而本票一旦合入 main，
#    merge-base 就是 HEAD、diff 恒空，判据永久绿 —— 一条永远绿的门禁不是门禁。
#    所以这里断言的是隧道方案**赖以成立的那三处内容本身**，改一处就红，合入之后也一样红。
proto="$ROOT/crates/source/src/protocol.rs"
grep -q 'if url.scheme() != "http"' "$proto" || {
  echo "protocol.rs 不再硬性拒绝非 http 的 sink_base_url —— 隧道方案的前提（产品零改动）没了" >&2
  exit 1
}
grep -qx 'sink_base_url = "http://127.0.0.1:8080"' "$ROOT/config/source.toml.example" || {
  echo "source.toml.example 的 sink_base_url 变了 —— 源端 stunnel 的 accept 端口跟着就得改" >&2
  exit 1
}
grep -qx 'listen = "127.0.0.1:8080"' "$ROOT/config/sink.toml.example" || {
  echo "sink.toml.example 的 listen 变了 —— ADR-0024「只绑回环」的兜底或目标端 connect 端口跟着就得改" >&2
  exit 1
}

# 10. 三处自述的判据区间必须都是 T0–T11 —— 编号集由第 1 条守着，散文里的区间号没人守的话
#     会各说各话（评审实测：README 写过 T1–T12，脚本头写过 T1–T11）。
for f in "$check" "$TPL/README.md" README.md; do
  grep -q 'T0–T11' "$f" || { echo "$f 里的判据区间不是 T0–T11" >&2; exit 1; }
  grep -qE 'T1–T1[12]' "$f" && { echo "$f 里还留着旧的判据区间写法" >&2; exit 1; }
done

# 11. vault 换源那一段与构建镜像里的必须指同一个存档 —— 这段现在有两份实现
#     （Dockerfile 一份、up 脚本内嵌一份），源地址一变要改两处，没门禁就会各走各的。
vault_docker=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$ROOT/packaging/centos7/Dockerfile" | head -1)
vault_up=$(grep -oE 'vault\.centos\.org/[0-9.]+' "$up" | head -1)
[[ -n "$vault_docker" && "$vault_docker" == "$vault_up" ]] || {
  echo "vault 存档源对不上：Dockerfile=[$vault_docker] up=[$vault_up]" >&2; exit 1; }

echo "rehearsal tunnel 静态自检 PASS"
