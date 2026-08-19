#!/usr/bin/env python3
"""第一版渲染面走查（X1–X8）的桩后端。

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

TASKS = [
    {"task_id": "task-holding", "name": "持仓日明细",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
    {"task_id": "task-fa", "name": "财务凭证",
     "source_datasource_id": "ds-ora-fa", "target_datasource_id": "ds-my-dw",
     "spec": copy.deepcopy(SPEC)},
]


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
            return self._send(200, [])
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
