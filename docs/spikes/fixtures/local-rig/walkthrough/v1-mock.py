#!/usr/bin/env python3
"""第一版渲染面走查（X1–X12）的桩后端。

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
    {"datasource_id": "ds-my-dw", "name": "数仓 MySQL", "kind": "mysql",
     "host": "10.0.0.12", "port": 3306, "username": "sink", "database": "dw_stage", "has_password": True},
    {"datasource_id": "ds-my-mart", "name": "集市 MySQL", "kind": "mysql",
     "host": "10.0.0.13", "port": 3307, "username": "mart", "database": "dw_mart", "has_password": True},
    {"datasource_id": "ds-my-spare", "name": "备用 MySQL", "kind": "mysql",
     "host": "10.0.0.14", "port": 3306, "username": "spare", "database": "dw_spare", "has_password": False},
]

SPEC = {
    "owner": "APP",
    "table": "T_HOLDING",
    "target_table": "HOLDING",
    "primary_key": ["ID"],
    "columns": [{"source": c, "target": c} for c in ("ID", "C_NAME", "LOAD_DATE")],
    "conditions": [
        {"column": "LOAD_DATE", "operator": "eq", "value_type": "date",
         "parameter": "load_date", "value_source": "runtime", "constant": ""},
    ],
    "order_by": [],
}

# 财务凭证多一条「运行时填」的 `region`——X9 的预填三规则要它：
# 失败那行里有 `load_date`（取行值）、没有 `region`（留空）、多出一个 `legacy_region`（丢弃）。
FA_SPEC = copy.deepcopy(SPEC)
FA_SPEC["conditions"] = FA_SPEC["conditions"] + [
    {"column": "C_NAME", "operator": "eq", "value_type": "text",
     "parameter": "region", "value_source": "runtime", "constant": ""},
]

TASKS = [
    {"task_id": "task-holding", "name": "持仓日明细",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
    {"task_id": "task-fa", "name": "财务凭证",
     "source_datasource_id": "ds-ora-fa", "target_datasource_id": "ds-my-dw",
     "spec": FA_SPEC},
]


def run_row(**overrides):
    """一条运行历史行。字段照 `web/src/api.ts` 的 `RunHistory` 给全，缺一个前端就渲染不出来。"""
    row = {
        "run_record_id": "rec", "run_id": "run", "task_id": "task-holding",
        "run_params": {}, "source_sql": "SELECT a.ID AS ID\n  FROM APP.T_HOLDING a",
        "staging_table": "STG_1", "started_at": "2026-08-19T02:00:00Z",
        "finished_at": "2026-08-19T02:03:00Z", "outcome": "FAILED",
        "target_table_effect": "DISCARDED", "stage": "FAILED",
        "source_rows": 120, "staged_rows": 120, "sink_reported_rows": 118, "purged_rows": 0,
        "source_batches": 2, "received_batches": 2,
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


# X9 的五态各一行：可重跑两种（FAILED / 结局不明）、不给入口两种（进行中 / SUCCEEDED）、
# 禁用一种（任务已删除）。`rec-live-fa` 与 `rec-failed-fa` 同任务，且参数只差一个 `region`——
# 重跑时把 `region` 补成 `HZ`，就撞上「同一组参数可能已有 run 进行中」那条既有提示。
HISTORY = [
    run_row(run_record_id="rec-failed-fa", run_id="run-fa-7", task_id="task-fa",
            run_params={"load_date": "2026-08-18", "legacy_region": "SH"}),
    run_row(run_record_id="rec-live-fa", run_id="run-fa-8", task_id="task-fa",
            run_params={"load_date": "2026-08-18", "region": "HZ"},
            outcome=None, stage="STREAMING", finished_at=None, sink_code=None,
            target_table_effect=None, sink_reported_rows=None, staged_rows=None,
            started_at="2026-08-19T02:10:00Z", ms=42000),
    run_row(run_record_id="rec-unknown", run_id="run-h-5",
            run_params={"load_date": "2026-08-17"},
            outcome=None, stage=None, sink_code=None, target_table_effect=None,
            unknown_reason="PROCESS_DISAPPEARED", message=None, failure_kind="UNKNOWN"),
    run_row(run_record_id="rec-succeeded", run_id="run-h-4",
            run_params={"load_date": "2026-08-16"},
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=120, message="run completed successfully",
            failure_kind=None),
    run_row(run_record_id="rec-task-gone", run_id="run-x-1", task_id="task-removed",
            run_params={"load_date": "2026-08-15"}),
]


# `X_BULK=1`：把任务与运行历史各加量到 26 条，好让**客户端分页**（ADR-0042 §2）有对象。
# 默认关着——填充行会把 X1–X9 的实录塞满噪声，那几条要看的是具体那几个态。
# 填充行**只**用来数数与翻页，态一律取「成功」，不参与任何结局判定。
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
            task_id=task_id, run_params={"load_date": "2026-08-19"},
            outcome="SUCCEEDED", target_table_effect="SWAPPED", stage="COMMITTING",
            sink_code=None, sink_reported_rows=120, failure_kind=None,
            message="run completed successfully",
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
        if path == "/api/builder/tables":
            return self._send(200, BUILDER_TABLES)
        if path == "/api/builder/columns":
            return self._send(200, BUILDER_COLUMNS)
        if path == "/api/builder/sql":
            spec = self._read()
            projection = ",\n       ".join(
                f"a.{c['source']} AS {c['target']}" for c in spec.get("columns", [])
            )
            return self._send(200, {
                "source_sql": f"SELECT {projection}\n  FROM {spec.get('owner')}.{spec.get('table')} a",
                "run_parameters": [
                    {"parameter": c["parameter"], "column": c["column"], "value_type": c["value_type"]}
                    for c in spec.get("conditions", []) if c["value_source"] == "runtime"
                ],
            })
        if path == "/api/runs":
            # 重跑落地成的**新**记录：旧那条原样留在清单里，这里只往前插一条进行中的。
            body = self._read()
            new_id = f"rec-new-{len(HISTORY) + 1}"
            HISTORY.insert(0, run_row(
                run_record_id=new_id, run_id=None, task_id=body.get("task_id", ""),
                run_params=body.get("run_params", {}), outcome=None, stage=None,
                finished_at=None, sink_code=None, target_table_effect=None,
                staging_table=None, started_at="2026-08-19T02:20:00Z", ms=0,
                source_rows=None, staged_rows=None, sink_reported_rows=None,
                purged_rows=None, seq=0, rows_pushed=0, bytes=0, message=None,
                failure_kind=None))
            return self._send(202, {"run_record_id": new_id})
        if path == "/api/datasources":
            draft = self._read()
            entry = dict(draft)
            entry["datasource_id"] = f"ds-new-{len(DATASOURCES) + 1}"
            DATASOURCES.append(view_of(entry))
            return self._send(201, view_of(entry))
        return self._send(404, {"error": {"message": "not found"}})

    def do_PUT(self):
        path = self.path.split("?")[0]
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
