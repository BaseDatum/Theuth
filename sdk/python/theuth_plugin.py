# theuth plugin SDK for Python 3.9+, stdlib only (design 216). Vendored: copy this file beside main.py.
"""Write a theuth plugin in a few lines: register handlers with `plugin`, then `plugin.run()`.

plugin.yaml is the contract: the host learns tools, subscriptions and placements from it, never by running
this file. `python3 main.py --theuth-manifest` prints what the code registered, to compare.
Stdout is the protocol: `print` becomes a `log` line while `run()` is active; stderr goes to the plugin log.
Handlers run one at a time on one thread and the stdin reader is another, so `call()`/`request()` from a
handler never deadlocks. Raw hatch: `send`, `request`, `on_request`, `on_notify`.
"""
from __future__ import annotations

import io
import json
import os
import queue
import sys
import threading
from pathlib import Path
from typing import Any, Callable, Optional

PROTOCOL = 1
__all__ = ["plugin", "Plugin", "RpcError", "State", "Msg"]


class RpcError(Exception):
    """A JSON-RPC error. Raise it from a handler to answer with this error; `call`/`request` raise it."""

    def __init__(self, code: int, message: str, data: Any = None):
        super().__init__(message)
        self.code, self.message, self.data = code, message, data
        self.retry_after_ms = data.get("retry_after_ms") if isinstance(data, dict) else None

    def to_json(self) -> dict:
        return _opt({"code": self.code, "message": self.message}, data=self.data)


class Msg(dict):
    """A dict whose keys are also attributes: `ev.data`, `req.args`. Missing keys read as None."""

    def __getattr__(self, k):
        return self.get(k)


class State:
    """A dict persisted to `<data dir>/state.json`. Every `set`/`add` writes it atomically (tmp + rename)."""

    def __init__(self, owner: "Plugin"):
        self._owner, self._data, self._dir = owner, None, None

    def _path(self) -> Path:
        d = self._dir or os.environ.get("THEUTH_PLUGIN_DATA") or "./.plugin-data"
        return Path(d) / "state.json"

    def _load(self) -> dict:
        if self._data is None:
            try:
                d = json.loads(self._path().read_text(encoding="utf-8"))
                self._data = d if isinstance(d, dict) else {}
            except FileNotFoundError:
                self._data = {}
            except (OSError, ValueError) as e:
                self._data = {}
                self._owner.log(f"state.json unreadable, starting empty: {e}", level="warn")
        return self._data

    def get(self, key: str, default: Any = None) -> Any:
        return self._load().get(key, default)

    def int(self, key: str) -> int:
        """The integer at `key`, 0 when missing or not an integer."""
        v = self.get(key, 0)
        return v if isinstance(v, int) and not isinstance(v, bool) else 0

    def set(self, key: str, value: Any) -> None:
        self._load()[key] = value
        self.save()

    def add(self, key: str, n: int = 1) -> int:
        """Read-modify-write: state[key] += n, saved. Returns the new value."""
        self.set(key, self.int(key) + n)
        return self.int(key)

    def all(self) -> dict:
        return self._load()

    def save(self) -> None:
        p = self._path()
        p.parent.mkdir(parents=True, exist_ok=True)
        tmp = p.with_name("state.tmp")
        tmp.write_text(json.dumps(self._load(), ensure_ascii=False), encoding="utf-8")
        os.replace(tmp, p)


def openui_card(text: str, buttons) -> str:
    """The OpenUI source `ui(text, buttons)` sends: deterministic, so equal input never remounts."""
    lines = ["root = Card([body, controls])" if buttons else "root = Card([body])",
             f"body = TextContent({_js(text)})"]
    calls = []
    for i, b in enumerate(buttons or []):
        label, action, style = (list(b) + ["secondary"])[:3]
        lines.append(f"a{i} = Mutation({_js(action)}, {{}})")
        calls.append(f"Button({_js(label)}, @Run(a{i}), {_js(style)})")
    if buttons:
        lines.append(f"controls = Buttons([{', '.join(calls)}])")
    return "\n".join(lines)


def _js(v: Any) -> str:
    return json.dumps(v, ensure_ascii=False, separators=(",", ":"))


class _LogWriter(io.TextIOBase):
    """Replaces sys.stdout during run(): each printed line becomes a `log` notification."""

    def __init__(self, owner: "Plugin"):
        self._owner, self._buf = owner, ""

    def write(self, s: str) -> int:
        self._buf += s
        while "\n" in self._buf:
            line, self._buf = self._buf.split("\n", 1)
            self._owner.log(line, fields={"stdout": True})
        return len(s)


class Plugin:
    """The plugin. `hello`, `grants`, `data_dir`, `clients` hold what the host sent; `cancelled` holds the
    ids of host requests named by `notifications/cancelled` (a late reply is still sent and discarded)."""

    def __init__(self):
        self.state, self.hello, self.grants, self.clients = State(self), {}, {}, {}
        self.data_dir: Optional[str] = None
        self.cancelled: set = set()
        self._handlers = {k: {} for k in ("event", "action", "query", "command", "tool", "request", "notify")}
        self._hooks = {k: [] for k in ("start", "shutdown", "granted", "lagged", "cancel", "render")}
        self._out_lock, self._pending, self._next_id = threading.Lock(), {}, 0
        self._sink: Optional[Callable[[dict], None]] = None  # drive(): outbound messages go here
        self._responder = None  # drive(): answers our requests synchronously
        self._reader_thread, self._stdout, self._closed = None, sys.stdout, False

    # ---- registration: each works as a decorator, or as plugin.x(name, fn) ----
    def _reg(self, kind, name, fn):
        def put(f):
            self._handlers[kind][name] = f
            return f
        return put(fn) if fn else put

    def on(self, name: str, fn=None):
        """Event handler. `name`: `agent.events#TurnFinished`, `agent.events`, `turns.limit_reached`, or `*`."""
        return self._reg("event", name, fn)
    def action(self, name: str, fn=None):
        """`action` request. Return a JSON value (None -> {"ok": true}); raise RpcError for an error."""
        return self._reg("action", name, fn)
    def query(self, name: str, fn=None):
        """OpenUI `Query(name, …)`. The return value is the query's value."""
        return self._reg("query", name, fn)
    def command(self, name: str, fn=None):
        """`/plugin:name` command. Return a str (-> {"text": …}), a dict (sent as-is) or None (-> {})."""
        return self._reg("command", name, fn)
    def tool(self, name: str, fn=None):
        """`tools/call` for a tool declared in plugin.yaml. Return a str, a CallToolResult dict, or any JSON."""
        return self._reg("tool", name, fn)
    def on_request(self, method: str, fn=None):
        """Raw hatch: answer a host request the helpers do not wrap. fn(params) -> `result`."""
        return self._reg("request", method, fn)
    def on_notify(self, method: str, fn=None):
        """Raw hatch: handle a host notification. fn(params)."""
        return self._reg("notify", method, fn)

    def _hook(self, kind, fn):
        self._hooks[kind].append(fn)
        return fn

    def on_start(self, fn):
        """After `hello`, before the loop. Paint here. Also re-run on `render` unless on_render is set."""
        return self._hook("start", fn)
    def on_shutdown(self, fn):
        """`shutdown` or stdin EOF; the process exits 0 afterwards."""
        return self._hook("shutdown", fn)
    def on_granted(self, fn):
        """Grants changed; `plugin.grants` is already replaced. fn(grants)."""
        return self._hook("granted", fn)
    def on_lagged(self, fn):
        """Events were dropped: fn({dropped, since_ms}). Default: a warn log."""
        return self._hook("lagged", fn)
    def on_cancel(self, fn):
        """`notifications/cancelled`: fn({requestId, reason}). The id is already in `plugin.cancelled`."""
        return self._hook("cancel", fn)
    def on_render(self, fn):
        """`render` request: fn(req); send a `ui`. The SDK replies {}."""
        return self._hook("render", fn)

    # ---- plugin -> host notifications ----
    def send(self, method: str, params: Optional[dict] = None) -> None:
        """Raw notification."""
        self._write({"jsonrpc": "2.0", "method": method, "params": params or {}})

    def _placement(self, placement):
        if placement:
            return placement
        ids = (self.grants or {}).get("placements") or []
        if len(ids) != 1:
            raise ValueError(f"pass placement=…: the grant holds {len(ids)} placements {ids}")
        return ids[0]

    def status(self, dot: str, badge: Any = None, label: Optional[str] = None,
               tooltip: Optional[str] = None, placement: Optional[str] = None) -> None:
        """Dot (ok|busy|warn|error|off|none), badge (int, 0 hides; or ≤4 chars), label ≤16, tooltip ≤200."""
        self.send("status", _opt({"placement": self._placement(placement), "dot": dot},
                                 badge=badge, label=label, tooltip=tooltip))

    def ui(self, text: str, buttons=None, source: Optional[str] = None,
           format: Optional[str] = None, placement: Optional[str] = None) -> None:
        """Replace a placement's content. `text` is the fallback for TUI/channels and always sent.
        `buttons`: [(label, action[, style])] -> an OpenUI card. `source` is sent unchanged."""
        if source is None:
            source = openui_card(text, buttons) if format in (None, "openui") else text
        self.send("ui", {"placement": self._placement(placement), "format": format or "openui",
                         "text": text, "source": source})

    def emit(self, event: str, data: dict, session: Optional[str] = None) -> None:
        """Publish a declared event. An undeclared name is dropped by the host (-32003 in the plugin log)."""
        if not isinstance(data, dict):
            raise TypeError("emit data must be a dict")
        self.send("emit", _opt({"event": event, "data": data}, session=session))

    def notify(self, title: str, body: str = "", level: str = "info", open: Optional[dict] = None) -> None:
        """Inbox item (needs capability `notify`; dropped by the host otherwise)."""
        self.send("notify", _opt({"title": title, "body": body, "level": level}, open=open))

    def invalidate(self, placement: Optional[str] = None) -> None:
        """Clients re-run the placement's OpenUI Queries, keeping `$state`."""
        self.send("invalidate", {"placement": self._placement(placement)})

    def log(self, message: str, level: str = "info", fields: Optional[dict] = None) -> None:
        """A line in the plugin log (level debug|info|warn|error). Not stdout."""
        self.send("log", _opt({"level": level, "message": str(message)}, fields=fields or None))

    # ---- plugin -> host requests ----
    def request(self, method: str, params: Optional[dict] = None) -> Any:
        """Raw request. Returns `result` or raises RpcError. Never call it from the reader thread."""
        if self._responder is not None:
            return self._drive_request(method, params or {})
        if threading.current_thread() is self._reader_thread:
            raise RuntimeError("request() from the reader thread would deadlock")
        with self._out_lock:
            self._next_id += 1
            rid = self._next_id
        done = threading.Event()
        slot = {"done": done}
        self._pending[rid] = slot
        if self._closed:  # stdin already gone: nobody will answer
            self._pending.pop(rid, None)
            raise RpcError(-32006, "host closed stdin")
        self._write({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        done.wait()
        if "error" in slot:
            e = slot["error"] or {}
            raise RpcError(e.get("code", -32603), e.get("message", ""), e.get("data"))
        return slot.get("result")

    def call(self, tool: str, args: Optional[dict] = None) -> Any:
        """Host-brokered tool call. Returns the ToolOutput `{content, is_error, structured?}`."""
        return self.request("call", {"tool": tool, "args": args or {}})

    def input(self, text: str, session: Optional[str] = None, delivery: Optional[str] = None) -> Any:
        """Send text into a session as the user (capability `input`). Returns `{session}`."""
        return self.request("input", _opt({"text": text, "session": session}, delivery=delivery))

    def catalog(self, topic: Optional[str] = None) -> Any:
        return self.request("catalog", {"topic": topic} if topic else {})

    def ping(self) -> Any:
        """Returns `{now_ms}`."""
        return self.request("ping", {})

    # ---- I/O ----
    def _write(self, msg: dict) -> None:
        if self._sink is not None:
            self._sink(msg)
            return
        line = json.dumps(msg, ensure_ascii=False) + "\n"
        with self._out_lock:
            try:
                self._stdout.write(line)
                self._stdout.flush()
            except (BrokenPipeError, ValueError):
                pass  # the host is gone; stdin EOF follows and ends run()

    def _reply(self, mid, result=None, error=None) -> None:
        body = {"error": error} if error is not None else {"result": result}
        self._write({"jsonrpc": "2.0", "id": mid, **body})

    # ---- dispatch ----
    def _on_hello(self, p: dict) -> None:
        if p.get("protocol") != PROTOCOL:
            self.log(f"unsupported protocol {p.get('protocol')!r}; this SDK speaks {PROTOCOL}", level="error")
            sys.exit(1)
        self.hello, self.grants = p, p.get("grants") or {}
        self.data_dir, self.clients = p.get("data_dir"), p.get("clients") or {}
        if not os.environ.get("THEUTH_PLUGIN_DATA") and self.data_dir:
            self.state._dir, self.state._data = self.data_dir, None
        self._guard("on_start", lambda: [fn() for fn in self._hooks["start"]])

    def _guard(self, what, fn) -> None:
        """Run a notification handler or hook; an exception is logged and the process stays up."""
        try:
            fn()
        except Exception as e:  # noqa: BLE001
            self.log(f"{what} failed: {e!r}", level="error")

    def _dispatch(self, msg: dict) -> bool:
        """Handle one host message on the handler thread. Returns False on shutdown."""
        method, p, mid = msg.get("method"), msg.get("params") or {}, msg.get("id")
        if "id" in msg:
            try:
                self._reply(mid, self._request(method, p, mid, msg))
            except RpcError as e:
                self._reply(mid, error=e.to_json())
            except Exception as e:  # noqa: BLE001 - a handler bug answers -32603, the process stays up
                self.log(f"{method} failed: {e!r}", level="error")
                self._reply(mid, error={"code": -32603, "message": str(e) or type(e).__name__})
            return True
        if method == "shutdown":
            return False
        self._guard(method, lambda: self._notify(method, p, msg))
        return True

    def _notify(self, method, p, msg) -> None:
        if method == "event":
            ev = Msg(topic=p.get("topic"), version=p.get("version"), type=p.get("type"),
                     session=p.get("session"), at_ms=p.get("at_ms"), data=p.get("data"), raw=msg)
            h = self._handlers["event"]
            for key in (f"{ev.topic}#{ev.type}", ev.topic, ev.type, "*"):
                if key in h:
                    return h[key](ev)
        elif method == "granted":
            self.grants = p.get("grants") or {}
            for fn in self._hooks["granted"]:
                fn(self.grants)
        elif method == "lagged":
            for fn in self._hooks["lagged"] or [lambda q: self.log(f"lagged: {q.get('dropped')} events dropped", level="warn", fields=q)]:
                fn(p)
        elif method == "notifications/cancelled":
            self.cancelled.add(p.get("requestId"))
            for fn in self._hooks["cancel"]:
                fn(p)
        elif method in self._handlers["notify"]:
            self._handlers["notify"][method](p)

    def _request(self, method, p, mid, msg):
        meta = ((p.get("_meta") or {}).get("theuth") or {})
        req = Msg(placement=p.get("placement"), args=p.get("args"), session=p.get("session", meta.get("session")),
                  client=p.get("client"), form=p.get("form"), activation=p.get("activation"), id=mid, raw=msg)
        kinds = {"action": ("action", "action"), "query": ("query", "query"),
                 "command": ("command", "name"), "tools/call": ("tool", "name")}
        if method in kinds:
            kind, key = kinds[method]
            req["name"] = p.get(key)
            if method == "tools/call":
                req["args"] = p.get("arguments") or {}
            fn = self._handlers[kind].get(req.name)
            if fn is None:
                raise RpcError(-32601, f"{method} not handled: {req.name}")
            if kind == "tool":
                try:
                    return _tool_result(fn(req))
                except RpcError:
                    raise
                except Exception as e:  # noqa: BLE001 - the model must see the text
                    self.log(f"tool {req.name} failed: {e!r}", level="error")
                    return {"content": [{"type": "text", "text": str(e) or type(e).__name__}], "isError": True}
            r = fn(req)
            if kind == "action":
                return {"ok": True} if r is None else r
            if kind == "command":
                return {"text": r} if isinstance(r, str) else ({} if r is None else r)
            return r
        if method in self._handlers["request"]:
            return self._handlers["request"][method](p)
        if method == "render" and (self._hooks["render"] or self._hooks["start"]):
            for fn in self._hooks["render"] or [lambda _: [f() for f in self._hooks["start"]]]:
                fn(req)
            return {}
        raise RpcError(-32601, f"method not found: {method}")

    def _manifest(self) -> str:
        h = self._handlers
        rows = [("tools", h["tool"]), ("actions", h["action"]), ("queries", h["query"]),
                ("commands", h["command"]), ("on", h["event"])]
        return "# registered in code (plugin.yaml is the contract)\n" + "".join(
            f"{k}: [{', '.join(_js(n) for n in v)}]\n" for k, v in rows)

    # ---- run loop ----
    def run(self) -> None:
        """Block until `shutdown` or stdin EOF, then call on_shutdown and exit 0."""
        if "--theuth-manifest" in sys.argv[1:]:
            sys.stdout.write(self._manifest())
            sys.stdout.flush()
            sys.exit(0)
        self._stdout = sys.stdout
        sys.stdout = _LogWriter(self)
        inbox: queue.Queue = queue.Queue()
        self._reader_thread = threading.Thread(target=self._read, args=(inbox,), daemon=True, name="theuth-reader")
        self._reader_thread.start()
        started = False
        while True:
            msg = inbox.get()
            if msg is None:
                break
            if not started:
                if msg.get("method") == "hello":
                    self._on_hello(msg.get("params") or {})
                    started = True
                elif "id" in msg:
                    self._reply(msg["id"], error={"code": -32601, "message": "hello first"})
                continue
            if not self._dispatch(msg):
                break
        self._guard("on_shutdown", lambda: [fn() for fn in self._hooks["shutdown"]])
        sys.stdout = self._stdout
        os._exit(0)  # the reader thread may sit in a blocking read

    def _read(self, inbox: queue.Queue) -> None:
        for raw in sys.stdin.buffer:
            try:
                msg = json.loads(raw.decode("utf-8").rstrip("\r\n"))
            except ValueError:
                continue
            if not isinstance(msg, dict):
                continue
            if "method" not in msg and "id" in msg:
                slot = self._pending.pop(msg["id"], None)
                if slot is not None:
                    slot.update({k: msg[k] for k in ("result", "error") if k in msg})
                    slot["done"].set()
                continue
            if msg.get("method") == "notifications/cancelled":
                self.cancelled.add((msg.get("params") or {}).get("requestId"))
            inbox.put(msg)
        self._closed = True  # before failing pending requests: request() checks it after registering
        for slot in list(self._pending.values()):
            slot["error"] = {"code": -32006, "message": "host closed stdin"}
            slot["done"].set()
        inbox.put(None)

    # ---- testing ----
    def drive(self, lines, responder: Optional[Callable[[str, dict], Any]] = None) -> list:
        """Run host messages (dicts or JSON strings) through the dispatcher without stdin. Returns what the
        plugin sent. `responder(method, params)` answers the plugin's requests (raise RpcError to fail)."""
        out: list = []
        self._sink, self._responder = out.append, responder or _no_responder
        try:
            for line in lines:
                msg = json.loads(line) if isinstance(line, str) else line
                if msg.get("method") == "hello":
                    self._on_hello(msg.get("params") or {})
                elif not self._dispatch(msg):
                    self._guard("on_shutdown", lambda: [fn() for fn in self._hooks["shutdown"]])
                    break
        finally:
            self._sink = self._responder = None
        return out

    def _drive_request(self, method, params):
        self._next_id += 1
        self._sink({"jsonrpc": "2.0", "id": self._next_id, "method": method, "params": params})
        return self._responder(method, params)


def _no_responder(method, params):
    raise RpcError(-32601, f"drive(): no responder for {method}")


def _tool_result(r: Any) -> dict:
    if isinstance(r, dict) and ("content" in r or "isError" in r):
        return r
    out = {"content": [{"type": "text", "text": r if isinstance(r, str) else _js(r)}], "isError": False}
    return {**out, "structuredContent": r} if isinstance(r, dict) else out


def _opt(p: dict, **kw) -> dict:
    """p plus the keyword values that are not None."""
    return {**p, **{k: v for k, v in kw.items() if v is not None}}


plugin = Plugin()
