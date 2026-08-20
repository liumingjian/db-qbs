#!/usr/bin/env python3
"""#154 —— 目标端自检的桩 sink：只答两个端点，答什么由 QBS_STUB_CASE 决定。

存在的理由：目标端自检把「开连接仪式三项前提」**问给 sink**，按它的报错措辞分档
（crates/sink/src/mysql_destination.rs 的 run_connection_ritual）。那段分档是自检里
最容易悄悄判反的一处 —— 而演练台上真 sink 要等 #156 才装得上来，
在那之前这几档一次都没被走过。这个桩把每一档的原话喂进去，逼自检当场把判定说出来。

措辞不是编的：每一条都抄自产品那两个文件；产品改了词，
scripts/test-rehearsal-preflight.sh 第 6 条会先红（它按内容盯着那几处）。
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

RITUAL = "开连接仪式失败，整个 sink 不可用："
CASES = {
    "ok": None,
    "connect": "连接 MySQL 失败：Access denied for user 'qbs'@'10.0.0.7' (using password: YES)",
    "charset": "开连接仪式设置 utf8mb4 失败：ERROR 1115 (42000): Unknown character set: 'utf8mb4'",
    "sqlmode": "开连接仪式设置 sql_mode 失败：ERROR 1231 (42000): Variable 'sql_mode' can't be set",
    "readback": "开连接仪式回读会话变量失败：ERROR 1142 (42000): SELECT command denied",
    "settings-all": RITUAL
    + "环境配置错误：character_set_client 期望 utf8mb4，实际 latin1；"
    + "环境配置错误：sql_mode 期望完整值 STRICT_ALL_TABLES，实际 ；"
    + "环境配置错误：max_allowed_packet 期望至少 67108864 字节，实际 4194304 字节；"
    + "请调整 MySQL 配置，不要排查业务数据",
    "settings-packet": RITUAL
    + "环境配置错误：max_allowed_packet 期望至少 67108864 字节，实际 4194304 字节；"
    + "请调整 MySQL 配置，不要排查业务数据",
    # 认不出的回答：自检必须记未判定，不许掉进「message 里没提到就算合格」那一档。
    "bad-request": "@@400@@请求体坏了：expected value at line 1 column 1",
    "settings-sqlmode": RITUAL
    + "环境配置错误：sql_mode 期望完整值 STRICT_ALL_TABLES，实际 ONLY_FULL_GROUP_BY",
}

CASE = os.environ.get("QBS_STUB_CASE", "ok")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, payload):
        # 紧凑分隔符：产品用 serde_json::to_vec，键值之间**没有空格**。
        # 桩要是打得比产品松，自检里那段截 message 的 sed 就会在这里假绿。
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        # sink 的 404 信封 —— 自检就是按这个码认出「那头真是 sink」的。
        self._send(404, {"error": {"code": "RUN_UNKNOWN", "message": "请求的 sink v1 资源不存在",
                                   "run_id": None, "details": {}}})

    def do_POST(self):
        if self.path != "/v1/target/test-connection":
            return self._send(404, {"error": {"code": "RUN_UNKNOWN", "message": "no", "details": {}}})
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        # Content-Length 算错的话这里就解不出来 —— 自检那半是手写的 HTTP，这一档要判。
        try:
            request = json.loads(raw.decode("utf-8"))
        except Exception as error:  # noqa: BLE001
            return self._send(400, {"error": {"code": "BAD_REQUEST", "message": f"请求体坏了：{error}",
                                              "details": {}}})
        missing = [k for k in ("host", "port", "username", "password", "database") if k not in request]
        if missing:
            return self._send(400, {"error": {"code": "BAD_REQUEST",
                                              "message": f"缺字段：{','.join(missing)}", "details": {}}})
        problem = CASES[CASE]
        if problem is None:
            return self._send(200, {"ok": True})
        if problem.startswith("@@400@@"):
            return self._send(400, {"error": {"code": "BAD_REQUEST", "message": problem[7:],
                                              "details": {}}})
        self._send(500, {"error": {"code": "SINK_ENVIRONMENT", "message": f"连接目标端失败：{problem}",
                                   "run_id": None, "details": {"kind": "OTHER"}}})


if __name__ == "__main__":
    if CASE not in CASES:
        sys.exit(f"QBS_STUB_CASE 只收：{' '.join(CASES)}")
    port = int(sys.argv[1])
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
