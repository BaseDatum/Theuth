#!/usr/bin/env python3
"""Deterministic fake Anthropic + OpenAI endpoint (127.0.0.1). Usage: fake.py [port]  (0 = free port)."""
import json, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

def b(s, n):
    return s.encode()[:n].decode(errors="ignore").replace("\n", " ")

def answer(user):
    if "Reply with exactly" in user:
        body = user.split("nothing else:\n", 1)[1]
        parts = re.split(r"(?m)^\[(\d+)\] ", body)  # ['', '1', text, '2', text...]
        return "\n".join(f"{parts[i]}| {b(parts[i + 1], 200)}" for i in range(1, len(parts) - 1, 2))
    return b(user.rsplit("bytes:\n", 1)[1], 300)

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["content-length"])))
        user = req["messages"][-1]["content"]
        text = answer(user)
        tin, tout = len(json.dumps(req)) // 4, len(text) // 4
        if self.path == "/v1/messages":
            out = {"content": [{"type": "text", "text": text}], "usage": {"input_tokens": tin, "output_tokens": tout}}
        elif self.path == "/v1/chat/completions":
            out = {"choices": [{"message": {"content": text}}], "usage": {"prompt_tokens": tin, "completion_tokens": tout}}
        else:
            self.send_error(404); return
        d = json.dumps(out).encode()
        self.send_response(200); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(d))); self.end_headers(); self.wfile.write(d)
    def log_message(self, *a): pass

s = HTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 0), H)
print(f"PORT={s.server_port}", flush=True)
s.serve_forever()
