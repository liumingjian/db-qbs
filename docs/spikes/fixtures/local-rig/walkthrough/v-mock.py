#!/usr/bin/env python3
"""整份 V1–V25 走查的桩后端（#133）。

**为什么不是台架**：V 系列原本靠 `M2_KEEP_RIG=1 run-m2-acceptance.sh` 留下的实例造态，
而那三份台架此刻还是退役调用面（改造归 #134），起不来。渲染面的问题
（「三个圆点还是五个」「终态块出没出」「占位符在不在」）不依赖真库，
这里用桩把各态原样造出来，喂真实的 `web/dist` 构建产物。

**它不是验收替身**：只回答「渲染出来没有」，一个数据正确性问题都不回答。

**造态入口是发起对话框里的业务日期**：`2026-01-0X` 一个日期对应一个 run 态，
见 `RUNS_BY_DATE`。这样走查不必改产品代码，也不必起真库。

用法：python3 docs/spikes/fixtures/local-rig/walkthrough/v-mock.py [port]
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
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18097

DATASOURCES = [
    {"datasource_id": "ds-ora-core", "name": "生产核心库", "kind": "oracle",
     "connect_string": "//oracle-core:1521/ORCLPDB", "username": "app_reader",
     "has_password": True},
    {"datasource_id": "ds-my-dw", "name": "数仓 MySQL", "kind": "mysql",
     "host": "10.0.0.12", "port": 3306, "username": "sink", "database": "dw_stage",
     "has_password": True},
]

SPEC = {
    "owner": "APP",
    "table": "T_HOLDING",
    "target_table": "HOLDING",
    "primary_key": ["ID"],
    "columns": [{"source": c, "target": c} for c in ("ID", "C_NAME", "LOAD_DATE")],
    "column_precision": {},
    "conditions": [
        {"column": "LOAD_DATE", "operator": "eq", "value_type": "date",
         "parameter": "load_date", "value_source": "runtime", "constant": ""},
    ],
    "order_by": [],
}

# 2026-08-21（ADR-0043 §2）：运行历史独立屏并进作业中心，**一行 = 一个任务 + 它最近一次运行**。
# 于是「同一屏上同时看到 SWAPPED 与 DISCARDED」这件事不能再靠一个任务的多条历史，
# 得靠**多个任务各一条**。每条运行记录因此各挂一个任务，见 `RUNS` 里的 `task_id`。
# `task-holding` 留在第一位：`start_run()` 点的是第一行的「发起运行」，
# 而 V16 的并跑提示要它名下那条进行中的 run。
TASKS = [
    {"task_id": "task-holding", "name": "持仓日明细",
     "source_datasource_id": "ds-ora-core", "target_datasource_id": "ds-my-dw",
     "spec": SPEC},
    {"task_id": "task-success", "name": "成功那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
    {"task_id": "task-verify", "name": "校验失败那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
    {"task_id": "task-mapping", "name": "映射预检失败那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
    {"task_id": "task-unknown", "name": "结局不明那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
    {"task_id": "task-escape", "name": "哨兵逃逸那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
    {"task_id": "task-not-started", "name": "未发起那条", "source_datasource_id": "ds-ora-core",
     "target_datasource_id": "ds-my-dw", "spec": SPEC},
]

SOURCE_SQL = (
    "SELECT a.ID AS ID,\n"
    "       a.C_NAME AS C_NAME,\n"
    "       a.LOAD_DATE AS LOAD_DATE\n"
    "  FROM APP.T_HOLDING a\n"
    " WHERE a.LOAD_DATE = TO_DATE(:load_date,'YYYY-MM-DD')"
)

MAPPING_ISSUES = [
    {"column": "C_NAME", "source": "VARCHAR2(200)", "target": "VARCHAR(80)",
     "rule": "目标列过窄", "message": None, "suggestion": "把目标列放宽到 VARCHAR(200)"},
    {"column": "LOAD_DATE", "source": "DATE", "target": "VARCHAR(20)",
     "rule": "类型不兼容", "message": None, "suggestion": "把目标列改成 DATETIME(0)"},
    {"column": "ROW_NO", "source": "（未映射）", "target": "int(11) NOT NULL",
     "rule": "未映射且不允许留空", "message": None,
     "suggestion": "目标表的 ROW_NO 列未被映射且不允许留空，请映射它或给它默认值"},
]


def base_run(**over):
    """一条运行记录的底子；各态只覆盖自己那几个字段。"""
    row = {
        "run_record_id": "rec-x",
        "run_id": "20260819120000_aaaaaa",
        "task_id": "task-holding",
        "run_params": {"load_date": "2026-01-01"},
        "source_sql": SOURCE_SQL,
        "staging_table": "HOLDING__stg_20260819120000",
        "started_at": "2026-08-19T12:00:00.000Z",
        "finished_at": "2026-08-19T12:03:20.000Z",
        "outcome": "SUCCEEDED",
        "target_table_effect": "SWAPPED",
        "stage": None,
        "source_rows": 100000,
        "staged_rows": 100000,
        "sink_reported_rows": 100000,
        "purged_rows": 0,
        "source_batches": 20,
        "received_batches": 20,
        "fetch_ms": 1200,
        "push_ms": 8000,
        "commit_ms": 300,
        "count_ms": 40,
        "cursor_ms": 12,
        "source_code": None,
        "sink_code": None,
        "column": None,
        "value": None,
        "message": "run completed successfully",
        "failure_kind": None,
        "unknown_reason": None,
        "seq": 20,
        "rows_pushed": 100000,
        "bytes": 4589312,
        "last_ts": "2026-08-19T12:03:20.000Z",
        "ms": 9540,
        "total_rows": 100000,
        "precount_ms": 640,
        "mapping_issues": [],
        "live": False,
    }
    row.update(over)
    return row


# ---- 各态 -------------------------------------------------------------------
# V1 / V17：进行中，停在 STREAMING。
LIVE_STREAMING = {
    "run_record_id": "rec-live", "run_id": "20260819120500_bbbbbb",
    "run_params": {"load_date": "2026-01-01"}, "source_sql": SOURCE_SQL,
    "staging_table": "HOLDING__stg_20260819120500", "stage": "STREAMING",
    "seq": 1, "rows_pushed": 3, "bytes": 96, "ms": 940,
    "last_ts": "2026-08-19T12:05:01.000Z", "live": True,
}
# V17 第二态：已受理、子进程还没报到（阶段串三点全灰），取消当场如实拒绝。
LIVE_ACCEPTED = dict(LIVE_STREAMING, run_record_id="rec-accepted", run_id=None,
                     stage=None, seq=0, rows_pushed=0, bytes=0, ms=0,
                     staging_table=None, run_params={"load_date": "2026-01-02"})

RUNS = {
    "rec-live": LIVE_STREAMING,
    "rec-accepted": LIVE_ACCEPTED,
    # V2：成功，终态 SWAPPED。
    "rec-success": base_run(run_record_id="rec-success", task_id="task-success",
                            run_params={"load_date": "2026-01-03"}),
    # V3：校验失败，终态 DISCARDED，错误码 4xx。
    "rec-verify": base_run(
        run_record_id="rec-verify", task_id="task-verify", run_id="20260819121000_cccccc",
        run_params={"load_date": "2026-01-04"},
        outcome="FAILED", target_table_effect="DISCARDED",
        sink_code="VERIFY_FAILED", failure_kind="VERIFY_FAILED",
        sink_reported_rows=99998,
        message="目标端：行数校验未通过：暂存 100,000 行、目标端点到 99,998 行，暂存表已丢弃。"),
    # V4 / V7 / V11 / V22 / V23：映射预检失败。
    "rec-mapping": base_run(
        run_record_id="rec-mapping", task_id="task-mapping", run_id="20260819121500_dddddd",
        run_params={"load_date": "2026-01-05"},
        outcome="FAILED", target_table_effect=None, staging_table=None,
        sink_code="PRECHECK_FAILED", failure_kind="MAPPING_PRECHECK",
        source_rows=0, staged_rows=None, sink_reported_rows=None, purged_rows=None,
        source_batches=0, received_batches=None, seq=0, rows_pushed=0, bytes=0, ms=0,
        message="目标端：映射预检未通过：一次发现 3 项问题，未创建暂存表",
        mapping_issues=MAPPING_ISSUES),
    # V8：结局不明（进程消失）。
    "rec-unknown": base_run(
        run_record_id="rec-unknown", task_id="task-unknown", run_id="20260819122000_eeeeee",
        run_params={"load_date": "2026-01-06"},
        outcome="FAILED", target_table_effect="UNKNOWN",
        unknown_reason="PROCESS_DISAPPEARED", failure_kind="UNKNOWN",
        sink_code=None, message=None, finished_at=None),
    # V13 / V4 的 5xx 一例：哨兵逃逸，报文里带业务列与业务值。
    "rec-escape": base_run(
        run_record_id="rec-escape", task_id="task-escape", run_id="20260819122500_ffffff",
        run_params={"load_date": "2026-01-07"},
        outcome="FAILED", target_table_effect="DISCARDED",
        sink_code="INTERNAL_PRECHECK_ESCAPE", failure_kind="DEFECT",
        column="C_NAME", value="张三丰·测试客户名·2026",
        message="目标端：内部断言失败：预检放行的值在写入时被拒，暂存表已丢弃。"),
    # V15：run 未发起 —— 目标端不知道这次运行（源端就失败了）。
    "rec-not-started": base_run(
        run_record_id="rec-not-started", task_id="task-not-started", run_id=None,
        run_params={"load_date": "2026-01-08"},
        outcome="FAILED", target_table_effect=None, staging_table=None,
        sink_code=None, failure_kind="SOURCE_CONNECT",
        source_code="ORA-12541",
        source_rows=None, staged_rows=None, sink_reported_rows=None, purged_rows=None,
        seq=0, rows_pushed=0, bytes=0, ms=0,
        message="源端：连接 Oracle 失败：ORA-12541: TNS:no listener，未向 sink 发出请求。"),
}

RUNS_BY_DATE = {
    "2026-01-01": "rec-live",
    "2026-01-02": "rec-accepted",
    "2026-01-03": "rec-success",
    "2026-01-04": "rec-verify",
    "2026-01-05": "rec-mapping",
    "2026-01-06": "rec-unknown",
    "2026-01-07": "rec-escape",
    "2026-01-08": "rec-not-started",
}

# 历史列表的顺序：先摆两个块（V5 要在同一屏上量 SWAPPED 与 DISCARDED），
# 再摆三种中性形态（V9 要看它们参差不齐）。
HISTORY_ORDER = ["rec-success", "rec-verify", "rec-escape", "rec-mapping",
                 "rec-not-started", "rec-unknown", "rec-live"]

BUILDER_TABLES = [{"owner": "APP", "name": "T_HOLDING"}, {"owner": "APP", "name": "T_CUSTOMER"}]

BUILDER_COLUMNS = [
    {"name": "ID", "data_type": "NUMBER", "precision": 19, "scale": 0, "length": None,
     "nullable": False},
    {"name": "C_NAME", "data_type": "VARCHAR2", "precision": None, "scale": None,
     "length": 200, "nullable": True},
    {"name": "LOAD_DATE", "data_type": "DATE", "precision": None, "scale": None,
     "length": None, "nullable": True},
]

TARGET_TABLES = ["HOLDING", "HOLDING_DAILY", "CUSTOMER"]

TARGET_COLUMNS = {
    "columns": [
        {"name": "ID", "column_type": "bigint(20)", "data_type": "bigint", "precision": 19,
         "scale": 0, "length": None, "datetime_precision": None, "nullable": False,
         "character_set": None, "ordinal": 1, "default_value": None, "extra": ""},
        {"name": "C_NAME", "column_type": "varchar(200)", "data_type": "varchar",
         "precision": None, "scale": None, "length": 200, "datetime_precision": None,
         "nullable": True, "character_set": "utf8mb4", "ordinal": 2,
         "default_value": None, "extra": ""},
        {"name": "LOAD_DATE", "column_type": "datetime", "data_type": "datetime",
         "precision": None, "scale": None, "length": None, "datetime_precision": 0,
         "nullable": True, "character_set": None, "ordinal": 3,
         "default_value": None, "extra": ""},
        {"name": "ROW_NO", "column_type": "int(11)", "data_type": "int", "precision": 10,
         "scale": 0, "length": None, "datetime_precision": None, "nullable": False,
         "character_set": None, "ordinal": 4, "default_value": None,
         "extra": "auto_increment"},
    ],
    "keys": [{"name": "PRIMARY", "columns": ["ID"]}],
}


def ddl_for(target_table):
    """V19：`target_table` 留空时 DDL 里保留可见占位符 `<目标表名>`，整段照给。"""
    name = f"`{target_table}`" if target_table else "<目标表名>"
    return (
        "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n"
        "-- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。\n"
        f"CREATE TABLE {name} (\n"
        "  `ID` DECIMAL(19,0) NOT NULL,\n"
        "  `C_NAME` VARCHAR(200) NULL,\n"
        "  `LOAD_DATE` DATETIME(0) NULL,\n"
        "  PRIMARY KEY (`ID`)\n"
        ") DEFAULT CHARSET=utf8mb4;"
    )


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
            return self._send(200, DATASOURCES)
        if path == "/api/tasks":
            return self._send(200, TASKS)
        if path == "/api/runs":
            rows = []
            for key in HISTORY_ORDER:
                run = RUNS[key]
                # 进行中那条在列表里也要有一行（V9 的 `.live-summary`、V16 的并发提示）。
                rows.append(run if not run.get("live") else base_run(
                    run_record_id=run["run_record_id"], run_id=run["run_id"],
                    run_params=run["run_params"], staging_table=run["staging_table"],
                    outcome=None, target_table_effect=None, stage=run["stage"],
                    finished_at=None, sink_code=None, message=None, failure_kind=None,
                    source_rows=run["rows_pushed"], staged_rows=None,
                    sink_reported_rows=None, purged_rows=None,
                    seq=run["seq"], rows_pushed=run["rows_pushed"],
                    bytes=run["bytes"], ms=run["ms"]))
            return self._send(200, rows)
        if path.startswith("/api/runs/"):
            key = path.rsplit("/", 1)[-1]
            if key in RUNS:
                return self._send(200, RUNS[key])
            return self._send(404, {"error": {"message": "run not found"}})
        return self._static(path)

    def do_POST(self):
        path = self.path.split("?")[0]
        if path == "/api/runs":
            body = self._read()
            date = (body.get("run_params") or {}).get("load_date", "")
            return self._send(202, {"run_record_id": RUNS_BY_DATE.get(date, "rec-success")})
        if path.startswith("/api/runs/") and path.endswith("/cancel"):
            key = path.split("/")[3]
            run = RUNS.get(key)
            if run is None:
                return self._send(404, {"error": {"message": "run not found"}})
            if run.get("stage") is None:
                # V17 那句「禁用状态本身会说谎」要的行为：按钮不禁用，当场如实拒绝。
                return self._send(409, {"error": {"message": "run 尚未进入可取消阶段"}})
            return self._send(200, {"message": "已发送 SIGTERM，等待子进程退出"})
        if path == "/api/builder/tables":
            return self._send(200, BUILDER_TABLES)
        if path == "/api/builder/columns":
            return self._send(200, BUILDER_COLUMNS)
        if path == "/api/builder/sql":
            spec = self._read()
            projection = ",\n       ".join(
                f"a.{c['source']} AS {c['target']}" for c in spec.get("columns", []))
            return self._send(200, {
                "source_sql": f"SELECT {projection}\n  FROM {spec.get('owner')}.{spec.get('table')} a",
                "run_parameters": [
                    {"parameter": c["parameter"], "column": c["column"],
                     "value_type": c["value_type"]}
                    for c in spec.get("conditions", []) if c["value_source"] == "runtime"],
            })
        if path == "/api/columns":
            spec = self._read().get("spec", {})
            return self._send(200, {
                "columns": [
                    {"name": "ID", "type": "NUMBER", "precision": 19, "scale": 0,
                     "length": None, "support": "ok"},
                    {"name": "C_NAME", "type": "VARCHAR2", "precision": None, "scale": None,
                     "length": 200, "support": "ok"},
                    {"name": "LOAD_DATE", "type": "DATE", "precision": None, "scale": None,
                     "length": None, "support": "ok"},
                ],
                "target_ddl": ddl_for(spec.get("target_table", "")),
            })
        if path == "/api/target/tables":
            return self._send(200, {"tables": TARGET_TABLES})
        if path == "/api/target/columns":
            body = self._read()
            if body.get("target_table") not in TARGET_TABLES:
                return self._send(200, {"columns": [], "keys": []})
            return self._send(200, TARGET_COLUMNS)
        if path == "/api/datasources/test-connection":
            return self._send(200, {"ok": True, "elapsed_ms": 186, "label": "dw_stage"})
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
