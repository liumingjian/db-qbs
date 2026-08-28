#!/usr/bin/env bash
# #157 —— 终局从零演练：两台主机推倒重来，**只照两份手册**装完两端，经隧道跑通一次真实搬运。
#
# 这支脚本是 #155/#156 两份回放的编排层，**它自己不装任何东西**：
# 目标端那一趟出自 `rehearsal-target-install.sh`（照 `docs/install/target-centos7.md`），
# 源端那一趟出自 `rehearsal-source-install.sh`（照 `docs/install/source-centos7.md`）。
# 装机命令只有一份来源——手册；抄第二遍就等于给自己发了一张「手册没写也能装完」的通行证。
#
# 本脚本自己新增的只有最后那一段：**一次真实搬运**（规格 #149 User Story 14）——
# 建数据源 → 查数据 → 生成建表 DDL 并交给 DBA 建表 → 建任务并加过滤条件 → 发起 →
# 看进度 → 核对目标库数据。那一段走的是产品自己的 `/api/*`（ADR-0028 §1：断言面是 API 不是 DOM），
# 在**源端主机容器里**用 `curl` 打，与所有者在笔记本上经 SSH 转发点界面是同一条路径。
#
# 判据是过程性的（ADR-0041 §6），对应票面六条：
#   1) 从干净容器起步，全程只照手册，零即兴命令；
#   2) 两端自检**先红后绿**：干净时缺项一次列全，装完全绿；
#   3) 经隧道完成一次真实搬运：建任务、加过滤、发起、看进度、目标库数据核对无误；
#   4) 演练中的临场解决全部回写手册并重走过（回写这件事由人做，脚本只负责把它暴露出来）；
#   5) 行李清单逐项核对无缺；
#   6) 演练实录进仓库，与两份手册同处一个文档区（`docs/install/records/`）。
#
# 前提：
#   ./scripts/up.sh                                   两个库（本脚本会核对它们在跑）
#   packaging/centos7/build.sh --platform linux/amd64 三个二进制（行李清单第 1、2 项）
#   —— 两台主机由本脚本推倒重建，证书没有就地出一份，不必手工准备。
#
# 用法：./scripts/rehearsal-final.sh
#
# **跑完不清场**：两台主机、隧道、搬完的目标表全留着，实录要照着它们抄
# （与 run-v1-acceptance.sh 的「跑完默认不清场」同一条裁定）。要归零跑 ./scripts/rehearsal-reset.sh。
set -uo pipefail
cd "$(dirname "$0")/.."
RIG="$(pwd)"
ROOT="$(cd ../../../.. && pwd)"
TPL="$ROOT/packaging/stunnel"
BIN_DIR="$ROOT/packaging/centos7/out/bin/linux-amd64"

SRC=qbs-host-source
DST=qbs-host-target
SOURCE_UI=127.0.0.1:8088          # source 的 listen（只绑回环，手册第 7 步）
ORACLE_SERVICE=XE
ORACLE_USER=spike
ORACLE_PASSWORD=spike123
MYSQL_USER=spike
MYSQL_PASSWORD=spike123
MYSQL_DATABASE=qbs
SOURCE_TABLE=T_V2_TRIAL           # 「客户那张表」在演练台上的替身（acceptance/oracle-v2-final.sql）
TARGET_TABLE=V2_TRIAL
BIZ_DATE=2026-08-20               # 过滤条件的值：这一天五行，前一天两行
EXPECTED_ROWS=5

[[ $# -eq 0 ]] || { echo "用法：$0   （不带参数；要归零先 ./scripts/rehearsal-reset.sh）"; exit 2; }

OK=0
phase() { echo; echo "================================ $* ================================"; }
sub()   { echo; echo "-------- $* --------"; }
verdict() { # $1=说明 $2=期望 $3=实测
  if [[ "$3" == "$2" ]]; then printf '  PASS  %-52s 实测=%s\n' "$1" "$3"
  else printf '  FAIL  %-52s 期望=%s 实测=%s\n' "$1" "$2" "$3"; OK=1; fi
}
die() { echo; echo "!! $*"; exit 1; }

command -v jq >/dev/null || die "没有 jq —— 搬运那一段要靠它读产品的 JSON 回话"

# 产品自己的 API，在**源端主机容器里**打（真机上是所有者经 ssh -L 8088 点的那个界面）。
# 回话与状态码分开取：状态码贴在最后一行。
API_STATUS=""; API_BODY=""
api() { # $1=方法 $2=路径 $3=请求体（可空）
  local method=$1 path=$2 body=${3:-} out
  if [[ -n "$body" ]]; then
    out=$(docker exec -e BODY="$body" "$SRC" bash -lc \
      "curl -sS -X ${method} 'http://${SOURCE_UI}${path}' -H 'Content-Type: application/json' -d \"\$BODY\" -w '\n%{http_code}'" 2>&1)
  else
    out=$(docker exec "$SRC" bash -lc \
      "curl -sS -X ${method} 'http://${SOURCE_UI}${path}' -w '\n%{http_code}'" 2>&1)
  fi
  API_STATUS=$(tail -1 <<<"$out" | tr -d '\r')
  API_BODY=$(sed '$d' <<<"$out")
}
# 两个库上的「DBA 命令」。它们**不在被演练的那两台机器上敲**——那两台上装的东西只出自手册。
ora()  { docker exec -i qbs-client sqlplus -S "${ORACLE_USER}/${ORACLE_PASSWORD}@//oracle:1521/${ORACLE_SERVICE}"; }
mysqlq() { docker exec -i qbs-mysql8 mysql -N -B "-u${MYSQL_USER}" "-p${MYSQL_PASSWORD}" "$MYSQL_DATABASE" 2>/dev/null | tr -d '\r'; }

# ================================================================ 第 0 段：前提
phase "第 0 段：前提（两个库在跑、三个二进制在手上）"
for c in qbs-oracle11 qbs-mysql8 qbs-client; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || die "$c 没起 —— 先跑 ./scripts/up.sh"
done
echo "  两个库与 client 容器在跑（本机 $(hostname)）"

# ================================================================ 第 1 段：行李清单逐项核对
# 票面判据 5。**装机现场没有第二次机会**：缺一项就地停，别装到一半才发现。
phase "第 1 段：行李清单逐项核对（packaging/PACKING-LIST.md 十一项）"
PACK_MISSING=0
pack() { # $1=项号 $2=东西 $3=实测说明（空串=缺）
  if [[ -n "$3" ]]; then printf '  第 %-2s 项 OK    %-34s %s\n' "$1" "$2" "$3"
  else printf '  第 %-2s 项 缺    %s\n' "$1" "$2"; PACK_MISSING=$((PACK_MISSING+1)); fi
}
arch_of() { file -b "$1" 2>/dev/null | grep -o 'x86-64\|x86_64\|aarch64\|ARM aarch64' | head -1; }
p1=""; for b in db-qbs-source db-qbs-source-run; do
  [[ -x "$BIN_DIR/$b" ]] || { p1=""; break; }; p1="$(arch_of "$BIN_DIR/db-qbs-source")"
done
pack 1 "db-qbs-source / -source-run" "${p1:+${p1}，$(stat -f%z "$BIN_DIR/db-qbs-source" 2>/dev/null || stat -c%s "$BIN_DIR/db-qbs-source" 2>/dev/null) 字节}"
p2=""; [[ -x "$BIN_DIR/db-qbs-sink" ]] && p2="$(arch_of "$BIN_DIR/db-qbs-sink")"
pack 2 "db-qbs-sink" "${p2:+${p2}，$(stat -f%z "$BIN_DIR/db-qbs-sink" 2>/dev/null || stat -c%s "$BIN_DIR/db-qbs-sink" 2>/dev/null) 字节}"
IC_ZIP_NAME=instantclient-basic-linux.x64-19.32.0.0.0dbru.zip
IC_CACHE="${IC_CACHE:-$HOME/.cache/db-qbs}"
# **不在缓存里不能记 OK**：真机上「到时候下一个」一次都不成立（清单第 3 项写的是「出发前下好」），
# 而演练台联网、源端那趟会现下——这既不是「带齐了」也不是「缺」，走第三档，与第 8 项同一种记法。
# 记成 OK 的话，一台没缓存又不出网的机器会在这里全绿、到源端第 4 步才炸，
# 正是本段那句「缺一项就地停」要挡的「装到一半才发现」。
if [[ -f "$IC_CACHE/$IC_ZIP_NAME" ]] && unzip -tq "$IC_CACHE/$IC_ZIP_NAME" >/dev/null 2>&1; then
  pack 3 "Instant Client 19c Basic (x64)" "缓存在 ${IC_CACHE}，unzip -t 通过"
else
  printf '  第 3  项 不适用 Instant Client 19c Basic       不在缓存里；演练台联网、源端那趟现下。真机上这一项是「出发前下好」，缺它就是就地停\n'
fi
pack 4 "preflight-source.sh" "$([[ -f "$ROOT/packaging/preflight/preflight-source.sh" ]] && echo 在 packaging/preflight/)"
pack 5 "preflight-target.sh" "$([[ -f "$ROOT/packaging/preflight/preflight-target.sh" ]] && echo 在 packaging/preflight/)"
p6=""
if [[ -f "$TPL/source-side/stunnel-sink.conf" && -f "$TPL/target-side/stunnel-sink.conf" \
      && -f "$TPL/target-side/db-qbs-stunnel.service" ]]; then
  # 两端模板**同名不同内容**（拿错了占位符对不上）——这一条要判出来，不是只判文件在。
  if cmp -s "$TPL/source-side/stunnel-sink.conf" "$TPL/target-side/stunnel-sink.conf"; then
    p6=""; echo "  !! 两端模板内容相同 —— 目标端那份多半被源端那份覆盖了"
  else
    p6="两端各一份 + systemd unit，同名不同内容"
  fi
fi
pack 6 "stunnel 配置模板（两端 + unit）" "$p6"
if [[ ! -f "$TPL/out/source-side/source.key" || ! -f "$TPL/out/target-side/target.key" ]]; then
  echo "  （证书材料不在 —— 照行李清单第 7 项「出发前跑一次 gen-certs.sh」就地出一套）"
  "$TPL/gen-certs.sh" >/dev/null || die "gen-certs.sh 失败"
fi
p7=""
[[ -f "$TPL/out/source-side/source.key" && -f "$TPL/out/target-side/target.key" \
   && -f "$TPL/out/source-side/target.crt" && -f "$TPL/out/target-side/source.crt" ]] \
  && p7="两端各一套（私钥 $(ls -l "$TPL/out/source-side/source.key" | cut -c1-10)）"
pack 7 "stunnel 双端证书材料" "$p7"
# 第 8 项在演练台上**不适用**：容器联网走 vault 三源，离线 rpm 那条路是真机差异 ②。
printf '  第 8  项 不适用 离线 rpm            演练台联网走 vault 三源；离线那条路是手册真机差异 ②，容器上验不到\n'
p9=""
[[ -f "$ROOT/config/source.toml.example" && -f "$ROOT/config/sink.toml.example" ]] && p9="config/ 下两份都在"
pack 9 "两份配置样例" "$p9"
p10=""
[[ -f "$ROOT/docs/install/source-centos7.md" && -f "$ROOT/docs/install/target-centos7.md" ]] && p10="docs/install/ 下两份都在"
pack 10 "两份装机手册" "$p10"
p11=""
grep -q 'max_allowed_packet' "$ROOT/docs/install/target-centos7.md" \
  && grep -q 'GRANT SELECT, INSERT, UPDATE, CREATE, DROP' "$ROOT/docs/install/target-centos7.md" \
  && p11="目标端手册第 7 步（三前提 + 授权语句）"
pack 11 "给 DBA 的纸条" "$p11"
verdict "行李清单逐项核对无缺（不适用的那几项另记）" 0 "$PACK_MISSING"
(( PACK_MISSING == 0 )) || die "行李没齐 —— 装机那天没有第二次机会，就地停"

# ================================================================ 第 2 段：推倒重建 + 拓扑判据
phase "第 2 段：两台主机推倒重建回干净机器态，跑拓扑判据（装隧道之前）"
./scripts/rehearsal-topology-check.sh --reset
topo_rc=$?
verdict "拓扑判据 R0–R10（--reset，干净态）" 0 "$topo_rc"
(( topo_rc == 0 )) || die "拓扑判据没过 —— 演练会跑在一张比客户现场宽松的网上，就地停"

# ================================================================ 第 3 段：目标端照手册装
# 先目标端、后源端（源端自检 S8 要摸到目标端回环上的 sink）。
# **第 10 步延后**：那四条要在源端那台上敲，而源端此刻还是干净机器。
phase "第 3 段：目标端照 docs/install/target-centos7.md 从零装（第 10 步延后）"
./scripts/rehearsal-target-install.sh --defer-step10
dst_rc=$?
verdict "目标端装机演练（第 1–9 步）" 0 "$dst_rc"
(( dst_rc == 0 )) || die "目标端没装成 —— 源端的 S8 也就无从转绿，就地停"

# ================================================================ 第 4 段：源端照手册装
phase "第 4 段：源端照 docs/install/source-centos7.md 从零装"
./scripts/rehearsal-source-install.sh
src_rc=$?
verdict "源端装机演练（第 1–10 步）" 0 "$src_rc"
(( src_rc == 0 )) || die "源端没装成，就地停"

# ================================================================ 第 5 段：补上目标端手册第 10 步
phase "第 5 段：回来补目标端手册第 10 步（在**照手册装出来的**源端上敲）"
./scripts/rehearsal-target-install.sh --only-step10
step10_rc=$?
verdict "目标端手册第 10 步四条（公网侧只有经隧道到得了 sink）" 0 "$step10_rc"

# ================================================================ 第 6 段：隧道判据在真 sink 上
phase "第 6 段：隧道判据 T0–T11（落点是真 sink）"
./scripts/rehearsal-tunnel-check.sh --sink real
tunnel_rc=$?
verdict "隧道判据（--sink real）" 0 "$tunnel_rc"

# ================================================================ 第 7 段：一次真实搬运
phase "第 7 段：经隧道跑通一次真实搬运（规格 #149 User Story 14）"

sub "① 客户的库里本来就有那张表（这一步是 DBA 的活，不在两台主机上敲）"
ora < "$RIG/acceptance/oracle-v2-final.sql" | tail -12
ORACLE_HOST=$(docker inspect -f '{{index .NetworkSettings.Networks "qbs-src-side" "IPAddress"}}' qbs-oracle11)
MYSQL_HOST=$(docker inspect -f '{{index .NetworkSettings.Networks "qbs-dst-side" "IPAddress"}}' qbs-mysql8)
[[ -n "$ORACLE_HOST" && -n "$MYSQL_HOST" ]] || die "取不到两个库在侧网上的 IP"
echo "  Oracle（源端那边看到的）= ${ORACLE_HOST}:1521/${ORACLE_SERVICE}"
echo "  MySQL（目标端那边看到的）= ${MYSQL_HOST}:3306/${MYSQL_DATABASE}"

sub "①.5 注册目标端 Agent（手册第 10.5 步）——目标库只能经它访问（ADR-0044 §1）"
api POST /api/agents '{"name":"演练目标端","base_url":"http://127.0.0.1:8080"}'
echo "  POST /api/agents → $API_STATUS $(head -c 160 <<<"$API_BODY")"
AGENT_ID=$(jq -r '.agent_id // empty' <<<"$API_BODY")
if [[ -z "$AGENT_ID" ]]; then
  # 注册**探不通就不落库**，所以这里空着只有一种可能：隧道或目标端 agent 没起来。
  api GET /api/agents
  AGENT_ID=$(jq -r '.[0].agent_id // empty' <<<"$API_BODY")
fi
[[ -n "$AGENT_ID" ]] || die "目标端 agent 没注册上——隧道或 sink 没起（ADR-0044 §3：探不通就不落库）"

sub "② 两条数据源：Oracle 与目标库（目标库那条绑上面那台 agent，凭据随 run 过线，测连要经隧道走到它）"
api POST /api/datasources "$(jq -nc --arg cs "//${ORACLE_HOST}:1521/${ORACLE_SERVICE}" \
  --arg u "$ORACLE_USER" --arg p "$ORACLE_PASSWORD" \
  '{name:"演练 Oracle", kind:"oracle", connect_string:$cs, username:$u, password:$p}')"
echo "  POST /api/datasources (oracle) → $API_STATUS"
ORACLE_DS=$(jq -r '.datasource_id // empty' <<<"$API_BODY")
api POST /api/datasources "$(jq -nc --arg h "$MYSQL_HOST" --arg u "$MYSQL_USER" \
  --arg p "$MYSQL_PASSWORD" --arg d "$MYSQL_DATABASE" --arg a "$AGENT_ID" \
  '{name:"演练目标库", kind:"mysql", agent_id:$a, host:$h, port:3306, username:$u, password:$p, database:$d}')"
echo "  POST /api/datasources (mysql)  → $API_STATUS"
TARGET_DS=$(jq -r '.datasource_id // empty' <<<"$API_BODY")
[[ -n "$ORACLE_DS" && -n "$TARGET_DS" ]] || die "数据源没建成：oracle=$ORACLE_DS target=$TARGET_DS"
api POST "/api/datasources/${ORACLE_DS}/test-connection"
ora_test=$API_STATUS; echo "  测连 Oracle → $ora_test $(head -c 160 <<<"$API_BODY")"
api POST "/api/datasources/${TARGET_DS}/test-connection"
tgt_test=$API_STATUS; echo "  测连 目标库（经隧道到 sink，再由 sink 连 MySQL）→ $tgt_test $(head -c 160 <<<"$API_BODY")"

sub "③ 查数据：先列表、再取列（构建器那两下）"
api POST /api/builder/tables "$(jq -nc --arg ds "$ORACLE_DS" '{datasource_id:$ds}')"
tables_status=$API_STATUS
tables_hit=$(jq -r --arg t "$SOURCE_TABLE" '[.[] | select(.name == $t)] | length' <<<"$API_BODY" 2>/dev/null || echo 0)
echo "  POST /api/builder/tables → ${tables_status}，清单里有 ${SOURCE_TABLE}：$tables_hit"
api POST /api/builder/columns "$(jq -nc --arg ds "$ORACLE_DS" --arg o "$ORACLE_USER" --arg t "$SOURCE_TABLE" \
  '{datasource_id:$ds, owner:($o|ascii_upcase), table:$t}')"
cols_status=$API_STATUS
echo "  POST /api/builder/columns → $cols_status"
jq -r '[.[] | "\(.name) \(.data_type)"] | join("，")' <<<"$API_BODY" 2>/dev/null | sed 's/^/    /' 

sub "④ 组任务定义：四列映射 + 一段过滤条件（按业务日期的 WHERE 文本）"
SPEC=$(jq -nc --arg owner "$(echo "$ORACLE_USER" | tr '[:lower:]' '[:upper:]')" \
  --arg table "$SOURCE_TABLE" --arg target "$TARGET_TABLE" --arg biz_date "$BIZ_DATE" '{
    owner:$owner, table:$table, target_table:$target,
    write_mode:"APPEND",
    primary_key:["ROW_ID"],
    columns:[
      {source:"ROW_ID",    target:"ROW_ID"},
      {source:"CUST_NAME", target:"CUST_NAME"},
      {source:"AMOUNT",    target:"AMOUNT"},
      {source:"LOAD_DATE", target:"LOAD_DATE"}
    ],
    where_clause:("LOAD_DATE = DATE \u0027" + $biz_date + "\u0027")
  }')
api POST /api/builder/sql "$SPEC"
echo "  POST /api/builder/sql → $API_STATUS"
BUILT_SQL=$(jq -r '.source_sql // empty' <<<"$API_BODY")
echo "$BUILT_SQL" | sed 's/^/    /'
where_ok=$(grep -c "WHERE LOAD_DATE = DATE '$BIZ_DATE'" <<<"$BUILT_SQL")

sub "⑤ 生成建表 DDL，交给 DBA 在目标库上建表（v1：目标表手工建，产品不自动建）"
api POST /api/columns "$(jq -nc --arg ds "$ORACLE_DS" --argjson spec "$SPEC" '{datasource_id:$ds, spec:$spec}')"
echo "  POST /api/columns → $API_STATUS"
TARGET_DDL=$(jq -r '.target_ddl // empty' <<<"$API_BODY")
echo "$TARGET_DDL" | sed 's/^/    /'
[[ -n "$TARGET_DDL" ]] || die "没拿到建表 DDL：$API_STATUS $(head -c 300 <<<"$API_BODY")"
# DBA 在目标库上执行它。重跑时先删——上一趟的表还在的话，建表会报 already exists，
# 而那不是「搬运失败」，别让它冒充成一条判据。
# **建表的报错不许吞**：`generate_target_ddl` 对裸 `NUMBER` 会输出 `DECIMAL(<p>,<s>)` 占位符并照样回 200，
# 那条 DDL 交给 MySQL 是语法错。吞掉的话这里照样打印「已建好」，几步之后以一个对不上的行数收场。
ddl_err=$(printf 'DROP TABLE IF EXISTS `%s`;\n%s\n' "$TARGET_TABLE" "$TARGET_DDL" \
  | docker exec -i qbs-mysql8 mysql "-u${MYSQL_USER}" "-p${MYSQL_PASSWORD}" "$MYSQL_DATABASE" 2>&1 >/dev/null \
  | grep -v 'Using a password on the command line' || true)
[[ -z "$ddl_err" ]] || echo "  !! MySQL 执行建表 DDL 时报错：$ddl_err"
echo "  目标表已由 DBA 建好："
target_desc=$(printf 'DESCRIBE `%s`;\n' "$TARGET_TABLE" | mysqlq)
sed 's/^/    /' <<<"$target_desc"
# 判据取**实际建出来的列数**，不取「DDL 拿到了」——后者只证明产品出了字，没证明目标库认它。
target_cols=$(grep -c . <<<"$target_desc" || true)

sub "⑥ 建任务"
api POST /api/tasks "$(jq -nc --arg s "$ORACLE_DS" --arg t "$TARGET_DS" --argjson spec "$SPEC" \
  '{name:"演练：按业务日期搬一次", source_datasource_id:$s, target_datasource_id:$t, spec:$spec}')"
echo "  POST /api/tasks → $API_STATUS"
TASK_ID=$(jq -r '.task_id // empty' <<<"$API_BODY")
[[ -n "$TASK_ID" ]] || die "任务没建成：$API_STATUS $(head -c 300 <<<"$API_BODY")"

sub "⑦ 发起：点了就跑（过滤条件写在任务定义里，${BIZ_DATE} 这一天五行；前一天那两行不该被搬走）"
api POST /api/runs "$(jq -nc --arg t "$TASK_ID" '{task_id:$t}')"
echo "  POST /api/runs → $API_STATUS"
RUN_RECORD=$(jq -r '.run_record_id // empty' <<<"$API_BODY")
[[ -n "$RUN_RECORD" ]] || die "发起失败：$API_STATUS $(head -c 300 <<<"$API_BODY")"

sub "⑧ 看进度：轮询运行详情，阶段变一次记一行（界面上那条阶段线与四个计数）"
last=""; live=true; finished_body=""
for _ in $(seq 1 300); do
  api GET "/api/runs/${RUN_RECORD}"
  # **只有 200 才算一次有效的观察**：500（「run 投影锁已损坏」）或 curl 噪声下 `.live` 也不是 true，
  # 按它收尾会把一次台架抖动记成「搬运失败、终态为空」——红在一个假成因上。
  if [[ "$API_STATUS" != 200 ]]; then
    echo "    （GET /api/runs 回了 ${API_STATUS}，不算终态，继续轮询：$(head -c 120 <<<"$API_BODY")）"
    sleep 1; continue
  fi
  # **别写 `.live // empty`**：jq 的 `//` 把 `false` 也当「缺失」，落终态时（`live: false`）
  # 取回来的是空串，于是永远等不到那个 false —— 搬运明明成了，脚本却轮满 300 次报「卡住」。
  # 2026-08-20 的第二趟演练就是这么炸的，成因整整查了一圈产品侧。取原值，三态各判各的。
  live=$(jq -r '.live' <<<"$API_BODY")
  if [[ "$live" == false ]]; then
    finished_body=$API_BODY
    break
  fi
  if [[ "$live" == true ]]; then
    now=$(jq -r '"阶段=\(.stage // "—") 已推行数=\(.rows_pushed // 0) 批次序号=\(.seq // 0) 累计字节=\(.bytes // 0) 已用时=\(.ms // 0)ms"' <<<"$API_BODY")
    stage_now=$(jq -r '.stage // "—"' <<<"$API_BODY")
    [[ "$stage_now" == "$last" ]] || { echo "    $now"; last=$stage_now; }
  else
    echo "    （回话里没有 live 字段，不算一次有效观察：$(head -c 120 <<<"$API_BODY")）"
  fi
  sleep 1
done
[[ -n "$finished_body" ]] || die "轮到 300 次还没落终态 —— 搬运卡住了"
echo "  终态："
jq -r '"    outcome=\(.outcome) target_table_effect=\(.target_table_effect) 源端行数=\(.source_rows) 暂存行数=\(.staged_rows) sink 回报行数=\(.sink_reported_rows) 批数=\(.source_batches) 暂存表=\(.staging_table)"' <<<"$finished_body"
RUN_OUTCOME=$(jq -r '.outcome // empty' <<<"$finished_body")
RUN_SOURCE_ROWS=$(jq -r '.source_rows // 0' <<<"$finished_body")
RUN_SINK_ROWS=$(jq -r '.sink_reported_rows // 0' <<<"$finished_body")

sub "⑨ 核对目标库数据：目标库里的五行必须与 Oracle 里那一天的五行逐值一致"
expected=$(printf "%s\n" \
  "SET PAGESIZE 0 FEEDBACK OFF HEADING OFF VERIFY OFF LINESIZE 300" \
  "SELECT row_id||'|'||cust_name||'|'||TO_CHAR(amount,'FM99999999990.00')||'|'||TO_CHAR(load_date,'YYYY-MM-DD HH24:MI:SS')" \
  "  FROM ${SOURCE_TABLE} WHERE load_date = DATE '${BIZ_DATE}' ORDER BY row_id;" \
  "EXIT" | ora | sed 's/[[:space:]]*$//' | grep -v '^$')
actual=$(printf "SELECT CONCAT_WS('|', ROW_ID, CUST_NAME, CAST(AMOUNT AS CHAR), DATE_FORMAT(LOAD_DATE,'%%Y-%%m-%%d %%H:%%i:%%s')) FROM \`%s\` ORDER BY ROW_ID;\n" \
  "$TARGET_TABLE" | mysqlq | sed 's/[[:space:]]*$//' | grep -v '^$')
echo "  Oracle 那一天（期望）：";  sed 's/^/    /' <<<"$expected"
echo "  MySQL 目标表（实测）：";  sed 's/^/    /' <<<"$actual"
rows_actual=$(grep -c . <<<"$actual")
if diff <(echo "$expected") <(echo "$actual") >/dev/null; then values_match=一致; else
  values_match=不一致; echo "  差异："; diff <(echo "$expected") <(echo "$actual") | sed 's/^/    /'
fi
# 过滤真的生效了没有：前一天那两行**不许**出现在目标表里。
leaked=$(printf "SELECT COUNT(*) FROM \`%s\` WHERE LOAD_DATE <> '%s 00:00:00';\n" "$TARGET_TABLE" "$BIZ_DATE" | mysqlq)

# ================================================================ 总账
phase "总账"
verdict "第 7 段②：Oracle 数据源测连" 200 "$ora_test"
verdict "第 7 段②：目标库测连（经隧道到 sink）" 200 "$tgt_test"
verdict "第 7 段③：表清单里查得到 ${SOURCE_TABLE}" 1 "$tables_hit"
verdict "第 7 段③：取列" 200 "$cols_status"
verdict "第 7 段④：过滤条件进了源端 SQL（绑定变量）" 1 "$where_ok"
verdict "第 7 段⑤：目标表按产品给的 DDL 真建出来了（列数）" "$(jq '.columns | length' <<<"$SPEC")" "${target_cols:-0}"
verdict "第 7 段⑤：建表时 MySQL 没报错" 无 "$([[ -n "$ddl_err" ]] && echo 有 || echo 无)"
verdict "第 7 段⑦⑧：搬运终态" SUCCEEDED "$RUN_OUTCOME"
verdict "第 7 段⑧：源端行数 = 过滤后应有的行数" "$EXPECTED_ROWS" "$RUN_SOURCE_ROWS"
verdict "第 7 段⑧：sink 回报行数与源端一致" "$RUN_SOURCE_ROWS" "$RUN_SINK_ROWS"
verdict "第 7 段⑨：目标库实际行数" "$EXPECTED_ROWS" "$rows_actual"
verdict "第 7 段⑨：目标库逐值与源库一致" 一致 "$values_match"
verdict "第 7 段⑨：过滤外的行没被搬过去" 0 "$leaked"
echo
echo "  留在台架上的东西（实录照着抄，跑完不清场）："
echo "    源端界面 = 源端主机容器的 ${SOURCE_UI}（真机上 ssh -L 8088:127.0.0.1:8088）"
echo "    run_record_id = ${RUN_RECORD}    task_id = ${TASK_ID}"
echo "    目标表 = ${MYSQL_DATABASE}.${TARGET_TABLE}"
echo
if (( OK )); then
  echo "==== 终局演练：未达成（上面 FAIL 那几条就是还欠的地方；「手册没写、临场解决」一律回写手册后重走）===="
  exit 1
fi
echo "==== 终局演练：达成（两端只照手册从零装完、自检先红后绿、经隧道跑通一次真实搬运并核对无误）===="
