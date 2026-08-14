#!/usr/bin/env python3
"""Forward M1 HTTP calls, but close the first commit connection after sink completion."""

import http.client
import socket
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    commit_dropped = False

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        connection = http.client.HTTPConnection("127.0.0.1", 18080, timeout=1800)
        headers = {"Content-Type": self.headers.get("Content-Type", "application/json")}
        connection.request(self.command, self.path, body=body, headers=headers)
        response = connection.getresponse()
        response_body = response.read()

        if self.path.endswith("/commit") and not Handler.commit_dropped:
            Handler.commit_dropped = True
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


ThreadingHTTPServer(("127.0.0.1", 18081), Handler).serve_forever()
