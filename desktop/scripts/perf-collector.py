#!/usr/bin/env python3
"""A local OTLP/HTTP JSON collector for Rune's interaction timing
(RUNE_PERF=1): appends each posted body, one per line, to the file given.
Rune posts to 127.0.0.1:27686 by default.

    python3 desktop/scripts/perf-collector.py out.jsonl [port]
"""
import http.server, sys
out, port = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 27686
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        with open(out, "ab") as f:
            f.write(body.replace(b"\n", b" ") + b"\n")
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b"{}")
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
