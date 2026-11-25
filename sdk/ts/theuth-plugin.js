// theuth-plugin.js 0.1.0: theuth plugin SDK for JavaScript (design 216). One ESM file, no deps; Node 18+ (with the template's package.json), Bun, Deno 2.
/**
 * Write a theuth plugin in a few lines. Vendored: copy this file (and theuth-plugin.d.ts) beside main.mjs.
 *
 *   import { plugin } from "./theuth-plugin.js";
 *   plugin.action("inc", () => { plugin.state.add("n", 1); });
 *   await plugin.run();
 *
 * plugin.yaml is the contract: the host learns tools, subscriptions and placements from it, never by
 * running this file. `node main.mjs --theuth-manifest` prints what the code registered, to compare.
 *
 * Stdout is the protocol. console.log/info/debug become `log` notifications while run() is active;
 * stderr (console.error/warn) is the log file. Handlers may be sync or async and run one at a time; the
 * stdin reader keeps routing responses meanwhile, so `await plugin.call()` in a handler never deadlocks.
 * Raw hatch: send, request, on_request, on_notify. Every snake_case hook also has a camelCase alias.
 */
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { createInterface } from "node:readline";
import { format as fmt } from "node:util";

export const PROTOCOL = 1;

/** A JSON-RPC error. Throw it from a handler to answer with this error; request()/call() reject with it. */
export class RpcError extends Error {
  /** @param {number} code @param {string} message @param {unknown} [data] */
  constructor(code, message, data) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.data = data;
    this.retry_after_ms = isObj(data) ? data.retry_after_ms : undefined;
  }

  toJSON() {
    return this.data == null ? { code: this.code, message: this.message } : { code: this.code, message: this.message, data: this.data };
  }
}

/** An object persisted to `<data dir>/state.json`. Every set/add writes it atomically (tmp + rename). */
export class State {
  constructor(owner) {
    this._owner = owner;
    this._data = this._dir = null;
  }

  _path() {
    return join(this._dir || process.env.THEUTH_PLUGIN_DATA || "./.plugin-data", "state.json");
  }

  _load() {
    if (this._data === null) {
      try {
        const d = JSON.parse(readFileSync(this._path(), "utf8"));
        this._data = isObj(d) ? d : {};
      } catch (e) {
        this._data = {};
        if (e?.code !== "ENOENT") this._owner.log(`state.json unreadable, starting empty: ${e}`, "warn");
      }
    }
    return this._data;
  }

  /** The value at `key`, or `dflt` when the key is missing. */
  get(key, dflt) { const d = this._load(); return Object.hasOwn(d, key) ? d[key] : dflt; }
  /** The integer at `key`; 0 when missing or not an integer. */
  int(key) { const v = this.get(key, 0); return Number.isInteger(v) ? v : 0; }
  /** Set `key` and save. */
  set(key, value) { this._load()[key] = value; this.save(); }
  /** Add `n` to the integer at `key`, save, return the new value. */
  add(key, n = 1) { const v = this.int(key) + n; this.set(key, v); return v; }

  /** The whole state object (mutate it, then save()). */
  all() { return this._load(); }

  save() {
    const p = this._path(), tmp = join(dirname(p), "state.tmp");
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(tmp, JSON.stringify(this._load()), "utf8");
    renameSync(tmp, p);
  }
}

/** The OpenUI source `ui(text, buttons)` sends: deterministic, so equal input never remounts. */
export function openui_card(text, buttons) {
  const js = JSON.stringify, has = buttons && buttons.length > 0, calls = [];
  const lines = [has ? "root = Card([body, controls])" : "root = Card([body])", `body = TextContent(${js(text)})`];
  (buttons || []).forEach(([label, action, style = "secondary"], i) => {
    lines.push(`a${i} = Mutation(${js(action)}, {})`);
    calls.push(`Button(${js(label)}, @Run(a${i}), ${js(style)})`);
  });
  if (has) lines.push(`controls = Buttons([${calls.join(", ")}])`);
  return lines.join("\n");
}

const KINDS = new Map([["action", ["action", "action"]], ["query", ["query", "query"]],
  ["command", ["command", "name"]], ["tools/call", ["tool", "name"]]]);

export class Plugin {
  constructor() {
    this.state = new State(this);
    /** From the host's `hello`; `grants` is replaced on `granted`. */
    this.hello = {};
    this.grants = {};
    this.data_dir = null;
    this.clients = {};
    /** Ids of host requests the host cancelled (`notifications/cancelled`), updated as they arrive. */
    this.cancelled = new Set();
    this._handlers = {};
    for (const k of ["event", "action", "query", "command", "tool", "request", "notify"]) this._handlers[k] = new Map();
    this._hooks = { start: [], shutdown: [], granted: [], lagged: [], cancel: [], render: [] };
    this._out = process.stdout.write.bind(process.stdout); // the protocol, even while console is rerouted
    this._writes = this._nextId = 0;
    this._pending = new Map();
    this._sink = this._responder = null; // drive(): collects what the plugin sends / answers its requests
    this._drained = this._console = null;
    this._chain = Promise.resolve(); // handlers run serially on this chain
    this._started = this._closed = this._done = false;
  }

  // ---- registration (each returns the plugin) ----
  _reg(kind, name, fn) { this._handlers[kind].set(name, fn); return this; }
  /** Event handler. `name`: `agent.events#TurnFinished`, `agent.events`, `turns.limit_reached`, or `*`. */
  on(name, fn) { return this._reg("event", name, fn); }
  /** `action` request. Return a JSON value (null/undefined -> {ok: true}); throw RpcError for an error. */
  action(name, fn) { return this._reg("action", name, fn); }
  /** OpenUI `Query(name, …)`. The return value is the query's value. */
  query(name, fn) { return this._reg("query", name, fn); }
  /** `/plugin:name` command. Return a string (-> {text}), an object (as-is) or nothing (-> {}). */
  command(name, fn) { return this._reg("command", name, fn); }
  /** `tools/call` for a tool declared in plugin.yaml. Return a string, a CallToolResult, or any JSON. */
  tool(name, fn) { return this._reg("tool", name, fn); }
  /** Raw hatch: answer a host request method the helpers do not wrap; fn(params) -> result. */
  on_request(method, fn) { return this._reg("request", method, fn); }
  /** Raw hatch: handle a host notification method; fn(params). */
  on_notify(method, fn) { return this._reg("notify", method, fn); }

  _hook(kind, fn) { this._hooks[kind].push(fn); return this; }
  /** After `hello`, before the loop. Paint here. Also re-run on `render` unless on_render is set. */
  on_start(fn) { return this._hook("start", fn); }
  /** On `shutdown` or stdin EOF, before exit 0. */
  on_shutdown(fn) { return this._hook("shutdown", fn); }
  /** Grants changed; `plugin.grants` is already replaced. fn(grants). */
  on_granted(fn) { return this._hook("granted", fn); }
  /** Events were dropped; fn({dropped, since_ms}). Default: a warn log. */
  on_lagged(fn) { return this._hook("lagged", fn); }
  /** `notifications/cancelled`; fn({requestId, reason}). The id is already in `plugin.cancelled`. */
  on_cancel(fn) { return this._hook("cancel", fn); }
  /** `render` request; fn(req). Send a `ui`; the SDK replies {}. */
  on_render(fn) { return this._hook("render", fn); }

  // ---- plugin -> host notifications ----
  /** Raw notification. */
  send(method, params) { this._write({ jsonrpc: "2.0", method, params: params || {} }); }

  _placement(placement) {
    if (placement) return placement;
    const ids = this.grants?.placements || [];
    if (ids.length !== 1) throw new Error(`pass placement: the grant holds ${ids.length} placements ${JSON.stringify(ids)}`);
    return ids[0];
  }

  /** status(dot, badge?, label?, tooltip?, placement?) or status({dot, badge, …}). Dot: ok|busy|warn|error|off|none. */
  status(...a) {
    const { dot, badge, label, tooltip, placement } = opts(a, ["dot", "badge", "label", "tooltip", "placement"]);
    const p = { placement: this._placement(placement), dot };
    for (const [k, v] of Object.entries({ badge, label, tooltip })) if (v != null) p[k] = v;
    this.send("status", p);
  }

  /** ui(text, buttons?, source?, format?, placement?) or ui({text, buttons, …}). Buttons: [label, action, style?]. */
  ui(...a) {
    let { text, buttons, source, format, placement } = opts(a, ["text", "buttons", "source", "format", "placement"]);
    if (source == null) {
      if (format == null || format === "openui") [format, source] = ["openui", openui_card(text, buttons)];
      else source = text;
    }
    this.send("ui", { placement: this._placement(placement), format: format || "openui", text, source });
  }

  /** Publish a declared event. An undeclared name is dropped by the host. */
  emit(event, data, session) {
    if (!isObj(data)) throw new TypeError("emit data must be an object");
    this.send("emit", session == null ? { event, data } : { event, data, session });
  }

  /** notify(title, body?, level?, open?) or notify({title, …}). Inbox item (capability `notify`). */
  notify(...a) {
    const { title, body = "", level = "info", open } = opts(a, ["title", "body", "level", "open"]);
    this.send("notify", open == null ? { title, body, level } : { title, body, level, open });
  }

  /** Clients re-run the placement's OpenUI Queries, keeping `$state`. */
  invalidate(placement) { this.send("invalidate", { placement: this._placement(placement) }); }

  /** log(message, level?, fields?) or log(message, {level, fields}). */
  log(message, level = "info", fields) {
    if (isObj(level)) ({ level = "info", fields } = level);
    const p = { level, message: String(message) };
    if (fields && Object.keys(fields).length) p.fields = fields;
    this.send("log", p);
  }

  // ---- plugin -> host requests (each resolves to `result` or rejects with RpcError) ----
  /** Raw request. Rejects with RpcError -32006 once the host closed stdin. */
  async request(method, params) {
    params = params || {};
    const msg = { jsonrpc: "2.0", id: ++this._nextId, method, params };
    if (this._responder) {
      this._sink(msg);
      return this._responder(method, params);
    }
    if (this._closed) throw new RpcError(-32006, "host closed stdin");
    const done = new Promise((resolve, reject) => this._pending.set(msg.id, { resolve, reject }));
    this._write(msg);
    return done;
  }

  /** Host-brokered tool call. Resolves to the ToolOutput `{content, is_error, structured?}`. */
  call(tool, args) { return this.request("call", { tool, args: args || {} }); }

  /** Send text into a session as the user (capability `input`). Resolves to `{session}`. */
  input(text, session, delivery) {
    const p = { text, session: session ?? null };
    if (delivery != null) p.delivery = delivery;
    return this.request("input", p);
  }

  catalog(topic) { return this.request("catalog", topic ? { topic } : {}); }

  /** Resolves to `{now_ms}`. */
  ping() { return this.request("ping", {}); }

  // ---- I/O ----
  _write(msg) {
    if (this._sink) return void this._sink(msg);
    this._writes++;
    this._out(JSON.stringify(msg) + "\n", () => {
      if (--this._writes === 0 && this._drained) this._drained();
    });
  }

  _reply(id, result, error) {
    this._write(error ? { jsonrpc: "2.0", id, error } : { jsonrpc: "2.0", id, result: result ?? null });
  }

  async _exit(code) {
    if (this._writes) await new Promise((r) => { this._drained = r; }); // stdout is async on some pipes
    process.exit(code);
  }

  // ---- dispatch ----
  async _onHello(p) {
    if (p.protocol !== PROTOCOL) {
      const m = `unsupported protocol ${JSON.stringify(p.protocol)}; this SDK speaks ${PROTOCOL}`;
      if (this._sink) throw new Error(m);
      this.log(m, "error");
      return this._exit(1);
    }
    [this.hello, this.grants, this.data_dir, this.clients] = [p, p.grants || {}, p.data_dir ?? null, p.clients || {}];
    if (!process.env.THEUTH_PLUGIN_DATA && this.data_dir) [this.state._dir, this.state._data] = [this.data_dir, null];
    for (const fn of this._hooks.start) await fn();
  }

  /** Handle one host message. Resolves false on shutdown. */
  async _dispatch(msg) {
    const method = msg.method, p = msg.params || {};
    if ("id" in msg) {
      try {
        this._reply(msg.id, await this._request(method, p, msg));
      } catch (e) {
        const rpc = e instanceof RpcError; // any other error is a handler bug: -32603, the process stays up
        if (!rpc) this.log(`${method} failed: ${e}`, "error");
        this._reply(msg.id, null, rpc ? e.toJSON() : { code: -32603, message: errText(e) });
      }
      return true;
    }
    if (method === "shutdown") return false;
    if (method === "event") {
      const ev = { topic: p.topic, version: p.version, type: p.type, session: p.session, at_ms: p.at_ms, data: p.data, raw: msg };
      const h = this._handlers.event;
      const key = [`${ev.topic}#${ev.type}`, ev.topic, ev.type, "*"].find((k) => h.has(k));
      if (key !== undefined) await h.get(key)(ev);
    } else if (method === "granted") {
      this.grants = p.grants || {};
      for (const fn of this._hooks.granted) await fn(this.grants);
    } else if (method === "lagged") {
      if (!this._hooks.lagged.length) this.log(`lagged: ${p.dropped} events dropped`, "warn", p);
      for (const fn of this._hooks.lagged) await fn(p);
    } else if (method === "notifications/cancelled") {
      for (const fn of this._hooks.cancel) await fn(p);
    } else if (this._handlers.notify.has(method)) {
      await this._handlers.notify.get(method)(p);
    }
    return true;
  }

  async _request(method, p, msg) {
    const meta = p._meta?.theuth || {};
    const req = { placement: p.placement, name: undefined, args: p.args, session: Object.hasOwn(p, "session") ? p.session : meta.session,
      client: p.client, form: p.form, activation: p.activation, id: msg.id, raw: msg };
    const kind = KINDS.get(method);
    if (kind) {
      const [k, key] = kind;
      req.name = p[key];
      if (method === "tools/call") req.args = p.arguments || {};
      const fn = this._handlers[k].get(req.name);
      if (!fn) throw new RpcError(-32601, `${method} not handled: ${req.name}`);
      if (k === "tool") {
        try {
          return toolResult(await fn(req));
        } catch (e) {
          if (e instanceof RpcError) throw e;
          this.log(`tool ${req.name} failed: ${e}`, "error"); // the model must see the text
          return { content: [{ type: "text", text: errText(e) }], isError: true };
        }
      }
      const r = await fn(req);
      if (k === "action") return r ?? { ok: true };
      if (k === "command") return typeof r === "string" ? { text: r } : r ?? {};
      return r;
    }
    if (this._handlers.request.has(method)) return this._handlers.request.get(method)(p);
    const { render, start } = this._hooks;
    if (method === "render" && (render.length || start.length)) {
      for (const fn of render) await fn(req);
      if (!render.length) for (const fn of start) await fn();
      return {};
    }
    throw new RpcError(-32601, `method not found: ${method}`);
  }

  _manifest() {
    const h = this._handlers;
    const rows = [["tools", h.tool], ["actions", h.action], ["queries", h.query], ["commands", h.command], ["on", h.event]];
    return "# registered in code (plugin.yaml is the contract)\n" +
      rows.map(([k, m]) => `${k}: [${[...m.keys()].map((n) => JSON.stringify(n)).join(", ")}]\n`).join("");
  }

  // ---- run loop ----
  /** Serve stdin until `shutdown` or EOF, run on_shutdown hooks, exit 0. The returned promise never settles. */
  run() {
    const forever = new Promise(() => {});
    if (process.argv.slice(2).includes("--theuth-manifest")) {
      this._out(this._manifest(), () => process.exit(0));
      return forever;
    }
    this._console = { log: console.log, info: console.info, debug: console.debug };
    for (const k of Object.keys(this._console)) {
      console[k] = (...a) => { for (const line of fmt(...a).split("\n")) this.log(line, "info", { stdout: true }); };
    }
    const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
    rl.on("line", (line) => this._read(line));
    rl.on("close", () => this._eof());
    return forever;
  }

  /** Reader side: route responses to pending requests at once, queue everything else on the handler chain. */
  _read(line) {
    let msg;
    try { msg = JSON.parse(line.endsWith("\r") ? line.slice(0, -1) : line); } catch { return; }
    if (!isObj(msg)) return;
    if (!("method" in msg) && "id" in msg) {
      const slot = this._pending.get(msg.id);
      if (!slot) return;
      this._pending.delete(msg.id);
      if (!("error" in msg)) return slot.resolve(msg.result ?? null);
      const e = msg.error || {};
      return slot.reject(new RpcError(e.code ?? -32603, e.message ?? "", e.data));
    }
    if (msg.method === "notifications/cancelled") this.cancelled.add(msg.params?.requestId);
    if (this._started) {
      this._enqueue(msg.method, async () => { if (!(await this._dispatch(msg))) await this._finish(); });
    } else if (msg.method === "hello") {
      this._started = true;
      this._enqueue("hello", () => this._onHello(msg.params || {}));
    } else if ("id" in msg) {
      this._reply(msg.id, null, { code: -32601, message: "hello first" });
    }
  }

  _enqueue(label, fn) {
    this._chain = this._chain.then(() => this._done || fn()).catch((e) => this.log(`${label} failed: ${e}`, "error"));
  }

  _eof() {
    this._closed = true;
    for (const slot of this._pending.values()) slot.reject(new RpcError(-32006, "host closed stdin"));
    this._pending.clear();
    this._enqueue("shutdown", () => this._finish());
  }

  async _finish() {
    this._done = true;
    for (const fn of this._hooks.shutdown) await Promise.resolve().then(fn).catch((e) => this.log(`on_shutdown failed: ${e}`, "error"));
    Object.assign(console, this._console);
    await this._exit(0);
  }

  // ---- testing ----
  /**
   * Run host messages (objects or JSON strings) through the dispatcher without stdin. Resolves to what the
   * plugin sent. `responder(method, params)` answers the plugin's requests (throw RpcError to fail).
   */
  async drive(lines, responder) {
    const out = [];
    this._sink = (m) => out.push(m);
    this._responder = responder || noResponder;
    try {
      for (const line of lines) {
        const msg = typeof line === "string" ? JSON.parse(line) : line;
        if (msg.method === "hello") {
          await this._onHello(msg.params || {});
          continue;
        }
        if (msg.method === "notifications/cancelled") this.cancelled.add(msg.params?.requestId);
        if (!(await this._dispatch(msg))) {
          for (const fn of this._hooks.shutdown) await fn();
          break;
        }
      }
    } finally {
      this._sink = this._responder = null;
    }
    return out;
  }
}

for (const [alias, name] of [["onStart", "on_start"], ["onShutdown", "on_shutdown"], ["onGranted", "on_granted"],
  ["onLagged", "on_lagged"], ["onCancel", "on_cancel"], ["onRender", "on_render"], ["onRequest", "on_request"],
  ["onNotify", "on_notify"]]) Plugin.prototype[alias] = Plugin.prototype[name];

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const errText = (e) => (e instanceof Error ? e.message || e.name : String(e));
// Positional arguments, or one options object.
const opts = (a, names) => (a.length === 1 && isObj(a[0]) ? a[0] : Object.fromEntries(names.map((n, i) => [n, a[i]])));
const noResponder = (method) => { throw new RpcError(-32601, `drive(): no responder for ${method}`); };

function toolResult(r) {
  if (typeof r === "string") return { content: [{ type: "text", text: r }], isError: false };
  if (isObj(r) && ("content" in r || "isError" in r)) return r;
  const out = { content: [{ type: "text", text: JSON.stringify(r ?? null) ?? "null" }], isError: false };
  if (isObj(r)) out.structuredContent = r;
  return out;
}

/** The plugin. One per process. */
export const plugin = new Plugin();
