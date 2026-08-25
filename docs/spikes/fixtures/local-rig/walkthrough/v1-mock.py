#!/usr/bin/env python3
"""第一版渲染面走查（X1–X18）的桩后端。

**为什么不是台架**：清单写的是「复用 `run-v1-acceptance.sh` 的 C1–C5 造态」，
而那个入口归 #135，此刻还不存在；M1/M2/M3 三份也还是退役调用面（改造归 #134）。
数据源屏那几个态（测连失败 / 测连成功 / 删除被拒）不依赖真库，这里用桩把它们原样造出来，
喂真实的 `web/dist` 构建产物。

**它不是验收替身**：只回答「渲染出来没有」，一个数据正确性问题都不回答。

口令判定：`right` 通过，其余一律失败（造 X3 的两态）。

用法：python3 docs/spikes/fixtures/local-rig/walkthrough/v1-mock.py [port]
"""

import copy
import json
import os
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs

def _find_dist() -> Path:
    """向上找 `web/dist`——脚本挪过一次目录（`.playwright/` → `local-rig/walkthrough/`），
    数目录层数的写法当场失效，改成沿父链找，以后再挪也不用改。"""
    for parent in Path(__file__).resolve().parents:
        candidate = parent / "web" / "dist"
        if candidate.is_dir():
            return candidate
    raise SystemExit("找不到 web/dist——先在仓库根跑 `npm run build`")


DIST = _find_dist()
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18098

GOOD_PASSWORD = "right"

DATASOURCES = [
    {"datasource_id": "ds-ora-core", "name": "生产核心库", "kind": "oracle",
     "connect_string": "//oracle-core:1521/ORCLPDB", "username": "app_reader", "has_password": True},
    {"datasource_id": "ds-ora-fa", "name": "财务库", "kind": "oracle",
     "connect_string": "//oracle-fa:1521/FAPDB", "username": "fa_reader", "has_password": True},
    {"datasource_id": "ds-my-dw", "name": "数仓 MySQL", "kind": "mysql", "agent_id": "agent-a",
     "host": "10.0.0.12", "port": 3306, "username": "sink", "database": "dw_stage", "has_password": True},
    # 这一条故意绑在**不在线**的那台 agent 上：数据源屏「目标端 Agent」列的告警态要有对象（X19）。
    {"datasource_id": "ds-my-mart", "name": "集市 MySQL", "kind": "mysql", "agent_id": "agent-b",
     "host": "10.0.0.13", "port": 3307, "username": "mart", "database": "dw_mart", "has_password": True},
    {"datasource_id": "ds-my-spare", "name": "备用 MySQL", "kind": "mysql", "agent_id": "agent-a",
     "host": "10.0.0.14", "port": 3306, "username": "spare", "database": "dw_spare", "has_password": False},
]

# 目标端 agent 注册表（ADR-0044）。三档状态各摆一台——X19 看的就是这三行同屏：
# 在线 / 不在线 / 身份不符，且后两者各自带着自己的原因。
AGENTS = [
    {"agent_id": "agent-a", "name": "目标端 A", "base_url": "http://127.0.0.1:8080",
     "instance_id": "6f1a9c2d4e8b47f0a1b2c3d4e5f60718", "version": "0.1.0",
     "last_seen_at": "2026-08-24T02:00:00Z", "status": "online", "last_error": None},
    {"agent_id": "agent-b", "name": "目标端 B（灾备）", "base_url": "http://127.0.0.1:8081",
     "instance_id": "b21c7e0f5a934d18c0d2e3f405162738", "version": "0.1.0",
     "last_seen_at": "2026-08-23T18:41:00Z", "status": "offline",
     "last_error": "连不上 agent：Connection refused (os error 111)"},
    {"agent_id": "agent-c", "name": "目标端 C（迁移中）", "base_url": "http://127.0.0.1:8082",
     "instance_id": "c33d8f1a6b045e29d1e3f4a516273849", "version": "0.1.0",
     "last_seen_at": "2026-08-22T09:12:00Z", "status": "mismatch",
     "last_error": "这个地址上应答的是另一台 agent（注册时钉的是 c33d8f1a…，现在应答的是 99aa77bb…）"},
]

SPEC = {
    "owner": "APP",
    "table": "T_HOLDING",
    "target_table": "HOLDING",
    "primary_key": ["ID"],
    "columns": [{"source": c, "target": c} for c in ("ID", "C_NAME", "LOAD_DATE")],
    "where_clause": "LOAD_DATE = DATE '2026-08-19'",
}

# 财务凭证的 WHERE 片段写得**长一点、形态也更凶**：`>=`、`IN`、多行——
# 这些正是四格表单永远表达不出来、因而逼人绕道自定义 SQL 的那批条件。
FA_SPEC = copy.deepcopy(SPEC)
FA_SPEC["where_clause"] = (
    "LOAD_DATE >= DATE '2026-08-01'\n  AND C_NAME IN ('SH', 'HZ')"
)

# 自定义 SQL 取数的规格：`owner` / `table` **都是空串**，取数靠 `source_sql`。
# 作业中心「源表」那一格原来直接拼 `owner.table`，于是这类任务渲染成一个孤零零的
# `.`，搜索索引里也只剩这个点（按源表关键字永远搜不到）。X8 / X14 要这一态才看得见。
SQL_SPEC = {
    "owner": "",
    "table": "",
    "source_sql": "SELECT *\n  FROM APP.T_HOLDING@POC_LINK_A\n WHERE STATUS = 1",
    "target_table": "HOLDING",
    "primary_key": ["ID"],
    "columns": [{"source": c, "target": c} for c in ("ID", "C_NAME", "LOAD_DATE")],
    "where_clause": "",
}

# 作业中心一行 = 一个任务 + 它最近一次运行（ADR-0043 §2），所以**五种运行状态要靠五个任务摆出来**，
# 而不是像原来那样靠运行历史屏的五行。X17 看的是这五个词同屏、X16 看的是进度的三种空态。
TASKS = [
    {"task_id": "task-holding", "name": "持仓日明细",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
    {"task_id": "task-fa", "name": "财务凭证",
     "source_datasource_id": "ds-ora-fa", "target_datasource_id": "ds-my-dw",
     "spec": FA_SPEC},
    {"task_id": "task-verify", "name": "结算对账",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-mart",
     "spec": copy.deepcopy(SPEC)},
    {"task_id": "task-unknown", "name": "客户主数据",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
    {"task_id": "task-nocount", "name": "交易流水（计数失败）",
     "source_datasource_id": "ds-ora-fa", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
    # 从没跑过的那一个：进度列该是 `—`（不是 0%）、状态该是「尚未运行」、
    # 行内「运行详情」该是禁用态（X9 前半 / X16 / X17 都要它）。
    {"task_id": "task-never", "name": "产品维表",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-spare",
     "spec": copy.deepcopy(SPEC)},
    # 自定义 SQL 那一个。摆在最后，前面五种运行状态的取样对象一个都不动。
    {"task_id": "task-sqlmode", "name": "客户订单增量",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SQL_SPEC)},
]


def run_row(**overrides):
    """一条运行历史行。字段照 `web/src/api.ts` 的 `RunHistory` 给全，缺一个前端就渲染不出来。"""
    row = {
        "run_record_id": "rec", "run_id": "run", "task_id": "task-holding",
        "source_sql": "SELECT a.ID AS ID\n  FROM APP.T_HOLDING a",
        "staging_table": "STG_1", "started_at": "2026-08-19T02:00:00Z",
        "finished_at": "2026-08-19T02:03:00Z", "outcome": "FAILED",
        "target_table_effect": "DISCARDED", "stage": "FAILED",
        "source_rows": 120, "staged_rows": 120, "sink_reported_rows": 118, "purged_rows": 0,
        "source_batches": 2, "received_batches": 2,
        # `total_rows` 是**开跑前** `COUNT(*)` 拿到的分母，`precount_ms` 是那一次计数自己的耗时
        # （ADR-0043 §7）。两者与 sink 侧的门禁计数 `count_ms` 是两回事，别混。
        # `total_rows` 为 None = 那次计数没成功——运行照跑，进度列退回 `—`，X16 要这一态。
        "total_rows": 120, "precount_ms": 180,
        "fetch_ms": 900, "push_ms": 1400, "commit_ms": 120, "count_ms": 60, "cursor_ms": 20,
        "source_code": None, "sink_code": "VERIFY_FAILED", "column": None, "value": None,
        "message": "目标端：门禁计数不一致：源端 120 行，目标端报 118 行",
        "failure_kind": "VERIFY_FAILED", "unknown_reason": None,
        "seq": 2, "rows_pushed": 120, "bytes": 40960,
        "last_ts": "2026-08-19T02:03:00Z", "mapping_issues": [],
    }
    row.update(overrides)
    row["ms"] = overrides.get("ms", 180000)
    return row


# 五个任务各一条最近运行，凑齐作业中心的五个状态词；外加进度那三种空态。
# `rec-live-fa` 与 `rec-failed-fa` 同任务：前者至今在飞，于是再点一次这一行的「发起运行」
# 就撞上 409「该任务已有一次运行进行中」——那句话落在屏顶的横幅里（X9 后半）。
#
# 进度分母（`total_rows`）故意摆成四种：
#   task-verify  11998 / 12000 = 99.983% → **必须显示 99%**，向下取整这条就靠它（X16）
#   task-fa        430 / 1200          → 进行中也有真百分比，不是不确定进度条
#   task-unknown   900 / 1200          → 结局不明那行的进度停在最后一次上报
#   task-nocount  total_rows=None      → 进度 `—` + title 自陈「未取到总行数」，但运行照常跑完
#
# 注意 `task-fa` 的**最近一次**是那条进行中的 run，`rec-failed-fa` 只是它的上一次——
# 上一次不上列表（ADR-0043 §2 只展示最近一次），它留在库里是给 X9 的重跑用的。
# 因此 99% 那一格必须挂在**另一个任务**（`task-verify`）上，否则这一态在屏上根本不出现。
HISTORY = [
    run_row(run_record_id="rec-failed-fa", run_id="run-fa-7", task_id="task-fa"),
    # 99.983% —— 这一格必须显示 99%，四舍五入成 100% 等于拿显示撒谎（ADR-0043 §7 边界 1）。
    run_row(run_record_id="rec-verify", run_id="run-vf-1", task_id="task-verify",
            started_at="2026-08-19T02:20:00Z", finished_at="2026-08-19T02:20:15Z", ms=15000,
            total_rows=12000, rows_pushed=11998, source_rows=11998, staged_rows=11998,
            sink_reported_rows=11975, precount_ms=1806),
    # 并跑拦截那一态挂在 `task-fa` 上：它名下的 `rec-live-fa` 至今在飞，
    # 于是第二次点它的「发起运行」当场换回 409（X9 后半）。
    # `task-verify` 名下**只留失败那一条**——它是「重跑」那一态的对象，
    # 名下再挂一条在飞的 run，重跑就会被 409 挡住，那条判据也就没了对象。
    run_row(run_record_id="rec-live-fa", run_id="run-fa-8", task_id="task-fa",
            outcome=None, stage="STREAMING", finished_at=None, sink_code=None,
            target_table_effect=None, sink_reported_rows=None, staged_rows=None,
            started_at="2026-08-19T02:10:00Z", ms=42000,
            total_rows=1200, rows_pushed=430, source_rows=430),
    run_row(run_record_id="rec-unknown", run_id="run-h-5", task_id="task-unknown",
            outcome=None, stage=None, sink_code=None, target_table_effect=None,
            unknown_reason="PROCESS_DISAPPEARED", message=None, failure_kind="UNKNOWN",
            finished_at=None, total_rows=1200, rows_pushed=900, source_rows=900),
    run_row(run_record_id="rec-succeeded", run_id="run-h-4", task_id="task-holding",
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=120, message="run completed successfully",
            failure_kind=None, total_rows=120, rows_pushed=120),
    # 开跑前那次 COUNT(*) 没成功：分母缺席，但这次运行**照样跑完并成功**——
    # 「为了一个进度条把整次搬运判死」正是 ADR-0043 §7 边界 3 要挡的事。
    run_row(run_record_id="rec-nocount", run_id="run-nc-1", task_id="task-nocount",
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=120, message="run completed successfully",
            failure_kind=None, total_rows=120, rows_pushed=120, precount_ms=None),
    # 自定义 SQL 那个任务也得有一次运行，否则「运行详情」是禁用态、抽屉打不开——
    # X18 改判后要看的正是抽屉里「任务定义 · 源表」那一格（ADR-0045 §走查触发），
    # 没有这条运行，那条判据就没有对象。
    #
    # 五种运行状态的取样对象一个不动：这一条是**第二个**成功态，
    # `task-never` 仍然是唯一的「尚未运行」。
    #
    # `source_sql` 存的是**包裹之后的完整语句**——运行历史钉的是当时真执行的那一份，
    # 而自定义 SQL 从不原样执行（ADR-0045 §1）。
    run_row(run_record_id="rec-sqlmode", run_id="run-sq-1", task_id="task-sqlmode",
            source_sql=(
                "SELECT q.ID AS ID,\n"
                "       q.C_NAME AS C_NAME,\n"
                "       q.LOAD_DATE AS LOAD_DATE\n"
                "  FROM (\n"
                "         SELECT *\n"
                "           FROM APP.T_HOLDING@POC_LINK_A\n"
                "          WHERE STATUS = 1\n"
                "       ) q"
            ),
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=64, message="run completed successfully",
            failure_kind=None, total_rows=64, rows_pushed=64),
]


# `X_BULK=1`：把任务与运行历史各加量到 26 条以上，好让**客户端分页**（ADR-0042 §2、X11）
# 与**跨页全选**（ADR-0043 §6、X15：表头全选只选当前页，翻页不跟着跑）有对象。
# 默认关着——填充行会把 X1–X9 的实录塞满噪声，那几条要看的是具体那几个态。
# 填充行**只**用来数数、翻页、勾选，态一律取「成功」、进度一律 100%，不参与任何结局判定。
if os.environ.get("X_BULK") == "1":
    for index in range(24):
        task_id = f"task-bulk-{index:02d}"
        TASKS.append({
            "task_id": task_id, "name": f"批量任务 {index:02d}",
            "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
            "spec": copy.deepcopy(SPEC),
        })
        HISTORY.append(run_row(
            run_record_id=f"rec-bulk-{index:02d}", run_id=f"run-bulk-{index:02d}",
            task_id=task_id,
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=120, failure_kind=None,
            message="run completed successfully", total_rows=120, rows_pushed=120,
        ))


def referencing(datasource_id):
    return [
        task["name"]
        for task in TASKS
        if datasource_id in (task["source_datasource_id"], task["target_datasource_id"])
    ]


def view_of(entry):
    """写入面 → 读出面：**口令一个字都不回，连密文都不回**（ADR-0037 §5）。"""
    view = {k: v for k, v in entry.items() if k != "password"}
    view["has_password"] = entry.get("password", "") != ""
    return view


# 目标端元数据面（ADR-0038 §3）。HOLDING 表里 CREATE_TIME 与 ROW_NO 故意不被映射：
# 前者非空有默认值（预检放行）、后者 auto_increment，用来看「未映射整行压暗」。
TARGET_TABLES = ["HOLDING", "HOLDING_DAILY", "CUSTOMER", "ORDER_ITEM", "AUDIT_LOG"]

TARGET_COLUMNS = {
    "columns": [
        {"name": "ID", "column_type": "bigint(20)", "data_type": "bigint", "precision": 19,
         "scale": 0, "length": None, "datetime_precision": None, "nullable": False,
         "character_set": None, "ordinal": 1, "default_value": None, "extra": ""},
        {"name": "C_NAME", "column_type": "varchar(200)", "data_type": "varchar", "precision": None,
         "scale": None, "length": 200, "datetime_precision": None, "nullable": True,
         "character_set": "utf8mb4", "ordinal": 2, "default_value": None, "extra": ""},
        {"name": "LOAD_DATE", "column_type": "datetime", "data_type": "datetime", "precision": None,
         "scale": None, "length": None, "datetime_precision": 0, "nullable": True,
         "character_set": None, "ordinal": 3, "default_value": None, "extra": ""},
        {"name": "CREATE_TIME", "column_type": "datetime", "data_type": "datetime", "precision": None,
         "scale": None, "length": None, "datetime_precision": 0, "nullable": False,
         "character_set": None, "ordinal": 4, "default_value": "CURRENT_TIMESTAMP",
         "extra": "DEFAULT_GENERATED"},
        {"name": "ROW_NO", "column_type": "int(11)", "data_type": "int", "precision": 10,
         "scale": 0, "length": None, "datetime_precision": None, "nullable": False,
         "character_set": None, "ordinal": 5, "default_value": None, "extra": "auto_increment"},
    ],
    "keys": [
        {"name": "PRIMARY", "columns": ["ID"]},
        {"name": "u_code", "columns": ["C_NAME"]},
    ],
}

BUILDER_DBLINKS = ["POC_LINK_A", "FA", "ERP_PROD", "HR_LINK"]

# `/api/builder/sql-columns` 回的是 `FetchedColumn`（`type` 而不是 `data_type`），
# 且**不带可空**——describe 一条 SELECT 拿不到 nullability，界面上那一栏因此是「—」。
SQL_COLUMNS = [
    {"name": "ID", "type": "NUMBER", "precision": 10, "scale": 0, "length": None},
    {"name": "C_NAME", "type": "VARCHAR2", "precision": None, "scale": None, "length": 200},
    {"name": "LOAD_DATE", "type": "DATE", "precision": None, "scale": None, "length": None},
    {"name": "N_AMT", "type": "NUMBER", "precision": 18, "scale": 2, "length": None},
]

BUILDER_TABLES = [{"owner": "APP", "name": "T_HOLDING"}, {"owner": "APP", "name": "T_CUSTOMER"}]

BUILDER_COLUMNS = [
    {"name": "ID", "data_type": "NUMBER", "precision": 19, "scale": 0, "length": None, "nullable": False},
    {"name": "C_NAME", "data_type": "VARCHAR2", "precision": None, "scale": None, "length": 200, "nullable": True},
    {"name": "LOAD_DATE", "data_type": "DATE", "precision": None, "scale": None, "length": None, "nullable": True},
    {"name": "N_AMT", "data_type": "NUMBER", "precision": 12, "scale": 2, "length": None, "nullable": True},
]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read(self):
        length = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(length).decode() or "{}")

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/api/datasources":
            # 一个数据源都没有的态（ADR-0039 §8 的引导）：造它只需要把清单清空。
            if os.environ.get("V1_MOCK_EMPTY_DATASOURCES") == "1":
                return self._send(200, [])
            return self._send(200, DATASOURCES)
        if path == "/api/agents":
            return self._send(200, AGENTS)
        if path == "/api/tasks":
            return self._send(200, TASKS)
        if path == "/api/runs":
            task_id = None
            if "?" in self.path:
                task_id = parse_qs(self.path.split("?", 1)[1]).get("task_id", [None])[0]
            rows = [r for r in HISTORY if task_id in (None, "", r["task_id"])]
            return self._send(200, rows)
        if path.startswith("/api/runs/"):
            wanted = path.rsplit("/", 1)[-1]
            for row in HISTORY:
                if row["run_record_id"] == wanted:
                    return self._send(200, dict(row, live=row["outcome"] is None
                                                and row["unknown_reason"] is None))
            return self._send(404, {"error": {"message": "run not found"}})
        return self._static(path)

    def do_POST(self):
        path = self.path.split("?")[0]
        if path == "/api/datasources/test-connection":
            draft = self._read()
            password = draft.get("password", "")
            if password == "" and draft.get("datasource_id"):
                password = GOOD_PASSWORD  # 编辑态留空 = 用库里那份（这里的桩当它是对的）
            if password != GOOD_PASSWORD:
                message = (
                    "ORA-01017: invalid username/password; logon denied"
                    if draft.get("kind") == "oracle"
                    else "Access denied for user 'sink'@'10.0.0.9' (using password: YES)"
                )
                return self._send(400, {"error": {"message": message}})
            label = (
                draft.get("connect_string")
                if draft.get("kind") == "oracle"
                else draft.get("database")
            )
            return self._send(200, {"ok": True, "elapsed_ms": 186, "label": label})
        if path == "/api/target/tables":
            return self._send(200, {"tables": TARGET_TABLES})
        if path == "/api/target/columns":
            body = self._read()
            # 表不存在 → **空清单，不是错误**（ADR-0038 §9）。
            if body.get("target_table") not in TARGET_TABLES:
                return self._send(200, {"columns": [], "keys": []})
            return self._send(200, TARGET_COLUMNS)
        if path == "/api/builder/dblinks":
            # 源端 DBLINK 的自动发现（builder 屏新增）。桩不查库，给一组固定名字即可——
            # 走查只回答「下拉里有没有东西、badge 数对不对」。
            return self._send(200, BUILDER_DBLINKS)
        if path == "/api/builder/sql-columns":
            # 自定义 SQL 的结果列 describe。桩只判「像不像一条 SELECT」，
            # 不像就回 400，把 X 走查里那个报错态也造出来。
            source_sql = (self._read().get("source_sql") or "").strip()
            if not source_sql.lower().startswith("select"):
                return self._send(
                    400, {"error": {"message": "自定义 SQL 必须是一条只读 SELECT"}}
                )
            return self._send(200, SQL_COLUMNS)
        if path == "/api/builder/tables":
            return self._send(200, BUILDER_TABLES)
        if path == "/api/builder/columns":
            return self._send(200, BUILDER_COLUMNS)
        if path == "/api/builder/sql":
            spec = self._read()
            # 桩要能造出**报错态**：这条 400 是构建器里的报错通道，走查得看得见它。
            # （原注释写「自定义 SQL 模式下『构建 SQL』卡不再渲染」——那已经反了，
            #  ADR-0045 §6 判两种模式都渲染预览，因为用户的 SQL 不是原样执行的。）
            # 只镜像 `TaskSpec::validate()` 里前端能违反的那两条，不复刻整个校验。
            columns = spec.get("columns", [])
            seen = set()
            for mapping in columns:
                for what, name in (("column", mapping["source"]), ("target column", mapping["target"])):
                    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_$#]*", name or ""):
                        return self._send(400, {"error": {"message":
                            f"{what} {name!r} 必须是未加引号的 Oracle 标识符"}})
                if mapping["target"].upper() in seen:
                    return self._send(400, {"error": {"message":
                        f"目标字段 {mapping['target']} 重复"}})
                seen.add(mapping["target"].upper())
            source_sql = (spec.get("source_sql") or "").strip()
            if source_sql:
                # 自定义 SQL 不原样执行：外层套一层投影，内层原文一字不动。
                projection = ",\n       ".join(
                    f"q.{c['source']} AS {c['target']}" for c in columns
                )
                inner = "\n".join("         " + line for line in source_sql.splitlines())
                built = f"SELECT {projection}\n  FROM (\n{inner}\n       ) q"
            else:
                projection = ",\n       ".join(
                    f"a.{c['source']} AS {c['target']}" for c in columns
                )
                built = f"SELECT {projection}\n  FROM {spec.get('owner')}.{spec.get('table')} a"
                # 过滤片段原样拼进 WHERE 后面，一个字符不加不改——与 `TaskSpec::source_sql` 同形。
                clause = (spec.get("where_clause") or "").strip()
                if clause:
                    built += "\n WHERE " + clause
            return self._send(200, {"source_sql": built})
        if path == "/api/runs":
            # 重跑落地成的**新**记录：旧那条原样留在清单里，这里只往前插一条进行中的。
            body = self._read()
            task_id = body.get("task_id", "")
            # 互斥键退化成了任务本身：这个任务名下还有一条在飞，就当场 409。
            # 发起面上没有对话框可以预警了，那句话只剩屏顶的横幅一个落点（X9）。
            if any(row["task_id"] == task_id and row["finished_at"] is None
                   and row["outcome"] is None and row.get("unknown_reason") is None
                   for row in HISTORY):
                return self._send(409, {
                    "error": {"message": "该任务已有一次运行进行中"}})
            new_id = f"rec-new-{len(HISTORY) + 1}"
            HISTORY.insert(0, run_row(
                run_record_id=new_id, run_id=None, task_id=task_id,
                outcome=None, stage=None,
                finished_at=None, sink_code=None, target_table_effect=None,
                staging_table=None, started_at="2026-08-19T02:20:00Z", ms=0,
                source_rows=None, staged_rows=None, sink_reported_rows=None,
                purged_rows=None, seq=0, rows_pushed=0, bytes=0, message=None,
                failure_kind=None))
            return self._send(202, {"run_record_id": new_id})
        if path == "/api/tasks":
            # 建任务：桩原样收下 spec 并落进清单。走查要的是「建完能不能再打开编辑」
            # 这条回路（自定义 SQL 的规格得原样转一圈回来），不是服务端校验。
            draft = self._read()
            task = {
                "task_id": f"task-new-{len(TASKS) + 1}",
                "name": draft.get("name", ""),
                "source_datasource_id": draft.get("source_datasource_id"),
                "target_datasource_id": draft.get("target_datasource_id"),
                "spec": draft.get("spec", {}),
            }
            TASKS.append(task)
            return self._send(201, task)
        if path == "/api/datasources":
            draft = self._read()
            entry = dict(draft)
            entry["datasource_id"] = f"ds-new-{len(DATASOURCES) + 1}"
            DATASOURCES.append(view_of(entry))
            return self._send(201, view_of(entry))
        if path == "/api/agents":
            # **注册要求对方活着**（ADR-0044 §3）：桩按端口造两态——8080 通，其余一律不通。
            # 造的是「探不通就不落库」这条判定本身，不是网络。
            draft = self._read()
            base_url = (draft.get("base_url") or "").rstrip("/")
            if not base_url.startswith("http://"):
                return self._send(400, {"error": {
                    "message": "agent 地址必须是 http://（TLS 由部署者自建的隧道提供）"}})
            if not base_url.endswith(":8080"):
                return self._send(502, {"kind": "agent", "message":
                    f"连不上这个地址上的目标端 agent（{base_url}）：连不上 agent：Connection refused (os error 111)",
                    "error": {"message":
                        f"连不上这个地址上的目标端 agent（{base_url}）：连不上 agent：Connection refused (os error 111)"}})
            entry = {
                "agent_id": f"agent-new-{len(AGENTS) + 1}",
                "name": draft.get("name") or "target-host",
                "base_url": base_url,
                "instance_id": "d44e9a2b7c156f3ae2f4a5b627384950",
                "version": "0.1.0", "last_seen_at": "2026-08-24T02:05:00Z",
                "status": "online", "last_error": None,
            }
            AGENTS.append(entry)
            return self._send(201, entry)
        if path.startswith("/api/agents/") and path.endswith("/probe"):
            agent_id = path[len("/api/agents/"):-len("/probe")]
            for entry in AGENTS:
                if entry["agent_id"] == agent_id:
                    # 探测**失败也回 200**：结果本身就是要显示的信息（ADR-0044 §6）。
                    return self._send(200, entry)
            return self._send(404, {"error": {"message": "agent not found"}})
        return self._send(404, {"error": {"message": "not found"}})

    def do_PUT(self):
        path = self.path.split("?")[0]
        if path.startswith("/api/agents/"):
            agent_id = path.rsplit("/", 1)[-1]
            draft = self._read()
            for index, entry in enumerate(AGENTS):
                if entry["agent_id"] == agent_id:
                    updated = dict(entry, name=draft.get("name") or entry["name"],
                                   base_url=(draft.get("base_url") or entry["base_url"]).rstrip("/"),
                                   status="online", last_error=None)
                    AGENTS[index] = updated
                    return self._send(200, updated)
            return self._send(404, {"error": {"message": "agent not found"}})
        if path.startswith("/api/tasks/"):
            task_id = path.rsplit("/", 1)[-1]
            draft = self._read()
            for index, entry in enumerate(TASKS):
                if entry["task_id"] == task_id:
                    TASKS[index] = dict(entry, name=draft.get("name", entry["name"]),
                                        spec=draft.get("spec", entry["spec"]))
                    return self._send(200, TASKS[index])
            return self._send(404, {"error": {"message": "task not found"}})
        if path.startswith("/api/datasources/"):
            datasource_id = path.rsplit("/", 1)[-1]
            draft = self._read()
            for index, entry in enumerate(DATASOURCES):
                if entry["datasource_id"] == datasource_id:
                    updated = dict(entry)
                    updated["name"] = draft.get("name", entry["name"])
                    DATASOURCES[index] = updated
                    return self._send(200, updated)
        return self._send(404, {"error": {"message": "not found"}})

    def do_DELETE(self):
        path = self.path.split("?")[0]
        if path.startswith("/api/agents/"):
            agent_id = path.rsplit("/", 1)[-1]
            bound = [d["name"] for d in DATASOURCES
                     if d.get("kind") == "mysql" and d.get("agent_id") == agent_id]
            if bound:
                # 与「数据源被任务引用」那条 409 同一形态（ADR-0044 §6）。
                return self._send(409, {"error": {
                    "message": f"这台 agent 仍被 {len(bound)} 条数据源引用；请先改这些数据源绑定的 agent",
                    "datasources": bound,
                }})
            for index, entry in enumerate(AGENTS):
                if entry["agent_id"] == agent_id:
                    return self._send(200, AGENTS.pop(index))
            return self._send(404, {"error": {"message": "agent not found"}})
        if path.startswith("/api/datasources/"):
            datasource_id = path.rsplit("/", 1)[-1]
            names = referencing(datasource_id)
            if names:
                # ADR-0037 §7 的 409，报文里**点名列出**是哪几个任务（ADR-0039 §4）。
                return self._send(409, {"error": {
                    "message": f"数据源仍被 {len(names)} 个任务引用：{'、'.join(names)}；请先改这些任务的数据源",
                    "tasks": names,
                }})
            for index, entry in enumerate(DATASOURCES):
                if entry["datasource_id"] == datasource_id:
                    return self._send(200, DATASOURCES.pop(index))
        return self._send(404, {"error": {"message": "not found"}})

    def _static(self, path):
        relative = "index.html" if path in ("/", "") else path.lstrip("/")
        target = DIST / relative
        if not target.is_file():
            target = DIST / "index.html"
        body = target.read_bytes()
        kinds = {".html": "text/html", ".js": "text/javascript", ".css": "text/css"}
        self.send_response(200)
        self.send_header("Content-Type", kinds.get(target.suffix, "application/octet-stream"))
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
