#!/usr/bin/env python3
"""M3 渲染面走查（W1–W6）的桩后端。

**为什么不是台架**：W1–W6 原本复用 `run-m3-acceptance.sh` 的 B1–B6 造态，而 #121 换了
写入模型与任务定义之后，那三份台架的报文与判据都还是退役形态（改造归 #122），跑不起来。
渲染面的问题（「五列出来没有」「堆叠了没有」「占位符在不在」）不依赖真库，
所以这里用桩后端把六种态原样造出来，喂真实的 `web/dist` 构建产物。

**它不是验收替身**：只回答「渲染出来没有」，一个数据正确性问题都不回答。

用法：python3 docs/spikes/fixtures/local-rig/walkthrough/m3-mock.py [port]
"""

import json
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
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18099

SPEC = {
    "owner": "SPIKE",
    "table": "T_M3_B1",
    "target_table": "M3_B1",
    # ① 之后 columns 是 ColumnMapping（ADR-0038 §2）；走查只需恒等映射。
    "columns": [{"source": c, "target": c}
                for c in ("ROW_ID", "N_BARE", "V_TEXT", "PAYLOAD", "LOAD_DATE")],
    "primary_key": ["ROW_ID"],
    "where_clause": "LOAD_DATE = DATE '2026-08-18' AND V_TEXT > 'M3-'",
}

# 数据源（ADR-0037）：口令一个字都不回，只回 has_password。
DATASOURCES = [
    {"datasource_id": "ds-oracle", "name": "源库（走查）", "kind": "oracle",
     "connect_string": "//oracle:1521/XE", "username": "spike", "has_password": True},
    {"datasource_id": "ds-mysql", "name": "目标库（走查）", "kind": "mysql", "agent_id": "agent-a",
     "host": "127.0.0.1", "port": 3306, "username": "sink", "database": "qbs", "has_password": True},
]

# 见 v-mock.py 里同名常量：外壳把数据源与 agent 当成同一次读取（ADR-0044），
# 少了这个端点，数据源清单会跟着一起被判成读不到。
AGENTS = [
    {"agent_id": "agent-a", "name": "目标端 A", "base_url": "http://127.0.0.1:8080",
     "instance_id": "6f1a9c2d4e8b47f0a1b2c3d4e5f60718", "version": "0.1.0",
     "last_seen_at": "2026-08-24T02:00:00Z", "status": "online", "last_error": None},
]

TASK = {
    "task_id": "task-m3",
    "name": "M3 走查任务",
    "source_datasource_id": "ds-oracle",
    "target_datasource_id": "ds-mysql",
    "spec": SPEC,
}

BUILDER_TABLES = [
    {"owner": "SPIKE", "name": "T_M3_B1"},
    {"owner": "SPIKE", "name": "T_M3_B2"},
]

BUILDER_COLUMNS = [
    {"name": "ROW_ID", "data_type": "NUMBER", "precision": 8, "scale": 0, "length": None, "nullable": False},
    {"name": "N_BARE", "data_type": "NUMBER", "precision": None, "scale": None, "length": None, "nullable": True},
    {"name": "V_TEXT", "data_type": "VARCHAR2", "precision": None, "scale": None, "length": 200, "nullable": True},
    {"name": "PAYLOAD", "data_type": "CLOB", "precision": None, "scale": None, "length": None, "nullable": True},
    {"name": "LOAD_DATE", "data_type": "DATE", "precision": None, "scale": None, "length": None, "nullable": True},
]

# W3/W4：裸 NUMBER 待配精度 + BINARY_FLOAT 不支持，但整份 DDL 照给。
COLUMNS_OK = {
    "columns": [
        {"name": "ROW_ID", "type": "NUMBER", "precision": 8, "scale": 0, "length": None, "support": "ok"},
        {"name": "N_BARE", "type": "NUMBER", "precision": None, "scale": None, "length": None, "support": "needs_precision"},
        {"name": "V_TEXT", "type": "VARCHAR2", "precision": None, "scale": None, "length": 200, "support": "ok"},
        {"name": "TS3", "type": "TIMESTAMP", "precision": None, "scale": None, "length": None, "fsp": 3, "support": "ok"},
        {"name": "LOAD_DATE", "type": "DATE", "precision": None, "scale": None, "length": None, "support": "ok"},
    ],
    "target_ddl": (
        "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n"
        "-- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。\n"
        "-- N_BARE 列的精度 describe 给不出，请在取列面为它们配 (p,s)。\n"
        "CREATE TABLE `M3_B1` (\n"
        "  `ROW_ID` DECIMAL(8,0) NOT NULL,\n"
        "  `N_BARE` DECIMAL(<p>,<s>) NULL,\n"
        "  `V_TEXT` VARCHAR(200) NULL,\n"
        "  `TS3` DATETIME(3) NULL,\n"
        "  `LOAD_DATE` DATETIME(0) NULL,\n"
        "  PRIMARY KEY (`ROW_ID`)\n"
        ") DEFAULT CHARSET=utf8mb4;"
    ),
}

# W5：白名单外的列 —— 列表照给，只有 DDL 区块换成「整份不给」。
COLUMNS_REJECTED = {
    "kind": "target_ddl",
    "message": "2 column(s) cannot be expressed in the target table",
    "columns": [
        {"column": "PAYLOAD", "source": "CLOB", "message": "unsupported source type; narrow it in the source SQL or CAST it"},
        {"column": "BF", "source": "BINARY_FLOAT", "message": "unsupported source type; CAST it to NUMBER(p,s)"},
    ],
    "described_columns": [
        {"name": "ROW_ID", "type": "NUMBER", "precision": 8, "scale": 0, "length": None, "support": "ok"},
        {"name": "PAYLOAD", "type": "CLOB", "precision": None, "scale": None, "length": None, "support": "unsupported"},
        {"name": "BF", "type": "BINARY_FLOAT", "precision": None, "scale": None, "length": None, "support": "unsupported"},
        {"name": "LOAD_DATE", "type": "DATE", "precision": None, "scale": None, "length": None, "support": "ok"},
    ],
}

# W1/W6：映射预检失败 —— 逐列一条 + 值域校核那条混在同一张表里。
MAPPING_ISSUES = [
    {"column": "PAYLOAD", "source": "CLOB", "target": "<missing>", "rule": "目标表缺列", "message": None,
     "suggestion": "在目标表加列，或把该列从源 SQL 里去掉"},
    {"column": "V_TEXT", "source": "VARCHAR2(200)", "target": "VARCHAR(80)", "rule": "目标列过窄", "message": None,
     "suggestion": "把目标列放宽到 VARCHAR(200)"},
    {"column": "D_WRONG", "source": "DATE", "target": "VARCHAR(20)", "rule": "类型不兼容", "message": None,
     "suggestion": "把目标列改成 DATETIME(0)"},
    {"column": "N_TOO_WIDE", "source": "NUMBER(38,-30)", "target": "DECIMAL(65,30)", "rule": "超出 MySQL DECIMAL(65,30)",
     "message": None, "suggestion": "改源 SQL 或 CAST 收窄值域"},
    {"column": "N_MISSING", "source": "NUMBER", "target": "DECIMAL(10,2)", "rule": "裸 NUMBER 未声明精度",
     "message": None, "suggestion": "在取列面为该列配 (p,s)"},
    # W6：值域校核的不合规记录，与其余逐列规则同形，不另起区块。
    {"column": "N_BARE", "source": "NUMBER", "target": "DECIMAL(10,2)",
     "rule": "值域校核：3 行超出目标 DECIMAL(10,2)", "message": None,
     "suggestion": "调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域"},
]

RUN_HISTORY = {
    "run_record_id": "record-m3",
    "run_id": "20260818120000_a1b2c3",
    "task_id": "task-m3",
    "source_sql": "SELECT a.ROW_ID AS ROW_ID,\n       a.N_BARE AS N_BARE\n  FROM SPIKE.T_M3_B1 a\n WHERE a.LOAD_DATE = TO_DATE(:load_date,'YYYY-MM-DD')",
    "staging_table": None,
    "started_at": "2026-08-18T12:00:00.000Z",
    "finished_at": "2026-08-18T12:00:02.000Z",
    "outcome": "FAILED",
    "target_table_effect": None,
    "stage": "PREPARING",
    "source_rows": 0,
    "staged_rows": None,
    "sink_reported_rows": None,
    "purged_rows": None,
    "source_batches": 0,
    "received_batches": None,
    "fetch_ms": 0,
    "push_ms": 0,
    "commit_ms": 0,
    "count_ms": None,
    "cursor_ms": 1,
    "source_code": None,
    "sink_code": "PRECHECK_FAILED",
    "column": None,
    "value": None,
    "message": "目标端：映射预检未通过：一次发现 6 项问题，未创建暂存表",
    "failure_kind": "MAPPING_PRECHECK",
    "unknown_reason": None,
    "seq": 0,
    "rows_pushed": 0,
    "bytes": 0,
    "ms": 0,
    "last_ts": "2026-08-18T12:00:02.000Z",
    "mapping_issues": MAPPING_ISSUES,
    "live": False,
}


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

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/api/tasks":
            return self._send(200, [TASK])
        if path == "/api/datasources":
            return self._send(200, DATASOURCES)
        if path == "/api/agents":
            return self._send(200, AGENTS)
        if path.startswith("/api/runs/"):
            return self._send(200, RUN_HISTORY)
        if path == "/api/runs":
            return self._send(200, [RUN_HISTORY])
        return self._static(path)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length).decode() if length else "{}"
        path = self.path.split("?")[0]
        if path == "/api/builder/tables":
            return self._send(200, BUILDER_TABLES)
        if path == "/api/builder/columns":
            return self._send(200, BUILDER_COLUMNS)
        if path == "/api/builder/sql":
            spec = json.loads(raw)
            return self._send(200, {"source_sql": derive_sql(spec)})
        if path == "/api/columns":
            # 用目标表名切换 W3/W4 与 W5 两态，免得再开一个端口。
            spec = json.loads(raw).get("spec", {})
            if spec.get("target_table") == "REJECTED":
                return self._send(422, COLUMNS_REJECTED)
            return self._send(200, COLUMNS_OK)
        if path == "/api/runs":
            return self._send(202, {"run_record_id": "record-m3"})
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


def derive_sql(spec):
    """与 `task_spec.rs::source_sql` 同形——桩只需长得像，不需要是同一份实现。"""
    projection = ",\n       ".join(
        f"a.{c['source']} AS {c['target']}" for c in spec.get("columns", []))
    link = f"@{spec['dblink']}" if spec.get("dblink") else ""
    sql = f"SELECT {projection}\n  FROM {spec.get('owner')}.{spec.get('table')}{link} a"
    clause = (spec.get("where_clause") or "").strip()
    if clause:
        sql += "\n WHERE " + clause
    return sql


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
