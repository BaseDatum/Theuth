/**
 * Probe host for the progressive-UI and 50-session performance acceptance (design 46). It owns
 * the harness side of the run (sessions, model, timing record) and exposes a tiny JSON API so
 * the parent can interleave real-browser steps (open the session, sample DOM / rAF / LongTask)
 * with the model stream. It never renders anything itself and never calls a paid model: the
 * model is perf-fixture.ts, multiplexed by the `[[probe:<key>]]` marker in each prompt.
 *
 * Launch (both long-running; the parent starts them, e.g. through `hub start`):
 *
 *   bun scripts/apps/perf-fixture.ts --port 18790
 *   bun scripts/apps/ui-probe.ts --url ws://127.0.0.1:18776 --data-dir /tmp/wth-apps-e2e.XXXX \
 *       --fixture-url http://127.0.0.1:18790 [--port 18779] [--provider perf]
 *
 * On boot it connects, subscribes to every event (timestamped on arrival), registers the provider
 * (`add_provider(<provider>, <fixture>/v1, "perf-key")`) and serves. Workflow, one call per step:
 *
 *   POST /prepare {statements?:100, bg?:50, app_seconds?:6, bg_seconds?:45, app_chunk?:64, bg_chunk?:24}
 *        Creates 1 app session + `bg` prose sessions on `<provider>/app-fixture`, builds a valid
 *        `statements`-statement OpenUI program (root Stack + TextContent lines "Probe statement NNN: …"),
 *        scripts the fixture (app key: one `app` tool call whose arguments stream over ~app_seconds;
 *        bg key: prose streaming over ~bg_seconds; both gated). Returns
 *        {app_session, bg_sessions[], model, text_prefix, statements, args_bytes, app_delay_ms, ...}.
 *        → parent opens the app session in the real UI now.
 *   POST /start {wait_ms?:30000}
 *        Sends the bg prompts then the app prompt. Waits until the fixture holds every stream at
 *        its gate (the agent has issued the model requests; UI shows the turns as started) or
 *        wait_ms elapses. Returns {held:{app,bg}, t_start, t_held}. Nothing streams yet.
 *        → parent arms its browser measurements (rAF / PerformanceObserver longtask / DOM poll).
 *   POST /release
 *        Opens both gates; every stream begins within the same tick. Returns {t_release}.
 *   GET  /status
 *        {app:{pending_at, first_progress_at, progress_events, shown_at, tool_finished_at, is_error,
 *               turn_finished_at, stop_reason, call_id, artifact, library_hash, library_hash_ok, diagnostics},
 *         bg:{sessions, streaming, finished, errored, deltas}, t:{start, held, release}} — all times ms
 *        epoch (Date.now(), same clock as the browser's `Date.now()`), null until observed.
 *   GET  /events?session=<id>[&from=N]   timestamped raw events of one session (post-mortem).
 *   POST /cleanup
 *        Cancels open turns, deletes every created session, drops the fixture scripts. Repeatable:
 *        /prepare again for another run.
 *   POST /shutdown
 *        /cleanup, then removes the provider, restores the harness default model and exits.
 *        SIGINT does the same.
 *   GET  /health
 *
 * Every response is JSON; errors are {error} with status 4xx/5xx. Nothing outside the harness data
 * dir and the fixture is touched.
 */
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect, parseArgs, readToken, type Rpc, type WireEvent } from "./rpc.ts";

interface Info {
  default_model: string;
  app_enabled?: boolean;
}
interface Artifact {
  id: string;
  session: string;
  call_id: string;
  name: string;
  kind: string;
  version: number;
}
interface AppDoc {
  artifact: Artifact;
  library_hash: string;
  diagnostics: unknown[];
}
interface Tagged {
  at: number;
  ev: WireEvent;
}
interface Run {
  from: number;
  app: string;
  bg: string[];
  key: { app: string; bg: string };
  textPrefix: string;
  statements: number;
  t: { start: number | null; held: number | null; release: number | null };
  doc: AppDoc | null;
}

const HERE = dirname(fileURLToPath(import.meta.url));
const { flags } = parseArgs(process.argv.slice(2));
const str = (k: string, d: string) => (typeof flags[k] === "string" ? (flags[k] as string) : d);
const url = str("url", "ws://127.0.0.1:18776");
const dataDir = str("data-dir", "");
const token = typeof flags.token === "string" ? flags.token : dataDir ? readToken(dataDir) : "";
if (!token) throw new Error("--data-dir (for <data_dir>/token) or --token required");
const fixtureUrl = str("fixture-url", "http://127.0.0.1:18790").replace(/\/+$/, "").replace(/\/v1$/, "");
const port = Number(str("port", "18779"));
const provider = str("provider", "perf");
const MODEL = `${provider}/app-fixture`;
const APP_NAME = "perf-panel.openui";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

async function fixture(method: string, path: string, body?: unknown) {
  const r = await fetch(`${fixtureUrl}${path}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`fixture ${method} ${path}: ${r.status}`);
  return r.json();
}

// ---------------------------------------------------------------------------------------------
// boot: rpc, event log, provider
// ---------------------------------------------------------------------------------------------

if (!(await fetch(`${fixtureUrl}/fixture/health`).then((r) => r.ok, () => false)))
  throw new Error(`perf-fixture not reachable at ${fixtureUrl} (bun scripts/apps/perf-fixture.ts --port ...)`);
const rpc: Rpc = await connect(url, token);
const info = await rpc.call<Info>("info");
if (info.app_enabled === false) throw new Error("harness reports app_enabled: false");
const events: Tagged[] = [];
const unsubscribe = await rpc.subscribe(null, (ev) => {
  events.push({ at: Date.now(), ev });
});
await rpc.call("remove_provider", [provider]).catch(() => {});
await rpc.call("add_provider", [provider, `${fixtureUrl}/v1`, "perf-key"]);

let run: Run | null = null;

// ---------------------------------------------------------------------------------------------
// program + prose generators (deterministic)
// ---------------------------------------------------------------------------------------------

const WORDS =
  "cluster rollout replica canary health probe latency budget shard region failover quota drift cache queue worker ledger tenant policy schedule".split(
    " ",
  );

/** `statements` valid OpenUI statements: root Stack + numbered TextContent lines, ~80 bytes each. */
function program(statements: number, textPrefix: string) {
  const n = Math.max(2, statements) - 1;
  const ids = Array.from({ length: n }, (_, i) => `s${String(i + 1).padStart(3, "0")}`);
  const lines = [`root = Stack([${ids.join(", ")}])`];
  for (let i = 0; i < n; i++) {
    const filler = Array.from({ length: 7 }, (_, k) => WORDS[(i * 7 + k * 3) % WORDS.length]).join(" ");
    lines.push(`${ids[i]} = TextContent("${textPrefix} ${String(i + 1).padStart(3, "0")}: ${filler}")`);
  }
  return lines.join("\n");
}

function prose(bytes: number) {
  let out = "";
  for (let i = 0; out.length < bytes; i++) {
    const w = Array.from({ length: 9 }, (_, k) => WORDS[(i * 5 + k * 2) % WORDS.length]);
    w[0] = w[0]![0]!.toUpperCase() + w[0]!.slice(1);
    out += `${w.join(" ")} ${i + 1}. `;
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// event queries
// ---------------------------------------------------------------------------------------------

const first = (session: string, pred: (e: WireEvent) => boolean, from = run?.from ?? 0) =>
  events.slice(from).find((t) => t.ev.session === session && pred(t.ev));

function turnOpen(session: string, from: number) {
  const started = events.slice(from).some((t) => t.ev.session === session && t.ev.type === "TurnStarted");
  const finished = events.slice(from).some((t) => t.ev.session === session && t.ev.type === "TurnFinished");
  return started && !finished;
}

async function status() {
  if (!run) return { prepared: false };
  const r = run;
  const pending = first(r.app, (e) => e.type === "ToolCallPending" && e.name === "app");
  const callId = pending ? String(pending.ev.call_id) : null;
  const progress = events.slice(r.from).filter((t) => t.ev.type === "ToolCallProgress" && t.ev.call_id === callId);
  const shown = first(r.app, (e) => e.type === "ArtifactShown" && (e.artifact as Artifact | undefined)?.call_id === callId);
  const toolDone = first(r.app, (e) => e.type === "ToolCallFinished" && e.call_id === callId);
  const turnDone = first(r.app, (e) => e.type === "TurnFinished");
  if (shown && callId && !r.doc) {
    r.doc = await rpc.call<AppDoc>("get_app", [callId]).catch(() => null);
  }
  const hashFile = join(HERE, "../../packages/apps/generated/library-hash.txt");
  const expectedHash = existsSync(hashFile) ? readFileSync(hashFile, "utf8").trim() : null;
  const bgSet = new Set(r.bg);
  const streaming = new Set<string>();
  const finished = new Set<string>();
  const errored = new Set<string>();
  let deltas = 0;
  for (const { ev } of events.slice(r.from)) {
    const s = String(ev.session);
    if (!bgSet.has(s)) continue;
    if (ev.type === "TextDelta") {
      streaming.add(s);
      deltas++;
    } else if (ev.type === "TurnFinished") {
      finished.add(s);
      if (ev.stop_reason === "error") errored.add(s);
    }
  }
  return {
    prepared: true,
    t: r.t,
    app: {
      session: r.app,
      call_id: callId,
      pending_at: pending?.at ?? null,
      first_progress_at: progress[0]?.at ?? null,
      last_progress_at: progress.at(-1)?.at ?? null,
      progress_events: progress.length,
      progress_bytes: progress.length ? String(progress.at(-1)!.ev.args).length : 0,
      shown_at: shown?.at ?? null,
      artifact: (shown?.ev.artifact as Artifact | undefined) ?? null,
      tool_finished_at: toolDone?.at ?? null,
      is_error: toolDone?.ev.is_error ?? null,
      tool_output: toolDone ? String(toolDone.ev.output).slice(0, 400) : null,
      turn_finished_at: turnDone?.at ?? null,
      stop_reason: turnDone?.ev.stop_reason ?? null,
      library_hash: r.doc?.library_hash ?? null,
      library_hash_expected: expectedHash,
      library_hash_ok: r.doc && expectedHash ? r.doc.library_hash === expectedHash : null,
      diagnostics: r.doc?.diagnostics ?? null,
    },
    bg: { sessions: r.bg.length, streaming: streaming.size, finished: finished.size, errored: errored.size, deltas },
    events_total: events.length,
  };
}

// ---------------------------------------------------------------------------------------------
// steps
// ---------------------------------------------------------------------------------------------

async function prepare(body: Record<string, unknown>) {
  if (run) throw new Error("already prepared; POST /cleanup first");
  const num = (k: string, d: number) => (typeof body[k] === "number" ? (body[k] as number) : d);
  const statements = num("statements", 100);
  const bgCount = num("bg", 50);
  const appSeconds = num("app_seconds", 6);
  const bgSeconds = num("bg_seconds", 45);
  const appChunk = num("app_chunk", 64);
  const bgChunk = num("bg_chunk", 24);
  const stamp = Date.now().toString(36);
  const textPrefix = `Probe statement`;
  const source = program(statements, textPrefix);
  const args = { name: APP_NAME, title: `Perf panel ${stamp}`, source };
  const argsBytes = JSON.stringify(args).length;
  const appDelay = Math.max(1, Math.round((appSeconds * 1000) / Math.ceil(argsBytes / appChunk)));
  const bgDelay = 100;
  const bgText = prose(Math.ceil((bgSeconds * 1000) / bgDelay) * bgChunk);
  const key = { app: `app-${stamp}`, bg: `bg-${stamp}` };

  await fixture("POST", `/fixture/script/${key.app}`, {
    steps: [{ tool_calls: [{ name: "app", args }] }],
    default_text: "Done.",
    chunk: appChunk,
    delay_ms: appDelay,
    gate: true,
  });
  await fixture("POST", `/fixture/script/bg-${stamp}`, {
    steps: [],
    default_text: bgText,
    chunk: bgChunk,
    delay_ms: bgDelay,
    gate: true,
  });

  const sessions: string[] = [];
  for (let i = 0; i <= bgCount; i++) {
    const s = await rpc.call<string>("new_session");
    await rpc.call("set_model", [s, MODEL]);
    sessions.push(s);
  }
  const [app, ...bg] = sessions as [string, ...string[]];
  run = { from: events.length, app, bg, key, textPrefix, statements, t: { start: null, held: null, release: null }, doc: null };
  return {
    app_session: app,
    bg_sessions: bg,
    model: MODEL,
    marker: { app: `[[probe:${key.app}]]`, bg: `[[probe:${key.bg}]]` },
    text_prefix: textPrefix,
    statements,
    source_bytes: source.length,
    args_bytes: argsBytes,
    app_chunk: appChunk,
    app_delay_ms: appDelay,
    app_seconds_expected: (Math.ceil(argsBytes / appChunk) * appDelay) / 1000,
    bg_bytes: bgText.length,
    bg_chunk: bgChunk,
    bg_delay_ms: bgDelay,
    bg_seconds_expected: (Math.ceil(bgText.length / bgChunk) * bgDelay) / 1000,
    fixture: fixtureUrl,
  };
}

async function start(body: Record<string, unknown>) {
  if (!run) throw new Error("POST /prepare first");
  if (run.t.start) throw new Error("already started");
  const r = run;
  const waitMs = typeof body.wait_ms === "number" ? body.wait_ms : 30_000;
  r.from = events.length;
  r.t.start = Date.now();
  await Promise.all(r.bg.map((s) => rpc.call("send_input", [s, `[[probe:${r.key.bg}]] Write a long status narrative.`, []])));
  await rpc.call("send_input", [r.app, `[[probe:${r.key.app}]] Publish the perf panel.`, []]);
  const deadline = Date.now() + waitMs;
  let a = 0;
  let b = 0;
  for (;;) {
    const held = (await fixture("GET", "/fixture/held")) as Record<string, { waiting: number }>;
    a = held[r.key.app]?.waiting ?? 0;
    b = held[r.key.bg]?.waiting ?? 0;
    if (a >= 1 && b >= r.bg.length) break;
    if (Date.now() > deadline) {
      return { ok: false, error: `gate not full after ${waitMs}ms`, held: { app: a, bg: b, bg_expected: r.bg.length }, t_start: r.t.start };
    }
    await sleep(50);
  }
  r.t.held = Date.now();
  return { ok: true, held: { app: a, bg: b }, t_start: r.t.start, t_held: r.t.held };
}

async function release() {
  if (!run?.t.start) throw new Error("POST /start first");
  const r = run;
  if (r.t.release) return { ok: true, t_release: r.t.release, already: true };
  const [a, b] = await Promise.all([
    fixture("POST", `/fixture/release/${r.key.app}`),
    fixture("POST", `/fixture/release/${r.key.bg}`),
  ]);
  r.t.release = Date.now();
  return { ok: true, t_release: r.t.release, released: { app: a.released, bg: b.released } };
}

async function cleanup() {
  const out: Record<string, unknown> = { sessions: 0, cancelled: 0, deleted: 0, failed: [] as string[] };
  if (run) {
    const r = run;
    const all = [r.app, ...r.bg];
    out.sessions = all.length;
    // Release any gate still closed so held turns can end, then cancel what is still open.
    await fixture("POST", `/fixture/release/${r.key.app}`).catch(() => {});
    await fixture("POST", `/fixture/release/${r.key.bg}`).catch(() => {});
    const open = all.filter((s) => turnOpen(s, r.from));
    await Promise.all(open.map((s) => rpc.call("cancel", [s]).catch(() => {})));
    out.cancelled = open.length;
    const until = Date.now() + 10_000;
    while (Date.now() < until && all.some((s) => turnOpen(s, r.from))) await sleep(100);
    for (const s of all) {
      try {
        await rpc.call("delete_session", [s]);
        (out.deleted as number)++;
      } catch (e) {
        (out.failed as string[]).push(`${s}: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    run = null;
    await fixture("DELETE", "/fixture/script").catch(() => {});
  }
  return out;
}

async function shutdown() {
  const out = await cleanup().catch((e: Error) => ({ cleanup_error: e.message }));
  out.provider_removed = await rpc.call("remove_provider", [provider]).then(() => true, () => false);
  out.default_model_restored = await rpc.call("set_model", [null, info.default_model]).then(() => info.default_model, () => null);
  await unsubscribe().catch(() => {});
  rpc.close();
  return out;
}

// ---------------------------------------------------------------------------------------------
// server
// ---------------------------------------------------------------------------------------------

Bun.serve({
  port,
  hostname: "127.0.0.1",
  idleTimeout: 120,
  async fetch(req) {
    const u = new URL(req.url);
    const path = u.pathname.replace(/\/+$/, "");
    try {
      if (path === "/health") return json({ ok: true, url, fixture: fixtureUrl, model: MODEL, prepared: !!run, events: events.length });
      if (path === "/status") return json(await status());
      if (path === "/events") {
        const s = u.searchParams.get("session");
        const from = Number(u.searchParams.get("from") ?? run?.from ?? 0);
        const list = events.slice(from).filter((t) => !s || t.ev.session === s);
        return json(list);
      }
      if (req.method !== "POST") return json({ error: `no route ${req.method} ${path}` }, 404);
      const parsed = JSON.parse((await req.text()) || "{}") as Record<string, unknown>;
      if (path === "/prepare") return json(await prepare(parsed));
      if (path === "/start") return json(await start(parsed));
      if (path === "/release") return json(await release());
      if (path === "/cleanup") return json(await cleanup());
      if (path === "/shutdown") {
        const out = await shutdown();
        setTimeout(() => process.exit(0), 50);
        return json(out);
      }
      return json({ error: `no route ${req.method} ${path}` }, 404);
    } catch (e) {
      return json({ error: e instanceof Error ? e.message : String(e) }, 500);
    }
  },
});
console.log(JSON.stringify({ probe: `http://127.0.0.1:${port}`, url, fixture: fixtureUrl, model: MODEL, default_model: info.default_model }));

process.on("SIGINT", () => {
  void shutdown()
    .catch(() => {})
    .finally(() => process.exit(130));
});
