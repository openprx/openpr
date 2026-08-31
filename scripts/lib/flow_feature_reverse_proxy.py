#!/usr/bin/env python3
"""Tiny same-origin HTTP proxy used by the live feature-flag browser probe."""

from __future__ import annotations

import argparse
import http.server
import urllib.error
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    api_upstream = ""
    frontend_upstream = ""

    def proxy(self) -> None:
        upstream = self.api_upstream if self.path.startswith("/api/") else self.frontend_upstream
        body = None
        if length := self.headers.get("Content-Length"):
            body = self.rfile.read(int(length))
        request = urllib.request.Request(upstream + self.path, data=body, method=self.command)
        for key, value in self.headers.items():
            if key.lower() not in {"host", "connection", "content-length", "accept-encoding"}:
                request.add_header(key, value)
        try:
            response = urllib.request.urlopen(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        except Exception as error:
            payload = str(error).encode()
            self.send_response(502)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        payload = response.read()
        self.send_response(response.status)
        for key, value in response.headers.items():
            if key.lower() not in {"connection", "content-length", "content-encoding", "transfer-encoding"}:
                self.send_header(key, value)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(payload)

    do_GET = proxy
    do_HEAD = proxy
    do_POST = proxy
    do_PUT = proxy
    do_PATCH = proxy
    do_DELETE = proxy
    do_OPTIONS = proxy

    def log_message(self, _format: str, *_args: object) -> None:
        return


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bind", required=True)
    parser.add_argument("--api-upstream", required=True)
    parser.add_argument("--frontend-upstream", required=True)
    args = parser.parse_args()
    host, port = args.bind.rsplit(":", 1)
    Handler.api_upstream = args.api_upstream.rstrip("/")
    Handler.frontend_upstream = args.frontend_upstream.rstrip("/")
    http.server.ThreadingHTTPServer((host, int(port)), Handler).serve_forever()


if __name__ == "__main__":
    main()
