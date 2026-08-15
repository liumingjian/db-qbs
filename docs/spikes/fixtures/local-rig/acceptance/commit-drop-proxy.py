#!/usr/bin/env python3
"""Forward M1 HTTP calls, but close the first commit connection after sink completion.

M1_COMMIT_DROP_MODE selects which sink terminal the dropped commit leaves behind:

- ``swapped`` (default) forwards the commit untouched, so the sink swaps the target
  and tombstones the run SWAPPED;
- ``discarded`` inflates the committed ``total_rows`` by one before forwarding, so the
  sink's staged-versus-source verification fails, the staging table is dropped and the
  run is tombstoned DISCARDED with the target untouched.
- ``verify`` performs the same inflation but returns the sink response to the source,
  exposing the normal ``VERIFY_FAILED`` path without a transport failure.

Either way the source only ever sees the transport error, so it has to recover the
terminal from the tombstone via GET /v1/runs/{run_id}.
"""

import http.client
import json
import os
import socket
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = os.environ.get("M1_COMMIT_DROP_MODE", "swapped")
COMMIT_DELAY_SECONDS = float(os.environ.get("M1_COMMIT_DELAY_SECONDS", "0"))
if MODE not in ("swapped", "discarded", "verify"):
    raise SystemExit(f"unsupported M1_COMMIT_DROP_MODE: {MODE}")


class CommitDropProxyHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    commit_dropped = False

    def do_GET(self):
        self.forward_to_sink()

    def do_POST(self):
        self.forward_to_sink()

    def forward_to_sink(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        is_commit = self.path.endswith("/commit")
        if is_commit and MODE in ("discarded", "verify"):
            payload = json.loads(body)
            payload["total_rows"] = payload["total_rows"] + 1
            body = json.dumps(payload).encode()
        if is_commit and COMMIT_DELAY_SECONDS > 0:
            time.sleep(COMMIT_DELAY_SECONDS)
        connection = http.client.HTTPConnection("127.0.0.1", 18080, timeout=1800)
        headers = {"Content-Type": self.headers.get("Content-Type", "application/json")}
        connection.request(self.command, self.path, body=body, headers=headers)
        response = connection.getresponse()
        response_body = response.read()

        if is_commit and MODE != "verify" and not CommitDropProxyHandler.commit_dropped:
            CommitDropProxyHandler.commit_dropped = True
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return

        self.send_response(response.status)
        self.send_header("Content-Type", response.getheader("Content-Type", "application/json"))
        self.send_header("Content-Length", str(len(response_body)))
        self.end_headers()
        self.wfile.write(response_body)

    def log_message(self, _format, *_args):
        return


ThreadingHTTPServer(("127.0.0.1", 18081), CommitDropProxyHandler).serve_forever()
