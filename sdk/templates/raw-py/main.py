#!/usr/bin/env python3
"""presses: counts button presses, no SDK (`theuth plugin new --raw`; the shape of design 216 §13.1).

Host -> plugin: one JSON-RPC 2.0 message per stdin line. Plugin -> host: one per stdout line.
Every request gets an answer; a request this file does not handle gets -32601 at once.
"""
import json
import os
import pathlib
import sys

DATA = pathlib.Path(os.environ.get("THEUTH_PLUGIN_DATA", ".plugin-data"))
STATE = DATA / "state.json"


def out(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def send(method, **params):
    out({"jsonrpc": "2.0", "method": method, "params": params})


def reply(msg, result=None, error=None):
    if "id" in msg:
        out({"jsonrpc": "2.0", "id": msg["id"], **({"error": error} if error else {"result": result})})


def load():
    try:
        return json.loads(STATE.read_text())
    except (OSError, ValueError):
        return {"n": 0}


def save(s):
    DATA.mkdir(parents=True, exist_ok=True)
    tmp = STATE.with_name("state.tmp")
    tmp.write_text(json.dumps(s))
    tmp.replace(STATE)


def render(s):
    n = s["n"]
    send("status", placement="presses", dot="ok", badge=n, label=str(n))
    send("ui", placement="presses", format="openui", text=f"{n} presses", source="\n".join([
        "root = Card([body, controls])",
        f"body = TextContent({json.dumps(f'{n} presses')})",
        'a0 = Mutation("inc", {})',
        'controls = Buttons([Button("Press", @Run(a0), "secondary")])',
    ]))


state = load()
for raw in sys.stdin:
    try:
        msg = json.loads(raw)
    except ValueError:
        continue  # not JSON: ignore
    method, p = msg.get("method"), msg.get("params") or {}
    if method == "hello":
        render(state)  # paint once the host is listening
    elif method == "action" and p.get("action") == "inc":
        state["n"] += 1
        save(state)
        render(state)
        reply(msg, {"ok": True})
    elif method == "render":
        render(state)
        reply(msg, {})
    elif method == "tools/call" and p.get("name") == "presses":
        reply(msg, {"content": [{"type": "text", "text": str(state["n"])}], "isError": False})
    elif method == "shutdown":
        break
    else:
        reply(msg, error={"code": -32601, "message": f"method not found: {method}"})
save(state)
