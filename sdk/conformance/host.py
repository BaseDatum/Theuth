#!/usr/bin/env python3
"""Scripted theuth host for SDK conformance (design 216). Stdlib only.

    host.py --mode full|template|eof|manifest [--data DIR] [--start N] -- CMD ARGS...

Spawns CMD in the current directory, plays one script over JSON-RPC lines, prints one line per check
and exits 0 when every check passed. `full` drives a conformance fixture (sdk/conformance/plugins/*),
`template` a hello world (sdk/templates/*), `eof` checks exit 0 on stdin EOF after hello, `manifest`
runs CMD --theuth-manifest with stdin held open and checks it prints the registered names and exits.
"""
import argparse
import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import time

WAIT_S = float(os.environ.get("HOST_WAIT_S", "20"))


def source(n):
    """The OpenUI every SDK must produce for ui(text="<n> presses", buttons=[("Press", "inc")])."""
    return "\n".join([
        "root = Card([body, controls])",
        f'body = TextContent("{n} presses")',
        'a0 = Mutation("inc", {})',
        'controls = Buttons([Button("Press", @Run(a0), "secondary")])',
    ])


class Host:
    def __init__(self, cmd, data, name):
        env = dict(os.environ, THEUTH_PLUGIN=name, THEUTH_PLUGIN_DATA=data, THEUTH_PROTOCOL="1",
                   THEUTH_PLUGIN_DIR=os.getcwd(), PYTHONUNBUFFERED="1")
        self.proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env)
        self.inbox = queue.Queue()
        self.logs = []
        self.stray = []
        self.failed = 0
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for raw in self.proc.stdout:
            line = raw.decode("utf-8", "replace").rstrip("\n")
            try:
                msg = json.loads(line)
            except ValueError:
                self.stray.append(line)
                continue
            if not isinstance(msg, dict) or msg.get("jsonrpc") != "2.0":
                self.stray.append(line)
                continue
            if msg.get("method") == "log":
                self.logs.append(msg.get("params") or {})
            self.inbox.put(msg)
        self.inbox.put(None)

    def send(self, msg):
        line = msg if isinstance(msg, str) else json.dumps(msg)
        try:
            self.proc.stdin.write((line + "\n").encode())
            self.proc.stdin.flush()
        except BrokenPipeError:
            pass

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, mid, method, params):
        self.send({"jsonrpc": "2.0", "id": mid, "method": method, "params": params})

    def until(self, stop):
        """Messages (logs excluded) up to and including the first one `stop` accepts; None at EOF/timeout."""
        got = []
        deadline = time.time() + WAIT_S
        while True:
            try:
                msg = self.inbox.get(timeout=max(0.1, deadline - time.time()))
            except queue.Empty:
                got.append({"timeout": True})
                return got
            if msg is None:
                got.append({"eof": True})
                return got
            if msg.get("method") == "log":
                if stop(msg):
                    return got + [msg]
                continue
            got.append(msg)
            if stop(msg):
                return got

    def response(self, mid):
        return self.until(lambda m: m.get("id") == mid and "method" not in m)

    def log_until(self, pred):
        """Wait until a log whose params satisfy pred arrives (or timeout)."""
        if any(pred(p) for p in self.logs):
            return True
        got = self.until(lambda m: m.get("method") == "log" and pred(m.get("params") or {}))
        return got[-1].get("method") == "log"

    def check(self, name, ok, detail=""):
        print(f"  {name}: {'yes' if ok else 'NO'}{'' if ok else '  ' + str(detail)[:600]}")
        if not ok:
            self.failed += 1

    def wait_exit(self, secs=10):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            return self.proc.wait(timeout=secs)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return "hung"


def methods(seq):
    return [m.get("method") or ("result" if "result" in m else "error" if "error" in m else str(m)) for m in seq]


def params(seq, method):
    return [m.get("params") or {} for m in seq if m.get("method") == method]


def hello(data, tools):
    return {"protocol": 1, "plugin": "presses", "version": "0.1.0", "theuth": "0.0.0-conformance",
            "engine": "1", "data_dir": data, "clients": {"desktop": True, "tui": False},
            "grants": {"subscribe": ["conformance.tick", "agent.events#TurnFinished"], "emits": ["pressed"],
                       "placements": ["presses"], "tools": tools,
                       "capabilities": {"notify": True, "input": False,
                                        "call": {"tools": ["echo"], "max_risk": "read_only"}}}}


def paint_ok(h, seq, n, step):
    st, ui = params(seq, "status"), params(seq, "ui")
    h.check(f"{step}: status dot=ok badge={n}",
            st[-1:] == [{"placement": "presses", "dot": "ok", "badge": n, "label": str(n)}], st)
    h.check(f"{step}: ui openui, text, exact source",
            ui[-1:] == [{"placement": "presses", "format": "openui", "text": f"{n} presses",
                         "source": source(n)}], ui)


def answer(h, seq, method, mid_of=None, result=None, error=None):
    """seq ends in the plugin's request `method`; answer it."""
    req = seq[-1] if seq and seq[-1].get("method") == method and "id" in seq[-1] else None
    if req is None:
        return None
    body = {"error": error} if error else {"result": result}
    h.send({"jsonrpc": "2.0", "id": req["id"], **body})
    return req


def is_req(method):
    return lambda m: m.get("method") == method and "id" in m


def full(h, data):
    h.notify("hello", hello(data, ["presses", "fail"]))
    seq = h.until(lambda m: m.get("method") == "ui")
    h.check("2 start: status then ui", methods(seq) == ["status", "ui"], methods(seq))
    paint_ok(h, seq, 0, "2 start")
    seq = h.until(is_req("ping"))
    req = answer(h, seq, "ping", result={"now_ms": 4242})
    h.check("3 on_start ping request", req is not None and req.get("params") == {}, seq)
    h.check("3 log carries now_ms", h.log_until(lambda p: (p.get("fields") or {}).get("now_ms") == 4242), h.logs)

    h.request("a-1", "action", {"placement": "presses", "action": "inc", "args": {}, "session": None,
                                "client": "desktop", "activation": "act-1", "form": None})
    seq = h.response("a-1")
    h.check("4 action inc: status, ui, result", methods(seq) == ["status", "ui", "result"], methods(seq))
    paint_ok(h, seq, 1, "4 action inc")
    h.check("4 action result {ok:true}", seq[-1].get("result") == {"ok": True}, seq[-1])

    h.request("t-1", "tools/call", {"name": "presses", "arguments": {},
                                    "_meta": {"theuth": {"session": "s-1", "call_id": "toolu_1"}}})
    r = h.response("t-1")[-1].get("result") or {}
    h.check("5 tools/call text 1", r.get("content") == [{"type": "text", "text": "1"}] and r.get("isError") is False, r)

    h.request("q-1", "query", {"placement": "presses", "query": "history", "args": {}, "session": None})
    r = h.response("q-1")[-1]
    h.check("6 unhandled query -32601", (r.get("error") or {}).get("code") == -32601, r)

    h.send("this is not json {")
    h.notify("notifications/cancelled", {"requestId": "t-1", "reason": "timeout"})
    h.notify("lagged", {"dropped": 3, "since_ms": 1795530419000})
    h.check("9 lagged -> warn log", h.log_until(lambda p: p.get("level") == "warn"), h.logs)

    h.notify("event", {"topic": "plugins.events", "version": 1, "type": "conformance.tick", "at_ms": 1,
                       "data": {"plugin": "conformance", "event": "tick", "version": 1, "session": None,
                                "data": {"k": "v"}}})
    h.check("event plugin.event -> on(name)",
            h.log_until(lambda p: p.get("message") == "tick" and (p.get("fields") or {}).get("k") == "v"), h.logs)
    h.notify("event", {"topic": "agent.events", "version": 1, "type": "TurnFinished", "session": "s-1",
                       "at_ms": 2, "data": {"type": "TurnFinished", "session": "s-1", "stop_reason": "end_turn"}})
    h.check("event topic#type -> on(name)", h.log_until(
        lambda p: p.get("message") == "turn" and (p.get("fields") or {}).get("stop_reason") == "end_turn"), h.logs)

    h.request("r-1", "render", {"placement": "presses", "session": None, "client": "desktop"})
    seq = h.response("r-1")
    h.check("render: status, ui, {}", methods(seq) == ["status", "ui", "result"] and seq[-1].get("result") == {},
            seq)
    h.request("c-1", "command", {"placement": "presses-cmd", "name": "count", "args": "", "session": "s-1",
                                 "client": "tui"})
    r = h.response("c-1")[-1]
    h.check("command str -> {text}", r.get("result") == {"text": "n=1"}, r)

    h.request("a-2", "action", {"placement": "presses", "action": "deadlock", "args": {}})
    seq = h.until(lambda m: is_req("ping")(m) or m.get("id") == "a-2")
    req = answer(h, seq, "ping", result={"now_ms": 7})
    seq = h.response("a-2") if req else seq
    h.check("deadlock: request(ping) inside an action", req is not None and seq[-1].get("result") == {"now_ms": 7},
            seq)

    h.request("a-3", "action", {"placement": "presses", "action": "call", "args": {}})
    seq = h.until(lambda m: is_req("call")(m) or m.get("id") == "a-3")
    out = {"content": "echoed", "is_error": False, "structured": {"x": 1}}
    req = answer(h, seq, "call", result=out)
    h.check("call params {tool, args}", req is not None and req.get("params") == {"tool": "echo", "args": {"x": 1}},
            seq)
    r = h.response("a-3")[-1] if req else seq[-1]
    h.check("call returns ToolOutput", r.get("result") == out, r)

    h.request("a-4", "action", {"placement": "presses", "action": "denied", "args": {}})
    seq = h.until(lambda m: is_req("call")(m) or m.get("id") == "a-4")
    req = answer(h, seq, "call", error={"code": -32003, "message": "not granted: call shell", "data": {"tool": "shell"}})
    r = h.response("a-4")[-1] if req else seq[-1]
    h.check("call error -> RpcError code", r.get("result") == {"code": -32003}, r)

    h.request("a-5", "action", {"placement": "presses", "action": "boom", "args": {}})
    r = h.response("a-5")[-1]
    e = r.get("error") or {}
    h.check("handler exception -> -32603", e.get("code") == -32603 and "boom" in e.get("message", ""), r)
    h.request("t-2", "tools/call", {"name": "fail", "arguments": {}})
    r = h.response("t-2")[-1].get("result") or {}
    h.check("tool exception -> isError text", r.get("isError") is True and "kaput" in json.dumps(r.get("content")), r)

    h.request("x-1", "custom/echo", {"a": [1, "two"]})
    r = h.response("x-1")[-1]
    h.check("on_request raw hatch", r.get("result") == {"a": [1, "two"]}, r)
    h.notify("whatever/thing", {})
    h.request("a-6", "action", {"placement": "presses", "action": "extras", "args": {"k": 2}})
    seq = h.response("a-6")
    want = [("emit", {"event": "pressed", "data": {"n": 1}}),
            ("notify", {"title": "Hi", "body": "there", "level": "info"}),
            ("invalidate", {"placement": "presses"}), ("custom/note", {"a": 1})]
    got = [(m.get("method"), m.get("params")) for m in seq[:-1]]
    h.check("emit, notify, invalidate, send", got == want, got)
    h.check("action args reach handler", seq[-1].get("result") == {"k": 2}, seq[-1])

    h.notify("granted", {"grants": {"subscribe": [], "emits": [], "placements": ["presses", "other"], "tools": [],
                                    "capabilities": {"notify": False, "input": False, "call": None}}})
    h.check("granted -> on_granted with new grants",
            h.log_until(lambda p: p.get("message") == "granted" and (p.get("fields") or {}).get("placements") == 2),
            h.logs)
    h.notify("shutdown", {"reason": "disable", "grace_ms": 3000})
    h.check("shutdown -> on_shutdown", h.log_until(lambda p: p.get("message") == "bye"), h.logs)
    rc = h.wait_exit()
    h.check("shutdown -> exit 0", rc == 0, rc)
    state_is(h, data, 1)


def state_is(h, data, n):
    try:
        st = json.load(open(os.path.join(data, "state.json")))
    except (OSError, ValueError) as e:
        st = repr(e)
    h.check(f"state.json n={n}", isinstance(st, dict) and st.get("n") == n, st)


def template(h, data, start):
    h.notify("hello", hello(data, ["presses"]))
    seq = h.until(lambda m: m.get("method") == "ui")
    h.check("start: status then ui", methods(seq) == ["status", "ui"], methods(seq))
    paint_ok(h, seq, start, "start")
    h.request(1, "action", {"placement": "presses", "action": "inc", "args": {}, "session": None,
                            "client": "desktop", "activation": "act-1", "form": None})
    seq = h.response(1)
    h.check("press: status, ui, {ok:true}", methods(seq) == ["status", "ui", "result"]
            and seq[-1].get("result") == {"ok": True}, seq)
    paint_ok(h, seq, start + 1, "press")
    h.request(2, "tools/call", {"name": "presses", "arguments": {}})
    r = h.response(2)[-1].get("result") or {}
    h.check(f"tool text {start + 1}", (r.get("content") or [{}])[0].get("text") == str(start + 1)
            and r.get("isError") is False, r)
    h.request(3, "query", {"placement": "presses", "query": "nope", "args": {}})
    r = h.response(3)[-1]
    h.check("unhandled -32601", (r.get("error") or {}).get("code") == -32601, r)
    h.notify("shutdown", {"reason": "disable", "grace_ms": 3000})
    rc = h.wait_exit()
    h.check("shutdown -> exit 0", rc == 0, rc)
    state_is(h, data, start + 1)


def eof(h, data):
    h.notify("hello", hello(data, ["presses"]))
    seq = h.until(lambda m: m.get("method") == "ui")
    h.check("painted after hello", methods(seq)[-1:] == ["ui"], seq)
    rc = h.wait_exit()
    h.check("stdin EOF -> exit 0", rc == 0, rc)


def status_only(h, data):
    """A plugin that only writes `status` (the §13.2 shape): status after hello, exit 0 on shutdown."""
    h.notify("hello", hello(data, []))
    seq = h.until(lambda m: m.get("method") == "status")
    st = params(seq, "status")
    h.check("status after hello", st[-1:] and st[-1].get("placement") == "presses" and st[-1].get("dot") == "ok",
            seq)
    h.notify("shutdown", {"reason": "disable", "grace_ms": 3000})
    try:
        rc = h.proc.wait(timeout=5)  # stdin still open: the exit comes from `shutdown`, not EOF
    except subprocess.TimeoutExpired:
        rc = h.wait_exit(1)
    h.check("shutdown -> exit 0 at once", rc == 0, rc)


def manifest(cmd, want):
    proc = subprocess.Popen(cmd + ["--theuth-manifest"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    chunks = []
    reader = threading.Thread(target=lambda: chunks.append(proc.stdout.read()), daemon=True)
    reader.start()
    try:
        # stdin stays open until the process exits: a plugin that reads it hangs here instead of passing.
        rc = proc.wait(timeout=WAIT_S)
        reader.join(WAIT_S)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        rc = "hung (read stdin?)"
    finally:
        proc.stdin.close()
    out = b"".join(chunks)
    text = out.decode("utf-8", "replace")
    got = {}
    for line in text.splitlines():
        if line.strip() and not line.startswith("#") and ":" in line:
            k, v = line.split(":", 1)
            try:
                got[k.strip()] = sorted(json.loads(v))
            except ValueError:
                got[k.strip()] = v
    ok = rc == 0 and got == {k: sorted(v) for k, v in want.items()}
    print(f"  --theuth-manifest exit 0 + names: {'yes' if ok else 'NO  rc=%s out=%r' % (rc, text[:400])}")
    return 0 if ok else 1


FIXTURE = {"tools": ["presses", "fail"], "actions": ["inc", "deadlock", "call", "denied", "boom", "extras"],
           "queries": [], "commands": ["count"], "on": ["conformance.tick", "agent.events#TurnFinished"]}
TEMPLATE = {"tools": ["presses"], "actions": ["inc"], "queries": [], "commands": [], "on": []}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", required=True, choices=["full", "template", "eof", "status", "manifest-full",
                                                       "manifest-template"])
    ap.add_argument("--data")
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
    if a.mode.startswith("manifest"):
        return manifest(cmd, FIXTURE if a.mode == "manifest-full" else TEMPLATE)
    data = a.data or tempfile.mkdtemp(prefix="theuth-conf-")
    h = Host(cmd, data, "presses")
    {"full": lambda: full(h, data), "template": lambda: template(h, data, a.start),
     "eof": lambda: eof(h, data), "status": lambda: status_only(h, data)}[a.mode]()
    if h.proc.poll() is None:
        h.proc.kill()
    if h.stray:
        print(f"  (stray stdout ignored: {h.stray[:3]})")
    return 1 if h.failed else 0


if __name__ == "__main__":
    sys.exit(main())
