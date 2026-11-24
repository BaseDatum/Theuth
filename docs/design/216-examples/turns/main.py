#!/usr/bin/env python3
"""turns: counts finished turns today (design 216 §13.1). Standard library only, no SDK.

Host -> plugin: one JSON-RPC 2.0 message per stdin line. Plugin -> host: one per stdout line.
"""
import datetime
import json
import os
import pathlib
import sys

DATA = pathlib.Path(os.environ.get("THEUTH_PLUGIN_DATA", "."))
STATE = DATA / "state.json"
LIMIT = int(os.environ.get("TURNS_LIMIT", "50"))


def out(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def send(method, **params):
    out({"jsonrpc": "2.0", "method": method, "params": params})


def reply(msg, result=None, error=None):
    if "id" not in msg:
        return  # a notification gets no answer
    body = {"error": error} if error else {"result": result}
    out({"jsonrpc": "2.0", "id": msg["id"], **body})


def today():
    return datetime.date.today().isoformat()


def load():
    try:
        s = json.loads(STATE.read_text())
    except (OSError, ValueError):
        s = {}
    return s if s.get("day") == today() else {"day": today(), "turns": 0, "tokens": 0}


def save(s):
    DATA.mkdir(parents=True, exist_ok=True)
    tmp = STATE.with_suffix(".tmp")
    tmp.write_text(json.dumps(s))
    tmp.replace(STATE)


def summary(s):
    return f"{s['turns']} of {LIMIT} turns today · {s['tokens']:,} tokens"


def render(s):
    over = s["turns"] >= LIMIT
    send("status", placement="turns", dot="warn" if over else "ok", badge=s["turns"],
         tooltip=f"Turns today: {s['turns']}", label=f"{s['turns']}/{LIMIT}")
    line = f"**{s['turns']}** of {LIMIT} turns · {s['tokens']:,} tokens"
    send("ui", placement="turns", format="openui", text=summary(s), source="\n".join([
        "root = Card([title, body, controls])",
        'title = TextContent("## Turns today")',
        f"body = TextContent({json.dumps(line)})",
        'reset = Mutation("reset", {})',
        'controls = Buttons([Button("Reset counter", @Run(reset), "secondary")])',
    ]))


state = load()
render(state)
for raw in sys.stdin:
    try:
        msg = json.loads(raw)
    except ValueError:
        continue
    method, p = msg.get("method"), msg.get("params") or {}
    if state["day"] != today():
        state = load()
    if method == "event" and p.get("topic") == "agent.events" and p.get("type") == "TurnFinished":
        usage = p["data"].get("usage") or {}
        state["turns"] += 1
        state["tokens"] += usage.get("prompt", 0) + usage.get("completion", 0)
        save(state)
        render(state)
        if state["turns"] == LIMIT:
            send("emit", event="limit_reached", data={"day": state["day"], "turns": state["turns"]})
    elif method == "action" and p.get("action") == "reset":
        state = {"day": today(), "turns": 0, "tokens": 0}
        save(state)
        render(state)
        reply(msg, {"ok": True})
    elif method == "render":
        render(state)
        reply(msg, {})
    elif method == "tools/call" and p.get("name") == "turns_today":
        reply(msg, {"content": [{"type": "text", "text": summary(state)}],
                    "structuredContent": state, "isError": False})
    elif method == "shutdown":
        save(state)
        break
    else:
        # Every request this plugin does not handle is answered at once, so no caller
        # waits request_timeout_ms (§6.1); unknown notifications are ignored.
        reply(msg, error={"code": -32601, "message": f"method not found: {method}"})
