// 146 RESTART-CLARITY: the P1–P6 proof (docs/design/146-restart-clarity.md §Proof)
// against ONE real `t --headless` engine on a scratch data dir, the real `omp`
// ACP harness as the subagent and the 139 fake OpenAI provider
// (`scripts/durability-mesh/fake-openai.ts`) as the parent's model. Not CI.
// Driven by `scripts/restart-clarity-smoke.sh` / `just restart-clarity`.
//
//   bun smoke.ts [P1 P4 P5 P6 …]      (default: every proof; P2/P3 ride on P1)
//
// Never touches ~/.theuth: everything lives under SMOKE_DIR (default
// /tmp/146-smoke, wiped). P6 needs BACKUP_DB (a `.backup` copy of a real
// sessions.sqlite) and the `sqlite3` CLI.
import { Database } from "bun:sqlite";
import type { Subprocess } from "bun";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";

const ROOT = process.env.SMOKE_DIR ?? "/tmp/146-smoke";
const T_BIN = process.env.T_BIN ?? "target/debug/t";
const PORT = Number(process.env.SMOKE_PORT ?? 7961);
const FAKE_PORT = Number(process.env.FAKE_PORT ?? 7960);
const FAKE_URL = `http://127.0.0.1:${FAKE_PORT}/v1/`;
const OMP_BIN = process.env.OMP_BIN ?? "omp";
const BACKUP_DB = process.env.BACKUP_DB;
const HERE = import.meta.dir;
const DIR = `${ROOT}/engine`;
const WT = `${ROOT}/wt`;
const SLEEP_SECS = Number(process.env.CHILD_SLEEP_SECS ?? 150);

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
type Obj = { [k: string]: Json };
type RpcError = { code: number; message: string; data?: Json };

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const now = () => Date.now();
const T0 = now();
const ts = () => `${((now() - T0) / 1000).toFixed(1).padStart(6)}s`;
function log(msg: string) {
  console.log(`[${ts()}] ${msg}`);
}
const results: { proof: string; name: string; ok: boolean; detail: string }[] = [];
let current = "setup";
function check(name: string, ok: boolean, detail = "") {
  results.push({ proof: current, name, ok, detail });
  log(`${ok ? "✓" : "✗"} ${current} ${name}${detail ? ` — ${detail}` : ""}`);
}
async function waitFor<T>(what: string, f: () => T | Promise<T>, timeoutMs = 30_000, every = 200): Promise<NonNullable<T>> {
  const deadline = now() + timeoutMs;
  for (;;) {
    const v = await f();
    if (v) return v as NonNullable<T>;
    if (now() > deadline) throw new Error(`timeout waiting for ${what}`);
    await sleep(every);
  }
}
function isObj(v: unknown): v is Obj {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

// ---- the engine ---------------------------------------------------------------

type Ev = { at: number; ev: Obj };
class Engine {
  proc: Subprocess | null = null;
  token = "";
  events: Ev[] = [];
  startedAt = 0;
  private ws: WebSocket | null = null;
  get pid() {
    return this.proc?.pid ?? 0;
  }

  writeConfig() {
    mkdirSync(`${DIR}/acp`, { recursive: true });
    writeFileSync(
      `${DIR}/config.yaml`,
      // Block style: `set_model(…, persist)` / `add_provider` merge into an
      // `agent:` block but append a second `agent:` after a flow mapping, and
      // the duplicate key wins on the next boot (communication would go off).
      [
        "secrets:",
        "  key_store: file",
        "mesh:",
        "  relay: none",
        "  discovery: false",
        "  name: smoke146",
        `  bind: 127.0.0.1:${PORT + 1}`,
        "slack:",
        "  enabled: false",
        "signal:",
        "  enabled: false",
        "agent:",
        "  communication: true",
        "tools:",
        "  approvals: false",
        "  login_path: false",
        "durability:",
        "  resume: auto",
        "",
      ].join("\n"),
    );
    // The real harness: whatever `omp` is on PATH (or OMP_BIN).
    writeFileSync(`${DIR}/acp/agents.yaml`, `omp:\n  command: ${OMP_BIN}\n  args: [acp]\n  env: {}\n  description: omp ACP server over stdio (146 smoke)\n`);
  }

  async start() {
    this.startedAt = now();
    this.proc = Bun.spawn([T_BIN, "--headless", "--data-dir", DIR], {
      env: { ...process.env, THEUTH_RPC__LISTEN: `127.0.0.1:${PORT}`, THEUTH_LAUNCH: undefined, RUST_LOG: process.env.RUST_LOG ?? "info" },
      stdout: "ignore",
      stderr: "ignore",
    });
    await waitFor("token", () => existsSync(`${DIR}/token`), 60_000);
    this.token = readFileSync(`${DIR}/token`, "utf8").trim();
    await waitFor("rpc", async () => (await this.raw("info", [])).result !== undefined, 90_000, 250);
    await this.subscribe();
    log(`engine up: pid ${this.pid}, rpc :${PORT}`);
  }

  async raw(method: string, params: Json[]): Promise<{ result?: Json; error?: RpcError }> {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/`, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${this.token}` },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
        signal: AbortSignal.timeout(60_000),
      });
      const j: unknown = await res.json();
      if (!isObj(j)) return { error: { code: -1, message: `bad reply ${JSON.stringify(j)}` } };
      if (isObj(j.error)) return { error: j.error as unknown as RpcError };
      return { result: j.result as Json };
    } catch (e) {
      return { error: { code: -2, message: String(e) } };
    }
  }
  async rpc(method: string, params: Json[]): Promise<Json> {
    const r = await this.raw(method, params);
    if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
    return r.result ?? null;
  }

  async subscribe() {
    this.ws?.close();
    this.events = [];
    const ws = new WebSocket(`ws://127.0.0.1:${PORT}/`, { headers: { authorization: `Bearer ${this.token}` } });
    this.ws = ws;
    const { promise, resolve, reject } = Promise.withResolvers<void>();
    ws.onopen = () => {
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "subscribe_events", params: [null, true] }));
      resolve();
    };
    ws.onerror = (e) => reject(new Error(`ws: ${String(e)}`));
    await promise;
    ws.onmessage = (m) => {
      try {
        const j: unknown = JSON.parse(String(m.data));
        if (isObj(j) && isObj(j.params) && isObj(j.params.result) && typeof j.params.result.type === "string") {
          this.events.push({ at: now(), ev: j.params.result });
        }
      } catch {
        /* snapshot or non-JSON frame */
      }
    };
  }

  sql<T extends Obj>(q: string, ...params: (string | number)[]): T[] {
    const db = new Database(`${DIR}/sessions.sqlite`, { readonly: true });
    try {
      return db.query(q).all(...params) as T[];
    } finally {
      db.close();
    }
  }
  one<T extends Obj>(q: string, ...params: (string | number)[]): T | undefined {
    return this.sql<T>(q, ...params)[0];
  }

  /// `omp acp` processes this engine spawned (direct children).
  ompPids(): number[] {
    const out = Bun.spawnSync(["ps", "-ax", "-o", "pid=,ppid=,command="]).stdout.toString();
    return out
      .split("\n")
      .map((l) => l.trim().split(/\s+/))
      .filter((f) => f.length >= 3 && Number(f[1]) === this.pid && f.slice(2).join(" ").includes(" acp"))
      .map((f) => Number(f[0]));
  }

  /// `kill -9` the engine and the harness processes it spawned (ours).
  async crash() {
    if (!this.proc) return;
    const kids = this.ompPids();
    this.ws?.close();
    this.proc.kill("SIGKILL");
    await this.proc.exited;
    for (const k of kids) {
      try {
        process.kill(k, "SIGKILL");
      } catch {
        /* already gone */
      }
    }
    log(`engine pid ${this.pid} SIGKILLed (+ ${kids.length} omp acp: ${kids.join(",") || "none"})`);
    this.proc = null;
  }
  async stop() {
    if (!this.proc) return;
    const kids = this.ompPids();
    this.ws?.close();
    this.proc.kill("SIGTERM");
    const done = await Promise.race([this.proc.exited.then(() => true), sleep(15_000).then(() => false)]);
    if (!done) this.proc.kill("SIGKILL");
    for (const k of kids) {
      try {
        process.kill(k, "SIGKILL");
      } catch {
        /* already gone */
      }
    }
    this.proc = null;
  }
  logText() {
    const dir = `${DIR}/logs`;
    if (!existsSync(dir)) return "";
    return readdirSync(dir)
      .sort()
      .map((f) => readFileSync(`${dir}/${f}`, "utf8"))
      .join("");
  }
  /// Lines of the engine log written by the current process (after `startedAt`).
  bootLog() {
    const since = new Date(this.startedAt - 2_000).toISOString().slice(0, 19);
    return this.logText()
      .split("\n")
      .filter((l) => l.slice(0, 19) >= since);
  }
}

// ---- helpers ----------------------------------------------------------------

type Hist = { role: string; name?: string; text?: string; result?: [string, boolean] | null };
async function history(e: Engine, sid: string): Promise<Hist[]> {
  const loaded = await e.rpc("load_session", [sid, null, null, null]);
  return isObj(loaded) && Array.isArray(loaded.history) ? (loaded.history as unknown as Hist[]) : [];
}
async function newParent(e: Engine): Promise<string> {
  const sid = String(await e.rpc("new_session", []));
  await e.rpc("set_model", [sid, "fake/batch", true]);
  return sid;
}
const TASK = `Run exactly one shell command: \`sleep ${SLEEP_SECS}\` (foreground, do not background it, do not shorten it). When it finishes, reply with exactly the single word DONE and nothing else. If you are resumed after an interruption, run the same command again and then reply DONE.`;

/// Spawn an omp child named `name` under `parent`; return its session id.
async function spawnChild(e: Engine, parent: string, name: string): Promise<string> {
  const calls = [{ name: "spawn_subagent", args: { agent: "omp", name, cwd: WT, task: TASK } }];
  await e.rpc("send_input", [parent, `SMOKE ${JSON.stringify({ calls })}`, [], null, null]);
  const ev = await waitFor(
    `SubagentSpawned ${name}`,
    () => e.events.find((x) => x.ev.type === "SubagentSpawned" && x.ev.parent === parent && x.ev.name === name)?.ev ?? null,
    60_000,
  );
  return String(ev.child);
}
/// The child's last `messages` row as the 146 predicate sees it.
function lastRow(e: Engine, sid: string) {
  return e.one<{ role: string; call: number; body: string }>(
    "SELECT json_extract(body,'$.role') AS role, EXISTS (SELECT 1 FROM json_each(body,'$.content') WHERE json_type(value,'$.ToolCall') IS NOT NULL) AS call, substr(body,1,200) AS body FROM messages WHERE session_id = ? ORDER BY seq DESC LIMIT 1",
    sid,
  );
}
/// Wait until the child is mid-prompt: its last row is a `ToolCall` (omp running `sleep`) and its `omp acp` process is up.
async function waitMidPrompt(e: Engine, child: string) {
  const row = await waitFor(`${child.slice(0, 8)} mid-prompt (ToolCall row)`, () => {
    const r = lastRow(e, child);
    return r && r.role === "Assistant" && r.call ? r : null;
  }, 180_000, 500);
  const pids = e.ompPids();
  log(`child ${child.slice(0, 8)} mid-prompt: ${row.body.slice(0, 120)}… omp acp pids ${pids.join(",")}`);
  return pids;
}
function bootPassLines(e: Engine) {
  return e.bootLog().filter((l) => l.includes("146: boot pass"));
}
function field(line: string, name: string): string {
  return line.match(new RegExp(`${name}=(\\S+)`))?.[1] ?? "";
}
async function waitBootPass(e: Engine, pred: (l: string) => boolean, timeoutMs: number): Promise<string> {
  return waitFor("146: boot pass line", () => bootPassLines(e).find(pred) ?? null, timeoutMs, 500);
}

// ---- proofs -----------------------------------------------------------------

/// P1 + P2 + P3: a mid-prompt omp child of a parent that is NOT waiting on it
/// (the 125R1 shape: the `child_result` wait row is removed before the crash)
/// is respawned at boot, its process is back within 30 s, `list_sessions`
/// shows it running, and the parent gets its DONE later.
async function P1(e: Engine) {
  const parent = await newParent(e);
  const child = await spawnChild(e, parent, "w146");
  log(`parent ${parent.slice(0, 8)} spawned w146 ${child.slice(0, 8)}`);
  await waitFor("parent turn finished", () => e.events.find((x) => x.ev.type === "TurnFinished" && x.ev.session === parent) ?? null, 60_000);
  const oldPids = await waitMidPrompt(e, child);
  // The 125R1 shape: nothing waits on the child (a wait row would also trigger the 139 path).
  {
    const db = new Database(`${DIR}/sessions.sqlite`);
    db.run("PRAGMA busy_timeout = 5000");
    db.run("DELETE FROM waits WHERE kind = 'child_result' AND target = ?", [child]);
    db.close();
  }
  check("no child_result wait before the crash", e.sql("SELECT 1 AS x FROM waits WHERE target = ?", child).length === 0);
  await e.crash();
  await e.start();
  const t0 = e.startedAt;
  const scan = await waitFor("146 mid-prompt scan line", () => e.bootLog().find((l) => l.includes("146: mid-prompt ACP children")) ?? null, 60_000, 250);
  current = "P1";
  check("mid-prompt scan found the child", field(scan, "n") === "1", scan.slice(0, 160));
  const first = await waitBootPass(e, () => true, 60_000);
  check("boot pass line: child waiting", field(first, "waiting") === "1" && field(first, "lost") === "0", first.slice(-200));
  current = "P2";
  const back = await waitFor("new omp acp process", () => (e.ompPids().some((p) => !oldPids.includes(p)) ? e.ompPids() : null), 30_000, 250);
  check("omp process back within 30 s", true, `pids ${back.join(",")} after ${((now() - t0) / 1000).toFixed(1)} s`);
  current = "P1";
  const settled = await waitBootPass(e, (l) => field(l, "waiting") === "0", 180_000);
  check("boot pass: respawned=1 lost=0", field(settled, "respawned") === "1" && field(settled, "lost") === "0", settled.slice(-220));
  current = "P3";
  const running = await waitFor("list_sessions running", async () => {
    const list = await e.rpc("list_sessions", []);
    return Array.isArray(list) ? (list.find((s) => isObj(s) && s.id === child) ?? null) : null;
  }, 10_000, 250);
  check("list_sessions shows the child running during the respawned prompt", running.running === true, JSON.stringify({ running: running.running, name: running.name }));
  const att = await e.rpc("attention_list", []);
  const boot = Array.isArray(att) ? att.find((i) => isObj(i) && String(i.id).startsWith("resume:boot:")) : undefined;
  check("resume:boot item present with the counts", isObj(boot) && String(boot.title).includes("1 subagent back"), isObj(boot) ? String(boot.title) : "no item");
  current = "P2";
  const done = await waitFor("parent receives DONE", async () => (await history(e, parent)).find((h) => h.role === "user" && !(h.text ?? "").startsWith("SMOKE") && (h.text ?? "").includes(child) && (h.text ?? "").includes("DONE")) ?? null, (SLEEP_SECS + 240) * 1000, 2_000);
  check("parent received the child's DONE after the respawn", true, (done.text ?? "").replace(/\s+/g, " ").slice(0, 160));
  current = "P1";
}

/// P4: the same crash with the parent archived first → the child is marked lost
/// (note on the child, `<child_lost>` in the parent's messages, `resume:boot:*`
/// with lost=1 and the child's name); no respawn.
async function P4(e: Engine) {
  const parent = await newParent(e);
  const child = await spawnChild(e, parent, "w146b");
  await waitFor("parent turn finished", () => e.events.find((x) => x.ev.type === "TurnFinished" && x.ev.session === parent) ?? null, 60_000);
  await waitMidPrompt(e, child);
  await e.rpc("archive_session", [parent, true]);
  check("parent archived before the crash", e.one<{ archived: number }>("SELECT archived FROM sessions WHERE id = ?", parent)?.archived === 1);
  await e.crash();
  await e.start();
  const settled = await waitBootPass(e, (l) => field(l, "waiting") === "0", 120_000);
  check("boot pass: lost=1 with the child's name", field(settled, "lost") === "1" && settled.includes("w146b"), settled.slice(-220));
  const note = lastRow(e, child);
  check("local note on the child", note?.role === "System" && note.body.includes("not respawned: its parent is archived"), note?.body.slice(0, 160));
  const told = await waitFor("<child_lost> in the parent's messages", () => e.sql<{ body: string }>("SELECT body FROM messages WHERE session_id = ? AND body LIKE '%<child_lost %' ORDER BY seq DESC LIMIT 1", parent)[0] ?? null, 30_000);
  check("parent got <child_lost name=w146b …>", told.body.includes('name=\\"w146b\\"') || told.body.includes('name="w146b"'), told.body.replace(/\s+/g, " ").slice(0, 200));
  check("child not respawned (no omp acp process)", e.ompPids().length === 0, `pids ${e.ompPids().join(",") || "none"}`);
  const att = await waitFor("resume:boot attention item", async () => {
    const list = await e.rpc("attention_list", []);
    return Array.isArray(list) ? (list.find((i) => isObj(i) && String(i.id).startsWith("resume:boot:")) ?? null) : null;
  }, 30_000, 500);
  check("attention item names the lost child", String(att.title).includes("1 lost (w146b)"), String(att.title));
  check("attention item is dismissable (carries a session)", typeof att.session === "string", String(att.session));
}

/// P5: a second `t --headless` on the same data dir names the holder and exits 0 within 1 s; the first is unaffected.
async function P5(e: Engine) {
  const t0 = now();
  const second = Bun.spawn([T_BIN, "--headless", "--data-dir", DIR], {
    env: { ...process.env, THEUTH_RPC__LISTEN: `127.0.0.1:${PORT + 5}`, THEUTH_LAUNCH: undefined },
    stdout: "pipe",
    stderr: "pipe",
  });
  const code = await Promise.race([second.exited, sleep(10_000).then(() => -1)]);
  const ms = now() - t0;
  const err = await new Response(second.stderr).text();
  if (code === -1) second.kill("SIGKILL");
  check("second engine exits 0 within 1 s", code === 0 && ms <= 1_000, `exit ${code} after ${ms} ms`);
  check("guard line names the holder's pid", err.includes(`engine already running (pid ${e.pid}`), err.trim().split("\n").pop() ?? "");
  const info = await e.raw("info", []);
  check("first engine unaffected", info.result !== undefined, info.error?.message ?? "info ok");
}

/// P6: `open_turns` before/after on a `.backup` copy of a real store (sqlite3 CLI timing, 3 runs each).
async function P6() {
  if (!BACKUP_DB || !existsSync(BACKUP_DB)) {
    check("BACKUP_DB set", false, "set BACKUP_DB=/path/to/sessions.backup.sqlite (sqlite3 \".backup\" of a real store)");
    return;
  }
  const old =
    "SELECT count(*) FROM (SELECT o.session_id FROM (SELECT session_id, turn_id, kind, ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY n DESC) AS rn, (SELECT MAX(at) FROM turn_steps t2 WHERE t2.session_id = t1.session_id AND t2.turn_id = t1.turn_id) AS last_at FROM turn_steps t1 WHERE kind != 'assistant_partial') o JOIN sessions s ON s.id = o.session_id WHERE o.rn = 1 AND o.kind != 'turn_finished' AND (s.owner = '' OR s.owner = 'x') ORDER BY o.last_at DESC);";
  const neu =
    "SELECT count(*) FROM (SELECT o.session_id FROM (SELECT session_id, turn_id, kind, ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY (kind = 'assistant_partial'), n DESC) AS rn, MAX(at) OVER (PARTITION BY session_id, turn_id) AS last_at FROM turn_steps) o JOIN sessions s ON s.id = o.session_id WHERE o.rn = 1 AND o.kind != 'turn_finished' AND (s.owner = '' OR s.owner = 'x') ORDER BY o.last_at DESC);";
  const time = (q: string) => {
    // `.timer on` only prints when the statements arrive on stdin.
    const r = Bun.spawnSync(["sqlite3", BACKUP_DB], { stdin: Buffer.from(`.timer on\n${q}\n`) });
    return Number((r.stdout.toString() + r.stderr.toString()).match(/real (\d+\.\d+)/)?.[1] ?? NaN);
  };
  const med = (xs: number[]) => xs.sort((a, b) => a - b)[Math.floor(xs.length / 2)];
  const rows = Bun.spawnSync(["sqlite3", BACKUP_DB, "SELECT count(*) || ' rows / ' || count(DISTINCT session_id || turn_id) || ' turns' FROM turn_steps"]).stdout.toString().trim();
  const before = med([time(old), time(old), time(old)]);
  const after = med([time(neu), time(neu), time(neu)]);
  Bun.spawnSync(["sqlite3", BACKUP_DB, "CREATE INDEX IF NOT EXISTS turn_steps_turn_at ON turn_steps(session_id, turn_id, at)"]);
  const beforeIdx = med([time(old), time(old), time(old)]);
  const afterIdx = med([time(neu), time(neu), time(neu)]);
  Bun.spawnSync(["sqlite3", BACKUP_DB, "DROP INDEX IF EXISTS turn_steps_turn_at"]);
  check("window rewrite is faster than the correlated subquery", after < before, `${rows}: old ${before}s → new ${after}s (with 0042 index: old ${beforeIdx}s, new ${afterIdx}s)`);
}

// ---- main -------------------------------------------------------------------

async function main() {
  const wanted = Bun.argv.slice(2).length ? Bun.argv.slice(2) : ["P1", "P4", "P5", "P6"];
  rmSync(ROOT, { recursive: true, force: true });
  mkdirSync(WT, { recursive: true });
  Bun.spawnSync(["git", "init", "-q", WT]);
  const fake = Bun.spawn(["bun", `${HERE}/../durability-mesh/fake-openai.ts`, String(FAKE_PORT), `${ROOT}/fake-requests.ndjson`], { stdout: "inherit", stderr: "inherit" });
  await waitFor("fake provider", () => fetch(`${FAKE_URL}models`).then((r) => r.ok, () => false), 10_000, 100);
  const e = new Engine();
  const cleanup = async () => {
    await e.stop().catch(() => {});
    fake.kill();
  };
  process.on("SIGINT", () => void cleanup().then(() => process.exit(130)));
  try {
    e.writeConfig();
    await e.start();
    log(`add_provider → ${JSON.stringify(await e.rpc("add_provider", ["fake", FAKE_URL, "smoke-key"]))}`);
    const proofs: Record<string, () => Promise<void>> = { P1: () => P1(e), P4: () => P4(e), P5: () => P5(e), P6 };
    for (const name of wanted) {
      const p = proofs[name];
      if (!p) {
        log(`unknown proof ${name}`);
        continue;
      }
      current = name;
      const t = now();
      try {
        await p();
      } catch (err) {
        check("proof completed", false, String(err));
      }
      log(`${name} took ${((now() - t) / 1000).toFixed(1)} s`);
    }
  } finally {
    await cleanup();
  }
  console.log("\n| proof | check | result | detail |\n|---|---|---|---|");
  for (const r of results) console.log(`| ${r.proof} | ${r.name} | ${r.ok ? "✓" : "✗"} | ${r.detail.replace(/\|/g, "\\|").replace(/\n/g, " ")} |`);
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed; data dir ${DIR} (log in ${DIR}/logs/)`);
  process.exit(failed ? 1 : 0);
}

await main();
