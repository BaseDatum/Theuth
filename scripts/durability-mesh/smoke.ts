// 139 Z: the two-engine matrix (design 139 §11 Z, T1–T8; R2 adds T10/T10b)
// against two real engines on one Mac, the shape of 68-two-engine-smoke.md §1
// and 109-as-built-W6.md §8. Not CI. Driven by `scripts/durability-mesh-smoke.sh`.
//
//   bun smoke.ts [T1 T2 …]      (default: every row)
//
// Engines A and B run `t --headless` on scratch data dirs, paired over the
// loopback mesh, with the fake OpenAI provider (`fake-openai.ts`) as their
// model. A row opens a session on A, sends `SMOKE {calls}` so the model
// answers with exactly that tool batch (a `shell` that `sleep`s holds the
// batch `started`), then takes / partitions / restarts and asserts on both
// engines' RPC replies, event streams and `sessions.sqlite` journals.
import { Database } from "bun:sqlite";
import type { Subprocess } from "bun";
import { mkdirSync, rmSync, existsSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { isObj, log, makeWaitFor, now, rawRpc, rpcResult, sleep, subscribeEvents, type Ev, type Json, type Obj } from "../lib/harness.ts";

const ROOT = process.env.SMOKE_DIR ?? "/tmp/139-z";
const T_BIN = process.env.T_BIN ?? "target/debug/t";
const FAKE_PORT = Number(process.env.FAKE_PORT ?? 7950);
const FAKE_URL = `http://127.0.0.1:${FAKE_PORT}/v1/`;
const RELEASE_WAIT_SECS = 6;
const DRAIN_SECS = 10;
const HERE = import.meta.dir;

const results: { row: string; name: string; ok: boolean; detail: string }[] = [];
let currentRow = "setup";
function check(name: string, ok: boolean, detail = "") {
  results.push({ row: currentRow, name, ok, detail });
  log(`${ok ? "PASS" : "FAIL"} ${currentRow} ${name}${detail ? ` — ${detail}` : ""}`);
}

const waitFor = makeWaitFor(20_000, 100);

function str(v: Json | undefined): string {
  return typeof v === "string" ? v : JSON.stringify(v);
}

class Engine {
  proc: Subprocess | null = null;
  token = "";
  id = "";
  events: Ev[] = [];
  private ws: WebSocket | null = null;
  constructor(
    readonly name: string,
    readonly port: number,
    readonly bind: number,
  ) {}
  get dir() {
    return `${ROOT}/${this.name}`;
  }
  get pid() {
    return this.proc?.pid ?? 0;
  }

  writeConfig() {
    mkdirSync(this.dir, { recursive: true });
    writeFileSync(
      `${this.dir}/config.yaml`,
      [
        "secrets: { key_store: file }",
        `mesh: { relay: none, discovery: false, name: ${this.name}, bind: 127.0.0.1:${this.bind} }`,
        "slack: { enabled: false }",
        "signal: { enabled: false }",
        // spawn_subagent (T2's native-child variant) needs model-to-model communication.
        "agent: { communication: true }",
        // `shell` stays inline for 120 s so a `sleep` holds its call `started`; no approval prompts.
        "tools: { approvals: false, job_grace_secs: 120, login_path: false }",
        `sessions: { release_wait_secs: ${RELEASE_WAIT_SECS}, takeover_secs: 0 }`,
        `durability: { resume: auto, drain_secs: ${DRAIN_SECS}, pty_snapshot_secs: 1 }`,
        "",
      ].join("\n"),
    );
  }

  async start() {
    // The engine writes `<data_dir>/logs/theuth.log.<date>` itself (`logText`).
    this.proc = Bun.spawn([T_BIN, "--headless", "--data-dir", this.dir], {
      env: { ...process.env, THEUTH_RPC__LISTEN: `127.0.0.1:${this.port}`, THEUTH_LAUNCH: undefined, RUST_LOG: process.env.RUST_LOG ?? "info" },
      stdout: "ignore",
      stderr: "ignore",
    });
    await waitFor(`${this.name} token`, () => existsSync(`${this.dir}/token`), 60_000, 200);
    this.token = readFileSync(`${this.dir}/token`, "utf8").trim();
    await waitFor(`${this.name} rpc`, async () => (await this.raw("info", [])).result !== undefined, 60_000, 250);
    const mesh = await waitFor(`${this.name} mesh`, async () => {
      const r = await this.raw("mesh_status", []);
      return isObj(r.result) && typeof r.result.self_id === "string" ? r.result : null;
    }, 60_000, 250);
    this.id = String(mesh.self_id);
    await this.subscribe();
    log(`${this.name} up: pid ${this.pid}, rpc :${this.port}, id ${this.id}`);
  }

  raw(method: string, params: Json[]) {
    return rawRpc(this.port, this.token, method, params);
  }

  async rpc(method: string, params: Json[]): Promise<Json> {
    return rpcResult(await this.raw(method, params), method, `${this.name} `);
  }

  /// All events of every session; kept for the whole run.
  async subscribe() {
    this.ws?.close();
    this.events = [];
    this.ws = await subscribeEvents(this.port, this.token, `ws ${this.name}`, (ev) => this.events.push({ at: now(), ev }));
  }

  evs(session: string, type: string): Obj[] {
    return this.events.filter((e) => e.ev.type === type && e.ev.session === session).map((e) => e.ev);
  }
  async waitEv(session: string, type: string, timeoutMs = 30_000, pred: (e: Obj) => boolean = () => true): Promise<Obj> {
    return waitFor(`${this.name} ${type} of ${session.slice(0, 8)}`, () => this.evs(session, type).find(pred) ?? null, timeoutMs);
  }

  sql<T extends Obj>(q: string, ...params: (string | number)[]): T[] {
    const db = new Database(`${this.dir}/sessions.sqlite`, { readonly: true });
    try {
      return db.query(q).all(...params) as T[];
    } finally {
      db.close();
    }
  }
  one<T extends Obj>(q: string, ...params: (string | number)[]): T | undefined {
    return this.sql<T>(q, ...params)[0];
  }

  paused = false;
  pause() {
    if (this.paused || !this.proc) return;
    process.kill(this.pid, "SIGSTOP");
    this.paused = true;
    log(`${this.name} SIGSTOP`);
  }
  resume() {
    if (!this.paused || !this.proc) return;
    process.kill(this.pid, "SIGCONT");
    this.paused = false;
    log(`${this.name} SIGCONT`);
  }
  /// SIGSTOP this engine and wait until `peer`'s mesh marks it unreachable
  /// (the QUIC idle timeout, ~20–30 s): a `Write` take or a forward from
  /// `peer` then sees "owner unreachable" instead of waiting on a release.
  async partitionFrom(peer: Engine) {
    this.pause();
    const t0 = now();
    await waitFor(`${peer.name} sees ${this.name} unreachable`, async () => {
      const s = await peer.rpc("mesh_status", []);
      return isObj(s) && Array.isArray(s.peers) && s.peers.some((p) => isObj(p) && p.id === this.id && p.reachable === false);
    }, 120_000, 500);
    log(`${peer.name} saw ${this.name} unreachable after ${now() - t0} ms`);
  }
  async stop(signal: "SIGTERM" | "SIGKILL" = "SIGTERM") {
    if (!this.proc) return;
    this.ws?.close();
    this.proc.kill(signal);
    await this.proc.exited;
    this.proc = null;
    log(`${this.name} exited (${signal})`);
  }
  /// The engine's own tracing log (`<data_dir>/logs/theuth.log.<date>`), all days concatenated.
  logText() {
    const dir = `${this.dir}/logs`;
    if (!existsSync(dir)) return "";
    return readdirSync(dir).sort().map((f) => readFileSync(`${dir}/${f}`, "utf8")).join("");
  }
}

// ---- journal readers --------------------------------------------------------

type ToolRow = { call_id: string; name: string; state: string; output: string | null };
function calls(e: Engine, sid: string, turn: string): ToolRow[] {
  return e.sql<ToolRow>("SELECT call_id, name, state, output FROM tool_calls WHERE session_id = ? AND turn_id = ? ORDER BY at_seq, rowid", sid, turn);
}
function callState(e: Engine, sid: string, turn: string, name: string) {
  return calls(e, sid, turn).find((c) => c.name === name)?.state;
}
function newestStep(e: Engine, sid: string) {
  return e.one<{ turn_id: string; kind: string; payload: string }>("SELECT turn_id, kind, payload FROM turn_steps WHERE session_id = ? ORDER BY at DESC, n DESC LIMIT 1", sid);
}
function suspendedReasons(e: Engine, sid: string, turn: string): string[] {
  return e
    .sql<{ payload: string }>("SELECT payload FROM turn_steps WHERE session_id = ? AND turn_id = ? AND kind = 'suspended' ORDER BY n", sid, turn)
    .map((r) => String(JSON.parse(r.payload).reason));
}
function ownerRow(e: Engine, sid: string) {
  return e.one<{ owner: string; epoch: number }>("SELECT owner, epoch FROM sessions WHERE id = ?", sid);
}
type JobRow = { id: string; kind: string; pgid: number | null; state: string; exit: string | null; engine: string };
function jobs(e: Engine, sid: string): JobRow[] {
  return e.sql<JobRow>("SELECT id, kind, pgid, state, exit, engine FROM jobs WHERE session_id = ? ORDER BY id", sid);
}
function ownLogRows(e: Engine, sid: string): number {
  return Number(e.one<{ n: number }>("SELECT count(*) AS n FROM session_log WHERE session_id = ?", sid)?.n ?? 0);
}
function report(e: Engine, sid: string) {
  return e.one<{ cause: string; replayed: string; interrupted: string; lost_jobs: string }>("SELECT cause, replayed, interrupted, lost_jobs FROM resume_reports WHERE session_id = ? ORDER BY at DESC LIMIT 1", sid);
}
function pgAlive(pgid: number | null): boolean {
  if (!pgid) return false;
  try {
    process.kill(-pgid, 0);
    return true;
  } catch {
    return false;
  }
}
function hits(file: string): number {
  return existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean).length : 0;
}

// ---- turns ------------------------------------------------------------------

type Call = { name: string; args: Obj };
const FIXTURE = `${ROOT}/fixture.txt`;
const readFixture: Call = { name: "read_file", args: { path: FIXTURE } };
const heldShell = (hitsFile: string, secs: number): Call => ({ name: "shell", args: { command: `echo hit >> ${hitsFile}; sleep ${secs}` } });
// Third call of the batch (quick, read-only): `todo` would add a `<todo_continue>` nudge turn.
const listDir: Call = { name: "list_dir", args: { path: ROOT } };

async function newSession(e: Engine): Promise<string> {
  const sid = String(await e.rpc("new_session", []));
  await e.rpc("set_model", [sid, "fake/batch", true]);
  return sid;
}

/// Send the batch; return once `turn` is known and `held` (a call name) is
/// `started` on every engine in `visibleOn`.
async function sendBatch(e: Engine, sid: string, batch: Call[], held: string | null, visibleOn: Engine[]): Promise<string> {
  const before = e.evs(sid, "TurnStarted").length;
  await e.rpc("send_input", [sid, `SMOKE ${JSON.stringify({ calls: batch })}`, [], null, null]);
  const started = await waitFor(`${e.name} TurnStarted of ${sid.slice(0, 8)}`, () => e.evs(sid, "TurnStarted")[before] ?? null, 30_000);
  const turn = String(started.turn);
  if (held) {
    for (const v of visibleOn) {
      await waitFor(`${v.name} sees ${held} started`, () => callState(v, sid, turn, held) === "started", 30_000);
    }
  }
  return turn;
}

type Hist = { role: string; name?: string; text?: string; result?: [string, boolean] | null };
async function history(e: Engine, sid: string): Promise<Hist[]> {
  const loaded = await e.rpc("load_session", [sid, null, null, null]);
  return isObj(loaded) && Array.isArray(loaded.history) ? (loaded.history as unknown as Hist[]) : [];
}
function toolResult(h: Hist[], name: string): string {
  return h.find((x) => x.role === "tool" && x.name === name)?.result?.[0] ?? "";
}
/// `role[:name]` sequence: what "equal modulo the notes" compares. `error`
/// cards (the old owner's "background work outcome unknown: j1 killed when
/// the session moved") are notes about the move, not transcript.
function shape(h: Hist[]): string {
  return h.filter((x) => x.role !== "error").map((x) => (x.role === "tool" ? `tool:${x.name}` : x.role)).join(" ");
}

// ---- rows -------------------------------------------------------------------

async function T1(A: Engine, B: Engine) {
  // Replica boots while the owner is mid-turn: B resumes nothing, writes
  // nothing; A's turn finishes with the uninterrupted transcript.
  const sid = await newSession(A);
  const hitsFile = `${ROOT}/hits-T1`;
  const turn = await sendBatch(A, sid, [readFixture, heldShell(hitsFile, 25), listDir], "shell", [A, B]);
  const logBefore = B.logText().length;
  await B.stop("SIGTERM");
  await B.start();
  const bootLog = B.logText().slice(logBefore);
  const t0 = now();
  await A.waitEv(sid, "TurnFinished", 60_000);
  await waitFor("B has A's turn_finished", () => newestStep(B, sid)?.kind === "turn_finished", 20_000);
  check("B published no TurnResumed and no ResumePending", B.evs(sid, "TurnResumed").length === 0 && B.evs(sid, "ResumePending").length === 0);
  check("B's boot log resumed nothing", !/109: resuming turn|resume refused/.test(bootLog), bootLog.match(/109: .*open turn.*/)?.[0] ?? "(no open-turn line)");
  check("B wrote nothing: no own log rows, no resume report", ownLogRows(B, sid) === 0 && !report(B, sid));
  const h = await history(A, sid);
  check("A's transcript uninterrupted", !h.some((x) => x.role === "tool" && /<engine_restart/.test(x.result?.[0] ?? "")) && toolResult(h, "shell").includes("hit") === false && hits(hitsFile) === 1, shape(h));
  const owner = ownerRow(B, sid);
  check("B still a replica at epoch 1", owner?.owner === A.id && owner.epoch === 1, JSON.stringify(owner));
  log(`T1 done in ${now() - t0} ms after boot`);
  void turn;
}

async function T2(A: Engine, B: Engine) {
  // Transfer mid-batch: A reachable, `shell` held; B takes with `user`.
  const sid = await newSession(A);
  const hitsFile = `${ROOT}/hits-T2`;
  const turn = await sendBatch(A, sid, [readFixture, heldShell(hitsFile, 90), listDir], "shell", [A, B]);
  await waitFor("read_file finished on B", () => callState(B, sid, turn, "read_file") === "finished");
  const t0 = now();
  const take = await B.rpc("session_take", [sid, "user"]);
  const took = now() - t0;
  check("session_take(user) on B → owner B, epoch 2, here", isObj(take) && take.owner === B.id && take.epoch === 2 && take.here === true, `${JSON.stringify(take)} in ${took} ms`);
  const resumed = await B.waitEv(sid, "TurnResumed");
  check("B TurnResumed{cause: transfer, from: A, kind: suspended}", resumed.cause === "transfer" && resumed.from === A.id && resumed.kind === "suspended", JSON.stringify({ cause: resumed.cause, from: resumed.from, kind: resumed.kind }));
  check("suspended{release} is on B (applied at epoch 1 — a later arrival would be StaleEpoch)", suspendedReasons(B, sid, turn).includes("release") && ownerRow(B, sid)?.epoch === 2, suspendedReasons(B, sid, turn).join(","));
  const susp = await A.waitEv(sid, "TurnSuspended", 5_000).catch(() => null);
  check("A published TurnSuspended{release}", susp?.reason === "release", JSON.stringify(susp));
  await A.waitEv(sid, "SessionOwnerChanged", 20_000, (e) => e.here === false);
  const orphans = await A.rpc("session_orphans", [sid]);
  check("A demoted with session_orphans = 0", Array.isArray(orphans) && orphans.length === 0, JSON.stringify(orphans));
  await B.waitEv(sid, "TurnFinished", 60_000);
  check("B published exactly one TurnResumed", B.evs(sid, "TurnResumed").length === 1);
  const h = await history(B, sid);
  const shellOut = toolResult(h, "shell");
  check("shell (mutating, started) reported interrupted with the moved-here job note", /interrupted/.test(shellOut) && /moved here from|moved to this engine/.test(shellOut) && shellOut.includes(A.id), shellOut.split("\n")[0]);
  check("mutating call ran exactly once across A and B", hits(hitsFile) === 1, `${hits(hitsFile)} hit(s)`);
  check("read_file output reused (finished row)", toolResult(h, "read_file").includes("fixture-139"), toolResult(h, "read_file").split("\n")[1] ?? "");
  const note = h.find((x) => x.role === "tool" && /<engine_restart/.test(x.result?.[0] ?? ""))?.result?.[0] ?? "";
  check("<engine_restart kind=\"transfer\" from=A> heads the resumed batch", /<engine_restart[^>]*kind="transfer"/.test(note) && note.includes(`from="${A.id}"`), note.match(/<engine_restart[^>]*>/)?.[0] ?? "(none)");
  const rep = report(B, sid);
  check("resume_reports row on B: cause transfer, shell interrupted, lost job", rep?.cause === "transfer" && /shell/.test(rep.interrupted) && rep.lost_jobs !== "[]", JSON.stringify(rep));
  // Control: the same batch, uninterrupted, on A.
  const ctl = await newSession(A);
  await sendBatch(A, ctl, [readFixture, heldShell(`${ROOT}/hits-T2-control`, 1), listDir], null, []);
  await A.waitEv(ctl, "TurnFinished", 60_000);
  const hc = await history(A, ctl);
  check("transcript shape equals the uninterrupted run's (modulo notes)", shape(h) === shape(hc) && h.at(-1)?.text === hc.at(-1)?.text, `taken: ${shape(h)} | control: ${shape(hc)} | finals "${h.at(-1)?.text}" / "${hc.at(-1)?.text}"`);
  // Jobs: A's shell group was killed by the release drain (KillSession).
  const j = jobs(B, sid);
  check("job row lost on B with engine = A", j.length === 1 && j[0].state === "lost" && j[0].engine === A.id, JSON.stringify(j));
  const pgid = jobs(A, sid)[0]?.pgid ?? null;
  check("A's shell process group dead after the transfer", pgid !== null && !pgAlive(pgid), `pgid ${pgid}`);
  const ha = await history(A, sid);
  check("A's copy equals B's (B's rows overwrote A's)", shape(ha) === shape(h), shape(ha));
}

async function T2c(A: Engine, B: Engine) {
  // T2's native-child variant: the parent's batch spawns a child on A and
  // holds; B takes the parent; the child (still on A) finishes afterwards
  // and its result reaches the parent on B (69 P: peers follow the owner).
  await A.rpc("set_subagent_model", ["fake/batch"]);
  const sid = await newSession(A);
  const kidTask = `SMOKE ${JSON.stringify({ calls: [{ name: "shell", args: { command: "sleep 20" } }] })}`;
  const turn = await sendBatch(A, sid, [{ name: "spawn_subagent", args: { name: "kid", task: kidTask } }, heldShell(`${ROOT}/hits-T2c`, 90)], "shell", [A, B]);
  const kid = await waitFor("child session on A", () => A.one<{ id: string; owner: string }>("SELECT id, owner FROM sessions WHERE parent = ?", sid), 20_000);
  check("child spawned on A, owned by A", kid.owner === A.id, kid.id);
  await waitFor("child's turn running on A", () => callState(A, kid.id, newestStep(A, kid.id)?.turn_id ?? "", "shell") === "started", 30_000);
  await B.rpc("session_take", [sid, "user"]);
  const resumed = await B.waitEv(sid, "TurnResumed");
  check("parent resumed on B as a transfer", resumed.cause === "transfer");
  await B.waitEv(sid, "TurnFinished", 60_000);
  check("child still owned by A after the parent's take (children are not co-owned)", ownerRow(A, kid.id)?.owner === A.id && ownerRow(B, kid.id)?.owner === A.id);
  await A.waitEv(kid.id, "TurnFinished", 60_000);
  const delivered = await waitFor("child's result on B", async () => (await history(B, sid)).find((x) => x.role === "user" && (x.text ?? "").includes(`session:${kid.id}`)) ?? null, 60_000, 500);
  check("the child's result arrived in the parent's transcript on B", !!delivered, (delivered.text ?? "").split("\n")[0].slice(0, 160));
  await B.waitEv(sid, "TurnFinished", 60_000, () => B.evs(sid, "TurnFinished").length >= 2).catch(() => null);
  check("B ran a turn on the delivered result", B.evs(sid, "TurnFinished").length >= 2, `${B.evs(sid, "TurnFinished").length} TurnFinished on B`);
  void turn;
}

async function T3(A: Engine, B: Engine) {
  // Takeover: A partitioned (SIGSTOP) mid-batch; `user` take on B. Bridged
  // calls run one python cell at a time per session, so one session holds
  // the mutating `shell`, another the read-only `wait` on its own PTY. Then
  // A runs on alone (B stopped), writes its tail, and demotes with orphans
  // when B comes back.
  const hitsFile = `${ROOT}/hits-T3`;
  const sm = await newSession(A); // mutating started
  const sr = await newSession(A); // read-only started
  await sendBatch(A, sr, [{ name: "start", args: { command: "printf 'HELLO-T3\\n'; sleep 300", name: "t3", ready: { log: "HELLO" } } }], null, []);
  await A.waitEv(sr, "TurnFinished", 30_000);
  const pty = jobs(A, sr).find((j) => j.kind === "pty");
  check("a PTY job runs on A", !!pty && pgAlive(pty.pgid), JSON.stringify(pty));
  const tm = await sendBatch(A, sm, [readFixture, heldShell(hitsFile, 25)], "shell", [A, B]);
  const tr = await sendBatch(A, sr, [{ name: "wait", args: { id: pty?.id ?? "j1", pattern: "NEVER-MATCHES", timeout: 100 } }], "wait", [A, B]);
  await A.partitionFrom(B);
  const t0 = now();
  const take = await B.rpc("session_take", [sm, "user"]);
  const took = now() - t0;
  check("session_take(user) on B with A partitioned → B, epoch 2", isObj(take) && take.owner === B.id && take.epoch === 2, `${JSON.stringify(take)} in ${took} ms`);
  await B.rpc("session_take", [sr, "user"]);
  const resumed = await B.waitEv(sm, "TurnResumed");
  check("B TurnResumed{cause: takeover, from: A, kind: interrupted}", resumed.cause === "takeover" && resumed.from === A.id && resumed.kind === "interrupted", JSON.stringify({ cause: resumed.cause, from: resumed.from, kind: resumed.kind }));
  check("no suspended row shipped", suspendedReasons(B, sm, tm).length === 0 && suspendedReasons(B, sr, tr).length === 0);
  await B.waitEv(sm, "TurnFinished", 60_000);
  await B.waitEv(sr, "TurnResumed");
  await B.waitEv(sr, "TurnFinished", 60_000);
  const hm = await history(B, sm);
  const hr = await history(B, sr);
  const shellOut = toolResult(hm, "shell");
  const waitOut = toolResult(hr, "wait");
  check("A's started mutating `shell` reported interrupted (takeover wording)", /interrupted/.test(shellOut) && /took the session over|moved here from/.test(shellOut) && shellOut.includes(A.id), shellOut.split("\n")[0]);
  check("started read-only `wait` re-run on B: lost PTY, moved-from-A banner over A's last page", /process gone — the session moved from/.test(waitOut) && waitOut.includes(A.id) && /HELLO-T3/.test(waitOut), waitOut.split("\n").find((l) => /process gone/.test(l))?.slice(0, 120) ?? waitOut.slice(0, 120));
  check("<engine_restart kind=\"takeover\"> note on both", [hm, hr].every((h) => h.some((x) => x.role === "tool" && /<engine_restart[^>]*kind="takeover"/.test(x.result?.[0] ?? ""))));
  check("read_file output reused (finished row)", toolResult(hm, "read_file").includes("fixture-139"));
  // A alone: B stopped, A resumed; A's shell finishes and A closes its turn under epoch 1.
  B.pause();
  A.resume();
  await waitFor("A's turn finished on its own", () => newestStep(A, sm)?.kind === "turn_finished", 60_000, 250);
  const aLog = A.sql<{ seq: number; epoch: number }>("SELECT seq, epoch FROM session_log WHERE session_id = ? ORDER BY seq", sm);
  check("A wrote a tail at epoch 1 while partitioned", aLog.some((r) => r.epoch === 1) && newestStep(A, sm)?.kind === "turn_finished", `${aLog.length} log rows`);
  check("mutating call ran exactly once (A's run; B did not re-run it)", hits(hitsFile) === 1, `${hits(hitsFile)} hit(s)`);
  B.resume();
  await A.waitEv(sm, "SessionOwnerChanged", 90_000, (e) => e.here === false);
  const demoted = await waitFor("A's demote log line", () => A.logText().includes(`${sm} taken by ${B.id}`), 30_000, 250).catch(() => false);
  const orphans = await A.rpc("session_orphans", [sm]);
  const dropped = B.logText().includes(`dropped stale rows of ${sm} epoch 1 from ${A.id}`);
  check("A ran demote when B's take reached it (log row or snapshot)", demoted === true, A.logText().split("\n").find((l) => l.includes(`${sm} taken by`))?.slice(0, 200) ?? "(no demote line)");
  // 68 §4.3 as built: orphans are the rows the taker never acked. When the reconnect
  // ships A's tail before B's take row lands on A, B drops it as StaleEpoch and acks,
  // so A counts 0 orphans — either way the tail is discarded, never merged.
  check("A's tail discarded: in session_orphans or dropped as StaleEpoch on B", (Array.isArray(orphans) && orphans.length >= 1) || dropped, `orphans=${JSON.stringify(orphans)} stale-dropped-on-B=${dropped}`);
  const ha = await history(A, sm);
  check("A's transcript is B's (nothing merged)", shape(ha) === shape(hm), shape(ha));
  await A.waitEv(sr, "SessionOwnerChanged", 60_000, (e) => e.here === false);
  check("A's PTY group still alive after the takeover (B never signalled it)", pgAlive(pty?.pgid ?? null), `pgid ${pty?.pgid}`);
  const bj = jobs(B, sr);
  check("B lists A's PTY job lost with engine = A (settle_transferred, no signal)", bj.length === 1 && bj[0].state === "lost" && bj[0].engine === A.id, JSON.stringify(bj));
  if (pty?.pgid) try { process.kill(-pty.pgid, "SIGKILL"); } catch { /* gone */ }
}

async function T4(A: Engine, B: Engine) {
  // A partitioned. (a) `send_input` on B → front-door Write take; Revision 1
  // Q10: a client's write counts as the user's → resumes at once.
  // (b) a system take (`orphan`) → ResumePending, Attention item, no
  // TurnResumed; `resume_turn` on B → TurnResumed.
  const sa = await newSession(A);
  const sb = await newSession(A);
  const ta = await sendBatch(A, sa, [readFixture, heldShell(`${ROOT}/hits-T4a`, 60)], "shell", [A, B]);
  const tb = await sendBatch(A, sb, [readFixture, heldShell(`${ROOT}/hits-T4b`, 60)], "shell", [A, B]);
  await A.partitionFrom(B);
  await B.rpc("send_input", [sa, "hello from B", [], null, null]);
  const ra = await B.waitEv(sa, "TurnResumed", 40_000);
  check("(Q10) send_input on B took the session and resumed at once: TurnResumed{takeover}", ra.cause === "takeover" && ra.from === A.id, JSON.stringify({ cause: ra.cause, from: ra.from }));
  check("(Q10) no ResumePending for the client's write", B.evs(sa, "ResumePending").length === 0);
  await B.waitEv(sa, "TurnFinished", 60_000);
  const take = await B.rpc("session_take", [sb, "orphan"]);
  check("session_take(orphan) on B → B owns it", isObj(take) && take.owner === B.id);
  const pending = await B.waitEv(sb, "ResumePending", 10_000);
  check("system take → ResumePending{from: A}, no TurnResumed", pending.from === A.id && B.evs(sb, "TurnResumed").length === 0);
  const items = await B.rpc("attention_list", []);
  const item = Array.isArray(items) ? items.find((i) => isObj(i) && String(i.id).startsWith(`resume:${sb}`)) : null;
  check("B's attention_list has resume:<session>:<turn> with the takeover title", isObj(item) && String(item.id) === `resume:${sb}:${tb}` && /taken from/.test(JSON.stringify(item)), isObj(item) ? `${item.id} ${str(item.title)}` : JSON.stringify(items).slice(0, 200));
  const note = B.one<{ body: string }>("SELECT body FROM messages WHERE session_id = ? AND body LIKE '%press Resume%'", sb);
  check("local note row: taken from A … press Resume", !!note && note.body.includes(A.id), (note?.body ?? "").slice(0, 120));
  check("newest step still open on B (nothing ran)", newestStep(B, sb)?.kind !== "turn_finished" && callState(B, sb, tb, "shell") === "started");
  const ok = await B.rpc("resume_turn", [sb]);
  const rb = await B.waitEv(sb, "TurnResumed", 30_000);
  check("resume_turn on B → TurnResumed{cause: takeover, from: A}", ok === true && rb.cause === "takeover" && rb.from === A.id, JSON.stringify({ ok, cause: rb.cause, from: rb.from }));
  await B.waitEv(sb, "TurnFinished", 60_000);
  const after = await B.rpc("attention_list", []);
  check("the resume: item cleared", !(Array.isArray(after) && after.some((i) => isObj(i) && String(i.id).startsWith(`resume:${sb}`))));
  A.resume();
  await A.waitEv(sa, "SessionOwnerChanged", 60_000, (e) => e.here === false);
  await A.waitEv(sb, "SessionOwnerChanged", 60_000, (e) => e.here === false);
  check("A demoted both", ownerRow(A, sa)?.owner === B.id && ownerRow(A, sb)?.owner === B.id);
  void ta;
}

async function T5(A: Engine, B: Engine) {
  // `ask_user` pending on A; take → the same ask id on B's attention list;
  // `answer_ask` on B settles the call. Also D18: `answer_ask` on B before
  // the take is forwarded to A (T7's routing half).
  const q = { questions: [{ id: "color", question: "Which?", options: [{ label: "red" }, { label: "blue" }], recommended: 0 }] };
  const sid = await newSession(A);
  const turn = await sendBatch(A, sid, [{ name: "ask_user", args: q }], "ask_user", [A, B]);
  const row = await waitFor("question row on B", () => B.one<{ id: string; state: string }>("SELECT id, state FROM questions WHERE session_id = ? AND kind = 'ask'", sid));
  check("question row replicated pending", row.state === "pending", row.id);
  const itemsA = await A.rpc("attention_list", []);
  check("A lists ask:<id>", Array.isArray(itemsA) && itemsA.some((i) => isObj(i) && i.id === `ask:${row.id}`));
  const itemsB0 = await B.rpc("attention_list", []);
  check("B (replica) lists no ask item and no resume item for it", !(Array.isArray(itemsB0) && itemsB0.some((i) => isObj(i) && (i.id === `ask:${row.id}` || String(i.id).startsWith(`resume:${sid}`)))));
  await B.rpc("session_take", [sid, "user"]);
  await B.waitEv(sid, "TurnResumed");
  const itemB = await waitFor("ask item on B", async () => {
    const items = await B.rpc("attention_list", []);
    return Array.isArray(items) ? items.find((i) => isObj(i) && i.id === `ask:${row.id}`) ?? null : null;
  }, 20_000, 250);
  check("B's attention_list has ask:<same id> after the take", isObj(itemB), JSON.stringify(itemB).slice(0, 160));
  const answered = await B.rpc("answer_ask", [row.id, [{ kind: "choice", index: 1 }]]);
  check("answer_ask on B → true", answered === true);
  await B.waitEv(sid, "TurnFinished", 30_000);
  const h = await history(B, sid);
  check("the call settled with the answer on B", /blue/.test(toolResult(h, "ask_user")), toolResult(h, "ask_user").split("\n")[0]);
  await waitFor("A's copy shows answered", () => A.one<{ state: string }>("SELECT state FROM questions WHERE id = ?", row.id)?.state === "answered", 20_000);
  check("A's questions row is answered after replication", true);
  void turn;
}

async function T6(A: Engine, B: Engine) {
  // `shell` job + `start` PTY on A, transfer → B lists both lost with the
  // owner-changed reason; pty_screen on B serves the replicated page with
  // the moved-from banner; A's groups are dead (KillSession).
  const sid = await newSession(A);
  await sendBatch(A, sid, [{ name: "start", args: { command: "printf 'HELLO-T6\\n'; sleep 300", name: "t6", ready: { log: "HELLO" } } }], null, []);
  await A.waitEv(sid, "TurnFinished", 30_000);
  await sleep(1500); // one pty snapshot tick
  const turn = await sendBatch(A, sid, [heldShell(`${ROOT}/hits-T6`, 90)], "shell", [A, B]);
  const before = await waitFor("both job rows on A", () => (jobs(A, sid).length === 2 && jobs(A, sid).every((j) => j.pgid) ? jobs(A, sid) : null), 10_000);
  check("two running jobs on A, engine = A", before.length === 2 && before.every((j) => j.state === "running" && j.engine === A.id), JSON.stringify(before));
  await waitFor("pty screen replicated to B", () => B.one<{ text: string }>("SELECT text FROM pty_screens WHERE session_id = ?", sid)?.text.includes("HELLO-T6"), 20_000);
  await B.rpc("session_take", [sid, "user"]);
  await B.waitEv(sid, "TurnResumed");
  await B.waitEv(sid, "TurnFinished", 60_000);
  const listed = await B.rpc("list_jobs", [sid]);
  const lost = Array.isArray(listed) ? listed.filter((j) => isObj(j) && j.state === "lost") : [];
  check("B's list_jobs shows both jobs lost", lost.length === 2, JSON.stringify(listed).slice(0, 300));
  const pty = before.find((j) => j.kind === "pty");
  const screen = await B.rpc("pty_screen", [sid, pty?.id ?? "j1", { anchor: "live" }, null]);
  const runText = (r: Json) => (isObj(r) && Array.isArray(r.cells) ? r.cells.map((c) => (isObj(c) ? str(c.text) : "")).join("") : "");
  const text = isObj(screen) && Array.isArray(screen.lines) ? screen.lines.map((l) => (Array.isArray(l) ? l.map(runText).join("") : "")).join("\n") : "";
  check("pty_screen on B: the replicated page (stored_page, epoch null = lost)", isObj(screen) && screen.type === "updated" && /HELLO-T6/.test(text) && screen.epoch === null, `${isObj(screen) ? str(screen.type) : JSON.stringify(screen)} | ${text.split("\n").filter(Boolean).slice(0, 2).join(" | ")}`);
  // The model-facing page (`screen` tool) carries the moved-from banner (lost_page).
  await sendBatch(B, sid, [{ name: "screen", args: { id: pty?.id ?? "j1" } }], null, []);
  await B.waitEv(sid, "TurnFinished", 30_000, () => B.evs(sid, "TurnFinished").length >= 2);
  const page = toolResult(await history(B, sid), "screen");
  // Through the python bridge the tool's structured value is what the model sees: `lost: True` + A's last rows.
  check("screen() on B: the PTY is lost, A's last page served", /'lost': True/.test(page) && /HELLO-T6/.test(page), page.split("\n").find((l) => /'lost'/.test(l))?.slice(0, 160) ?? page.slice(0, 160));
  const h = await history(B, sid);
  check("resumed shell call carries the owner-changed job note", /moved|owner changed/.test(toolResult(h, "shell")), toolResult(h, "shell").split("\n")[0]);
  check("A's process groups dead after the transfer (KillSession)", before.every((j) => !pgAlive(j.pgid)), before.map((j) => `${j.id}:${j.pgid}`).join(","));
  const bj = jobs(B, sid);
  check("rows lost/lost with engine = A on B", bj.every((j) => j.state === "lost" && j.exit === "lost" && j.engine === A.id), JSON.stringify(bj));
  void turn;
}

async function T7(A: Engine, B: Engine) {
  // Replica display: load_session on B names the owner; resume_turn on B
  // (user session) takes + resumes here (Q15 as built); answer_ask on B for
  // A's pending question with A down → -32010.
  const sid = await newSession(A);
  const turn = await sendBatch(A, sid, [readFixture, heldShell(`${ROOT}/hits-T7`, 60)], "shell", [A, B]);
  const loaded = await B.rpc("load_session", [sid, null, null, null]);
  const resume = isObj(loaded) && isObj(loaded.resume) ? loaded.resume : null;
  check("B's load_session: resume.owner = A, here = false, kind interrupted, resumed false", resume?.owner === A.id && resume.here === false && resume.kind === "interrupted" && resume.resumed === false && resume.turn === turn, JSON.stringify(resume));
  check("B's Loaded.running is false (process-local flag)", isObj(loaded) && loaded.running === false);
  const rows = await B.rpc("list_sessions", []);
  const row = Array.isArray(rows) ? rows.find((r) => isObj(r) && r.id === sid) : null;
  check("B's list_sessions row: owner A, here false", isObj(row) && row.owner === A.id && row.here === false, JSON.stringify(row).slice(0, 200));
  const items = await B.rpc("attention_list", []);
  check("B raises no resume: item for A's turn", !(Array.isArray(items) && items.some((i) => isObj(i) && String(i.id).startsWith(`resume:${sid}`))));
  const la = await A.rpc("load_session", [sid, null, null, null]);
  const ra = isObj(la) && isObj(la.resume) ? la.resume : null;
  check("A's load_session: here true, resumed true (its live turn)", ra?.here === true && ra.resumed === true && ra.owner === A.id, JSON.stringify(ra));
  // resume_turn on B while A is up: Route::Take (user session) → Q15 → resumed here as a transfer.
  const ok = await B.rpc("resume_turn", [sid]);
  const rb = await B.waitEv(sid, "TurnResumed", 30_000);
  check("resume_turn on B (A up) takes the session: TurnResumed on B, from A", rb.from === A.id && (rb.cause === "transfer" || rb.cause === "takeover"), `ok=${ok} cause=${rb.cause}`);
  await B.waitEv(sid, "TurnFinished", 60_000);
  // D18 with A down: an answer for A's question from B's front door → -32010.
  const s2 = await newSession(A);
  await sendBatch(A, s2, [{ name: "ask_user", args: { questions: [{ id: "x", question: "?", options: [{ label: "a" }, { label: "b" }] }] } }], "ask_user", [A, B]);
  const q = await waitFor("question on B", () => B.one<{ id: string }>("SELECT id FROM questions WHERE session_id = ?", s2));
  await A.partitionFrom(B);
  const r = await B.raw("answer_ask", [q.id, [{ kind: "choice", index: 0 }]]);
  check("answer_ask on B for A's question with A down → -32010 OwnerUnreachable", r.error?.code === -32010, JSON.stringify(r.error));
  A.resume();
  const r2 = await waitFor("answer forwarded to A", async () => {
    const x = await B.raw("answer_ask", [q.id, [{ kind: "choice", index: 0 }]]);
    return x.result === true ? x : x.error && x.error.code !== -32010 ? x : null;
  }, 40_000, 500);
  check("answer_ask on B with A back is forwarded and settles A's call", r2.result === true, JSON.stringify(r2));
  await A.waitEv(s2, "TurnFinished", 30_000);
  check("A finished the turn with the forwarded answer", ownerRow(A, s2)?.owner === A.id && /\ba\b/.test(toolResult(await history(A, s2), "ask_user")));
}

async function T8(A: Engine, B: Engine) {
  // D16: an input to the session on A during its release drain → -32012.
  const sid = await newSession(A);
  await sendBatch(A, sid, [readFixture, heldShell(`${ROOT}/hits-T8`, 90)], "shell", [A, B]);
  const take = B.rpc("session_take", [sid, "user"]);
  await A.waitEv(sid, "TurnSuspended", 10_000);
  const r = await A.raw("send_input", [sid, "during the drain", [], null, null]);
  const data = isObj(r.error?.data) && isObj(r.error.data.SessionDraining) ? r.error.data.SessionDraining : null;
  check("send_input on A mid-drain → -32012 SessionDraining{reason: release}", r.error?.code === -32012 && data?.reason === "release", `${JSON.stringify(r.error)}`);
  const got = await take;
  check("the take completed", isObj(got) && got.owner === B.id);
  await B.waitEv(sid, "TurnFinished", 60_000);
  const h = await history(B, sid);
  check("the refused input is in no transcript", !h.some((x) => /during the drain/.test(x.text ?? "")) && !(await history(A, sid)).some((x) => /during the drain/.test(x.text ?? "")));
}

/// A PTY job that stays up `secs` seconds (never signalled by a takeover, T3).
const ptyJob = (tag: string, secs: number): Call => ({ name: "start", args: { command: `printf 'HELLO-${tag}\\n'; sleep ${secs}; echo DONE-${tag}`, name: tag, ready: { log: "HELLO" } } });
async function userTexts(e: Engine, sid: string, needle: string): Promise<string[]> {
  return (await history(e, sid)).filter((x) => x.role === "user" && (x.text ?? "").includes(needle)).map((x) => x.text ?? "");
}

async function T10(A: Engine, B: Engine) {
  // 139 R2 D4: reminders follow the owner on a connected transfer — armed on
  // B from the replicated row, A's timer stopped by the release drain. The
  // drain also kills A's job (§7.1), so no later result exists here; the
  // harness envelope for a job that outlives the move is T10b's.
  const sid = await newSession(A);
  await sendBatch(A, sid, [ptyJob("T10", 300), { name: "remind_me_in", args: { seconds: 30, message: "follow the owner" } }], null, []);
  await A.waitEv(sid, "TurnFinished", 30_000);
  const t0 = now();
  const rem = A.one<{ id: string }>("SELECT id FROM reminders WHERE session = ?", sid);
  check("reminder row r1 on A", rem?.id === "r1", JSON.stringify(rem));
  await waitFor("reminder row replicated to B", () => B.one("SELECT id FROM reminders WHERE session = ?", sid));
  const take = await B.rpc("session_take", [sid, "user"]);
  check("session_take(user) on B → B, epoch 2", isObj(take) && take.owner === B.id && take.epoch === 2, JSON.stringify(take));
  await A.waitEv(sid, "SessionOwnerChanged", 20_000, (e) => e.here === false);
  const fired = await B.waitEv(sid, "ReminderFired", 60_000);
  const at = (now() - t0) / 1000;
  check("the reminder fired on B, r1, about 30 s after it was set on A", fired.id === "r1" && at > 20 && at < 55, `+${at.toFixed(1)}s`);
  const onB = await waitFor("<reminder> in B's transcript", async () => {
    const t = await userTexts(B, sid, '<reminder id="r1"');
    return t.length ? t : null;
  }, 20_000, 500);
  check("exactly one <reminder> on B, not late", onB.length === 1 && /late="false"/.test(onB[0]), onB[0]?.split("\n")[0]);
  await B.waitEv(sid, "TurnFinished", 60_000, () => B.evs(sid, "TurnFinished").length >= 1);
  check("A's timer never fired (no ReminderFired on A; the drain stopped it)", A.evs(sid, "ReminderFired").length === 0 && A.logText().includes("reminder timer(s) stopped"), `${A.evs(sid, "ReminderFired").length} fired on A`);
  check("no reminder row left on either engine", !A.one("SELECT id FROM reminders WHERE session = ?", sid) && !B.one("SELECT id FROM reminders WHERE session = ?", sid));
  const pty = jobs(A, sid).find((j) => j.kind === "pty");
  check("A's PTY group dead after the transfer (release drain KillSession, §7.1)", !!pty && !pgAlive(pty.pgid), JSON.stringify(pty));
  check("nothing queued in A's harness outbox", A.sql("SELECT id FROM harness_outbox").length === 0);
}

async function T10b(A: Engine, B: Engine) {
  // 139 R2 D2/D3/D5: A partitioned, B takes over, the link returns → A
  // demotes while its PTY job still runs (a takeover never signals it, T3).
  // The job then finishes on A: its real <job_result> reaches B over
  // Msg::Harness exactly once, naming A; the reminder set on A fires on B;
  // A's timer stopped at the demote.
  const sid = await newSession(A);
  await sendBatch(A, sid, [ptyJob("T10b", 75), { name: "remind_me_in", args: { seconds: 90, message: "after the takeover" } }], null, []);
  await A.waitEv(sid, "TurnFinished", 30_000);
  const t0 = now();
  const pty = jobs(A, sid).find((j) => j.kind === "pty");
  check("a PTY job runs on A", !!pty && pgAlive(pty.pgid), JSON.stringify(pty));
  await waitFor("rows replicated to B", () => B.one("SELECT id FROM reminders WHERE session = ?", sid) && jobs(B, sid).length === 1);
  await A.partitionFrom(B);
  const take = await B.rpc("session_take", [sid, "user"]);
  check("session_take(user) on B with A partitioned → B, epoch 2", isObj(take) && take.owner === B.id && take.epoch === 2, JSON.stringify(take));
  A.resume();
  await A.waitEv(sid, "SessionOwnerChanged", 60_000, (e) => e.here === false);
  const demotedAt = (now() - t0) / 1000;
  check("A demoted while its job still ran", !!pty && pgAlive(pty.pgid), `demoted at +${demotedAt.toFixed(1)}s`);
  const results = await waitFor("A's <job_result> on B", async () => {
    const t = (await userTexts(B, sid, "<job_result")).filter((x) => x.includes("DONE-T10b"));
    return t.length ? t : null;
  }, 120_000, 500);
  const arrivedAt = (now() - t0) / 1000;
  const r = results[0] ?? "";
  check("the job's real result arrived on B over the envelope: one <job_result exit=0> naming A, the note inside the element", results.length === 1 && /<job_result[^>]*exit="0"/.test(r) && r.includes(`produced on ${A.id} after the session moved to ${B.id}`) && r.trimEnd().endsWith("</job_result>"), `+${arrivedAt.toFixed(1)}s ${r.split("\n")[0].slice(0, 120)}`);
  check("A queued it for the owner; B handed it to its supervisor", A.logText().includes("job j1: queued for the owner") && B.logText().includes("job j1 from") && B.logText().includes("handed to the supervisor"));
  // Every harness-made input A produced after the move travelled the same way: the
  // PTY's `<job_idle>` nudge (pty_idle_secs 60) and the `<job_result>` — one seen id each.
  const envelopes = (await userTexts(B, sid, "(produced on ")).length;
  check("A's outbox is empty (acked); B remembers exactly one id per envelope", A.sql("SELECT id FROM harness_outbox").length === 0 && envelopes >= 1 && B.sql("SELECT id FROM harness_seen").length === envelopes, `outbox=${A.sql("SELECT id FROM harness_outbox").length} seen=${B.sql("SELECT id FROM harness_seen").length} envelopes=${envelopes}`);
  check("B's own copy of the job row stays as the take left it (never un-lost here)", jobs(B, sid).length === 1 && jobs(B, sid)[0].engine === A.id, JSON.stringify(jobs(B, sid)));
  const fired = await B.waitEv(sid, "ReminderFired", 120_000);
  const firedAt = (now() - t0) / 1000;
  check("the reminder set on A fired on B about 90 s after it was set", fired.id === "r1" && firedAt > 80 && firedAt < 120, `+${firedAt.toFixed(1)}s`);
  const onB = await waitFor("<reminder> in B's transcript", async () => {
    const t = await userTexts(B, sid, '<reminder id="r1"');
    return t.length ? t : null;
  }, 20_000, 500);
  await B.waitEv(sid, "TurnFinished", 60_000, () => B.evs(sid, "TurnFinished").length >= 1 + envelopes);
  check("exactly one <reminder> and one <job_result> on B; B ran a turn on each input (the first turn ran on A)", onB.length === 1 && (await userTexts(B, sid, "<job_result")).length === 1 && B.evs(sid, "TurnFinished").length >= 1 + envelopes, `${B.evs(sid, "TurnFinished").length} TurnFinished on B`);
  check("A's timer never fired", A.evs(sid, "ReminderFired").length === 0 && A.logText().includes("reminder timer(s) stopped"), `${A.evs(sid, "ReminderFired").length} fired on A`);
  const ha = await history(A, sid);
  check("A's transcript is B's (the result and the reminder came back replicated, nothing injected on A)", shape(ha) === shape(await history(B, sid)), shape(ha));
}

const ROWS: Record<string, (A: Engine, B: Engine) => Promise<void>> = { T1, T2, T2c, T3, T4, T5, T6, T7, T8, T10, T10b };

// ---- main -------------------------------------------------------------------

async function main() {
  const wanted = Bun.argv.slice(2).length ? Bun.argv.slice(2) : Object.keys(ROWS);
  rmSync(ROOT, { recursive: true, force: true });
  mkdirSync(ROOT, { recursive: true });
  writeFileSync(FIXTURE, "fixture-139: the quick brown fox\n");
  const fake = Bun.spawn(["bun", `${HERE}/fake-openai.ts`, String(FAKE_PORT), `${ROOT}/fake-requests.ndjson`], { stdout: "inherit", stderr: "inherit" });
  await waitFor("fake provider", () => fetch(`${FAKE_URL}models`).then((r) => r.ok, () => false), 10_000, 100);
  const A = new Engine("A", 7931, 7941);
  const B = new Engine("B", 7932, 7942);
  const procs = [A, B];
  const cleanup = async () => {
    for (const e of procs) await e.stop("SIGKILL").catch(() => {});
    fake.kill();
  };
  process.on("SIGINT", () => void cleanup().then(() => process.exit(130)));
  try {
    for (const e of procs) e.writeConfig();
    await Promise.all(procs.map((e) => e.start()));
    for (const e of procs) {
      const p = await e.rpc("add_provider", ["fake", FAKE_URL, "smoke-key"]);
      log(`${e.name} add_provider → ${JSON.stringify(p)}`);
    }
    const pairing = String(await A.rpc("mesh_pairing_string", []));
    await B.rpc("mesh_add", [pairing]);
    const t0 = now();
    for (const [x, y] of [[A, B], [B, A]] as const) {
      await waitFor(`${x.name} sees ${y.name} reachable`, async () => {
        const s = await x.rpc("mesh_status", []);
        return isObj(s) && Array.isArray(s.peers) && s.peers.some((p) => isObj(p) && p.id === y.id && p.reachable === true);
      }, 60_000, 250);
    }
    log(`paired in ${now() - t0} ms`);
    // The sessions plugin needs the transport: a probe session must replicate before the rows start.
    const probe = await newSession(A);
    await waitFor("probe session on B", () => ownerRow(B, probe), 60_000, 250);
    log(`replication live (probe ${probe.slice(0, 8)} on B)`);
    for (const name of wanted) {
      const row = ROWS[name];
      if (!row) {
        log(`unknown row ${name}`);
        continue;
      }
      currentRow = name;
      const t = now();
      try {
        await row(A, B);
      } catch (e) {
        check("row completed", false, String(e));
      }
      // A row that failed mid-partition must not leave an engine stopped.
      A.resume();
      B.resume();
      log(`${name} took ${((now() - t) / 1000).toFixed(1)} s`);
    }
  } finally {
    await cleanup();
  }
  console.log("\n| row | check | result | detail |\n|---|---|---|---|");
  for (const r of results) console.log(`| ${r.row} | ${r.name} | ${r.ok ? "✓" : "✗"} | ${r.detail.replace(/\|/g, "\\|").replace(/\n/g, " ")} |`);
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed; engines' ids A=${A.id} B=${B.id}; logs in ${ROOT}/{A,B}/logs/`);
  process.exit(failed ? 1 : 0);
}

await main();
