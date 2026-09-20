#!/usr/bin/env bun
/**
 * Design 46 §10.7 model-steering evaluation against a running harness.
 *
 * Runs `steering-cases.json` through the real agent loop (RPC `send_input`), records every
 * `app`/`show` tool call, harness error, `ArtifactShown` and the reply text, re-validates each
 * `app` source locally with `@theuth/apps/check` against the live tool catalog, and scores
 * first-pass validity (≥12/15 frontier, ≥9/15 small) and vehicle choice (≥16/20). Positional /
 * visual intent is never judged: every app output is written to disk and flagged for user review.
 *
 * Host safety: each evaluation session hides every registered tool except `--visible`
 * (default `app,show,load_skill,argo_app_health,argo_sync`) via `set_mcp_hidden`, so the model
 * cannot reach shell/file/MCP-install tools; any approval that still surfaces is denied unless it
 * is for a visible tool. `load_skill` stays visible because the app prompt points the model at the
 * generated `apps` skill for signatures (46 §9); a `load_skill` call before the first `app` call
 * does not affect first-pass scoring, which counts app attempts only.
 *
 *   bun scripts/apps/steering.ts --data-dir /tmp/theuth-apps-e2e.XXXX [--url ws://127.0.0.1:7777]
 *       [--models "amazon-bedrock/us.openai.gpt-6-astra:frontier,other/model:small"] [--effort high]
 *       [--suite all|steering|vehicle] [--limit N] [--concurrency 2] [--timeout 240000]
 *       [--fixture-state /abs/argo-state.json] [--visible a,b] [--out /tmp/dir] [--dry-run] [--self-test]
 *
 * Exit 0 only when every model passes every threshold on a full (unlimited) suite.
 */
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { check, mergeCheck } from "../../packages/apps/src/check.ts";
import type {
  CheckResult,
  ToolSnapshot,
} from "../../packages/apps/src/protocol.ts";
import { connect, parseArgs, readToken, type Rpc } from "./rpc.ts";

type Expect = {
  no_bindings?: boolean;
  no_mutations?: boolean;
  queries?: string[];
  mutations?: string[];
  min_queries?: number;
  min_mutations?: number;
  state_min?: number;
};
type SteeringCase = {
  id: string;
  group: string;
  prompt: string;
  expect: Expect;
};
type Vehicle = "app" | "fence" | "show" | "none";
type VehicleCase = { id: string; prompt: string; expect: Vehicle };
type Cases = {
  catalog_required: string[];
  thresholds: {
    steering: Record<string, number>;
    vehicle: Record<string, number>;
  };
  steering: SteeringCase[];
  vehicle: VehicleCase[];
};

/** Wire events this driver reads (crates/agent AgentEvent, tools approvals, artifact ArtifactShown). */
export type Ev =
  | {
      type: "ToolCallStarted";
      call_id: string;
      name: string;
      args: Record<string, unknown>;
    }
  | {
      type: "ToolCallFinished";
      call_id: string;
      output: string;
      is_error: boolean;
    }
  | {
      type: "ArtifactShown";
      call_id: string;
      artifact: { call_id: string; kind: string };
    }
  | { type: "TextDelta"; text: string }
  | { type: "Error"; message: string }
  | { type: "ApprovalRequested"; id: string; tool: string; args: unknown }
  | { type: "AskRequested"; id: string }
  | { type: "TurnFinished"; stop_reason: string; usage: unknown }
  | { type: string };

type ToolSpec = { name: string; schema: unknown; risk: string };
type Info = { default_model: string; app_enabled?: boolean };

type Turn = {
  session: string;
  events: Ev[];
  text: string;
  finished: Extract<Ev, { type: "TurnFinished" }> | null;
  timed_out: boolean;
  approvals: { tool: string; decision: string; args: unknown }[];
  errors: string[];
};

const HERE = dirname(Bun.fileURLToPath(import.meta.url));
const FIXTURE_SERVER = join(HERE, "fixture-server.ts");
/** Sessions with a turn in flight; SIGINT/SIGTERM cancels them before exit. */
const running = new Set<string>();
const DEFAULT_VISIBLE = "app,show,load_skill,argo_app_health,argo_sync";
const FENCE = /```(json-render|mermaid)\b/;
const TOOL_REF = /\b(Query|Mutation)\(\s*"([^"]+)"/g;

function sleep(ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
}

// ---------------------------------------------------------------------------------------------
// Pure evaluation (covered by --self-test)

export function expectationFailures(exp: Expect, r: CheckResult): string[] {
  const q = r.bindings.filter((b) => b.kind === "query").map((b) => b.tool);
  const m = r.bindings.filter((b) => b.kind === "mutation").map((b) => b.tool);
  const out: string[] = [];
  if (exp.no_bindings && r.bindings.length)
    out.push(
      `expected no tool bindings, got ${r.bindings.map((b) => `${b.kind}:${b.tool}`).join(",")}`,
    );
  if (exp.no_mutations && m.length)
    out.push(`expected no mutations, got ${m.join(",")}`);
  for (const t of exp.queries ?? [])
    if (!q.includes(t)) out.push(`missing Query(${t})`);
  for (const t of exp.mutations ?? [])
    if (!m.includes(t)) out.push(`missing Mutation(${t})`);
  if (exp.min_queries && q.length < exp.min_queries)
    out.push(`expected ≥${exp.min_queries} queries, got ${q.length}`);
  if (exp.min_mutations && m.length < exp.min_mutations)
    out.push(`expected ≥${exp.min_mutations} mutations, got ${m.length}`);
  const states = r.statements.filter((s) => s.kind === "state").length;
  if (exp.state_min && states < exp.state_min)
    out.push(`expected ≥${exp.state_min} $state, got ${states}`);
  return out;
}

/** Tool names referenced by Query/Mutation literals that the catalog does not have. */
export function missingTools(source: string, catalog: Set<string>): string[] {
  const seen = new Set<string>();
  for (const m of source.matchAll(TOOL_REF))
    if (!catalog.has(m[2]!)) seen.add(m[2]!);
  return [...seen];
}

/** First vehicle the turn used, in event order; the reply fence only counts when no tool did. */
export function vehicleOf(
  events: Ev[],
  text: string,
): { actual: Vehicle; also_fence: boolean } {
  let actual: Vehicle = "none";
  for (const ev of events) {
    if (
      ev.type === "ToolCallStarted" &&
      "name" in ev &&
      (ev.name === "app" || ev.name === "show")
    ) {
      actual = ev.name === "app" ? "app" : "show";
      break;
    }
    if (ev.type === "ArtifactShown" && "artifact" in ev) {
      actual = ev.artifact.kind === "app" ? "app" : "show";
      break;
    }
  }
  const fence = FENCE.test(text);
  if (actual === "none" && fence) return { actual: "fence", also_fence: false };
  return { actual, also_fence: fence };
}

// ---------------------------------------------------------------------------------------------
// One turn through the real agent loop

async function runTurn(
  rpc: Rpc,
  model: string,
  effort: string | undefined,
  hidden: string[],
  visible: Record<string, true>,
  title: string,
  prompt: string,
  timeoutMs: number,
): Promise<Turn> {
  const session = await rpc.call<string>("new_session", []);
  const t: Turn = {
    session,
    events: [],
    text: "",
    finished: null,
    timed_out: false,
    approvals: [],
    errors: [],
  };
  await rpc.call("rename_session", [session, title]);
  await rpc.call("set_model", [session, model]);
  if (effort) await rpc.call("set_effort", [session, effort]);
  if (hidden.length) await rpc.call("set_mcp_hidden", [session, hidden, true]);

  const finished =
    Promise.withResolvers<Extract<Ev, { type: "TurnFinished" }>>();
  const unsub = await rpc.subscribe(session, (raw) => {
    // Wire shape generated from the Rust event enums; no schema library is available to this script.
    const ev = raw as Ev;
    t.events.push(ev);
    if (ev.type === "TextDelta" && "text" in ev) t.text += ev.text;
    else if (ev.type === "Error" && "message" in ev) t.errors.push(ev.message);
    else if (ev.type === "ApprovalRequested" && "tool" in ev) {
      const decision = visible[ev.tool] ? "allow" : "deny";
      t.approvals.push({ tool: ev.tool, decision, args: ev.args });
      rpc
        .call("resolve_approval", [ev.id, decision])
        .catch((e: unknown) => t.errors.push(`resolve_approval: ${e}`));
    } else if (ev.type === "AskRequested" && "id" in ev) {
      // A clarifying question is a failed steer; dismissing it cancels the turn.
      rpc
        .call("answer_ask", [ev.id, null])
        .catch((e: unknown) => t.errors.push(`answer_ask: ${e}`));
    } else if (ev.type === "TurnFinished" && "stop_reason" in ev)
      finished.resolve(ev);
  });
  running.add(session);
  try {
    await rpc.call("send_input", [session, prompt, []]);
    t.finished = await Promise.race([
      finished.promise,
      sleep(timeoutMs).then(() => null),
    ]);
    if (!t.finished) {
      t.timed_out = true;
      await rpc.call("cancel", [session]).catch(() => {});
      t.finished = await Promise.race([
        finished.promise,
        sleep(10_000).then(() => null),
      ]);
    }
  } finally {
    running.delete(session);
    await unsub().catch(() => {});
  }
  return t;
}

// ---------------------------------------------------------------------------------------------
// Steering analysis

type Attempt = {
  call_id: string;
  edit: boolean;
  base_call_id: string | null;
  source_path: string;
  harness_is_error: boolean | null;
  harness_output: string;
  artifact_call_id: string | null;
  local_ok: boolean;
  diagnostics: CheckResult["diagnostics"];
  bindings: CheckResult["bindings"];
  expectation_failures: string[];
  missing_tools: string[];
  valid: boolean;
  /** Local check and harness disagree: one accepted, the other rejected. */
  disagreement: boolean;
};

function analyzeSteering(
  c: SteeringCase,
  t: Turn,
  snapshot: ToolSnapshot[],
  catalog: Set<string>,
  dir: string,
) {
  const finishedBy = new Map<
    string,
    Extract<Ev, { type: "ToolCallFinished" }>
  >();
  const artifactBy = new Map<string, Extract<Ev, { type: "ArtifactShown" }>>();
  const started: Extract<Ev, { type: "ToolCallStarted" }>[] = [];
  for (const ev of t.events) {
    if (ev.type === "ToolCallFinished" && "is_error" in ev)
      finishedBy.set(ev.call_id, ev);
    else if (ev.type === "ArtifactShown" && "artifact" in ev)
      artifactBy.set(ev.call_id, ev);
    else if (ev.type === "ToolCallStarted" && "name" in ev) started.push(ev);
  }
  const accepted = new Map<string, string>(); // record call_id -> canonical source
  const attempts: Attempt[] = [];
  let n = 0;
  for (const ev of started) {
    if (ev.name !== "app") continue;
    n++;
    const source = typeof ev.args.source === "string" ? ev.args.source : "";
    const edit = ev.args.edit === true;
    const base =
      typeof ev.args.base_call_id === "string" ? ev.args.base_call_id : null;
    const baseSource = edit && base ? accepted.get(base) : undefined;
    const r =
      baseSource !== undefined
        ? mergeCheck(baseSource, source, snapshot)
        : check(source, snapshot);
    const fin = finishedBy.get(ev.call_id);
    const harnessErr = fin ? fin.is_error : null;
    const local_ok =
      r.diagnostics.length === 0 && (!edit || baseSource !== undefined);
    if (harnessErr === false) accepted.set(ev.call_id, r.source);
    const source_path = join(dir, `${c.id}.${n}${edit ? ".patch" : ""}.openui`);
    writeFileSync(source_path, source);
    const expectation_failures = expectationFailures(c.expect, r);
    attempts.push({
      call_id: ev.call_id,
      edit,
      base_call_id: base,
      source_path,
      harness_is_error: harnessErr,
      harness_output: (fin?.output ?? "").slice(0, 800),
      artifact_call_id: artifactBy.get(ev.call_id)?.artifact.call_id ?? null,
      local_ok,
      diagnostics: r.diagnostics,
      bindings: r.bindings,
      expectation_failures,
      missing_tools: missingTools(source, catalog),
      valid:
        local_ok && harnessErr === false && expectation_failures.length === 0,
      disagreement: harnessErr !== null && local_ok === harnessErr,
    });
  }
  const first = attempts[0];
  const outcome = t.timed_out
    ? "timeout"
    : !attempts.length
      ? t.errors.length
        ? "error"
        : "no_app_call"
      : first!.valid
        ? "valid"
        : attempts.some((a) => a.valid)
          ? "corrected"
          : "invalid";
  return {
    id: c.id,
    group: c.group,
    session: t.session,
    outcome,
    first_pass_valid: first?.valid ?? false,
    corrected: attempts.length
      ? attempts.some((a) => a.valid) && !first!.valid
      : null,
    attempts,
    missing_tools: [...new Set(attempts.flatMap((a) => a.missing_tools))],
    other_tools_called: [
      ...new Set(started.filter((e) => e.name !== "app").map((e) => e.name)),
    ],
    approvals: t.approvals,
    errors: t.errors,
    stop_reason: t.finished?.stop_reason ?? null,
    usage: t.finished?.usage ?? null,
    reply_excerpt: t.text.slice(0, 600),
    review: attempts.length
      ? {
          needed: true,
          note: "positional/visual intent not judged automatically; open the session or the .openui files",
          sources: attempts.map((a) => a.source_path),
        }
      : { needed: false },
  };
}

function analyzeVehicle(c: VehicleCase, t: Turn) {
  const { actual, also_fence } = vehicleOf(t.events, t.text);
  const tools = new Set<string>();
  for (const ev of t.events)
    if (ev.type === "ToolCallStarted" && "name" in ev) tools.add(ev.name);
  return {
    id: c.id,
    session: t.session,
    expect: c.expect,
    actual,
    also_fence,
    correct: !t.timed_out && actual === c.expect,
    timed_out: t.timed_out,
    tools_called: [...tools],
    approvals: t.approvals,
    errors: t.errors,
    stop_reason: t.finished?.stop_reason ?? null,
    reply_excerpt: t.text.slice(0, 400),
  };
}

// ---------------------------------------------------------------------------------------------
// Pool, catalog, main

async function pool<T, R>(
  items: T[],
  n: number,
  fn: (x: T) => Promise<R>,
): Promise<R[]> {
  const out: R[] = new Array(items.length);
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(n, items.length) }, async () => {
      for (;;) {
        const i = next++;
        if (i >= items.length) return;
        out[i] = await fn(items[i]!);
      }
    }),
  );
  return out;
}

async function catalog(
  rpc: Rpc,
  required: string[],
  fixtureState: string | undefined,
) {
  let tools = await rpc.call<ToolSpec[]>("list_tools", []);
  let names = new Set(tools.map((t) => t.name));
  if (
    fixtureState &&
    required.some((n) => n.startsWith("argo_") && !names.has(n))
  ) {
    await rpc.call("add_mcp_server", [
      "argo",
      {
        command: "bun",
        args: [
          FIXTURE_SERVER,
          "--mode",
          "mcp",
          "--state",
          resolve(fixtureState),
        ],
      },
    ]);
    tools = await rpc.call<ToolSpec[]>("list_tools", []);
    names = new Set(tools.map((t) => t.name));
  }
  const missing = required.filter((n) => !names.has(n));
  if (missing.length)
    throw new Error(
      `tool catalog missing ${missing.join(", ")} (pass --fixture-state to register the argo fixture; the app tool needs app.enabled)`,
    );
  const snapshot: ToolSnapshot[] = tools.map((t) => ({
    name: t.name,
    schema: t.schema,
    risk: t.risk,
    identity: t.name,
  }));
  return { snapshot, names };
}

function selfTest() {
  const snapshot: ToolSnapshot[] = [
    {
      name: "argo_app_health",
      schema: {
        type: "object",
        properties: { name: { type: "string" } },
        required: ["name"],
      },
      risk: "read_only",
      identity: "t",
    },
    {
      name: "argo_sync",
      schema: {
        type: "object",
        properties: { name: { type: "string" } },
        required: ["name"],
      },
      risk: "mutating",
      identity: "t",
    },
  ];
  const proto = `root = Stack([title, status, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
controls = Stack([begin, reset], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))`;
  const panel = `root = Stack([title, healthText, syncBtn, resultText])
$app = "portal-web"
title = TextContent("## ArgoCD health")
health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 30)
healthText = TextContent($app + ": " + health.status)
syncResult = Mutation("argo_sync", {name: $app})
syncBtn = Button("Sync " + $app, Action([@Run(syncResult), @Run(health)]))
resultText = TextContent(syncResult.status == "success" ? syncResult.data.message : "")`;
  const p = check(proto, snapshot);
  const a = check(panel, snapshot);
  const assert = (cond: unknown, msg: string) => {
    if (!cond) throw new Error(`self-test: ${msg}`);
  };
  assert(
    p.diagnostics.length === 0,
    `prototype should check clean: ${JSON.stringify(p.diagnostics)}`,
  );
  assert(
    a.diagnostics.length === 0,
    `panel should check clean: ${JSON.stringify(a.diagnostics)}`,
  );
  assert(
    expectationFailures({ no_bindings: true, state_min: 1 }, p).length === 0,
    "prototype expectation",
  );
  assert(
    expectationFailures(
      { queries: ["argo_app_health"], mutations: ["argo_sync"], state_min: 1 },
      a,
    ).length === 0,
    "action expectation",
  );
  assert(
    expectationFailures({ no_mutations: true }, a).length === 1,
    "read expectation rejects a mutation",
  );
  assert(
    expectationFailures({ min_queries: 2 }, a)[0]?.startsWith(
      "expected ≥2 queries",
    ),
    "min_queries",
  );
  const bad = check(panel.replace("argo_sync", "argo_rollback"), snapshot);
  assert(
    bad.diagnostics.some((d) => d.code === "tool-unknown"),
    "unknown tool is a diagnostic",
  );
  assert(
    missingTools(bad.source, new Set(snapshot.map((s) => s.name))).join() ===
      "argo_rollback",
    "missing tool named",
  );
  assert(
    vehicleOf(
      [{ type: "ToolCallStarted", call_id: "c", name: "app", args: {} }],
      "```mermaid\nx",
    ).actual === "app",
    "tool beats fence",
  );
  assert(
    vehicleOf(
      [
        {
          type: "ArtifactShown",
          call_id: "c",
          artifact: { call_id: "c", kind: "csv" },
        },
      ],
      "",
    ).actual === "show",
    "artifact → show",
  );
  assert(
    vehicleOf([], "here:\n```json-render\n{}\n```").actual === "fence",
    "fence",
  );
  assert(vehicleOf([], "plain").actual === "none", "none");
  console.log(
    JSON.stringify({ self_test: "ok", library_hash: p.library_hash }),
  );
}

async function main() {
  const args = parseArgs(Bun.argv.slice(2)).flags;
  const str = (k: string, d?: string) => {
    const v = args[k];
    return typeof v === "string" ? v : d;
  };
  if (args["self-test"]) return selfTest();

  const dataDir = str("data-dir", "/tmp/theuth-apps-e2e.B4KN1l")!;
  const url = str("url", "ws://127.0.0.1:7777")!;
  const token = str("token") ?? readToken(dataDir);
  const suite = str("suite", "all")!;
  const limit = Number(str("limit", "0"));
  const concurrency = Math.min(2, Math.max(1, Number(str("concurrency", "2"))));
  const timeoutMs = Number(str("timeout", "240000"));
  const effort = str("effort");
  const outDir = str(
    "out",
    `/tmp/theuth-apps-steering-${new Date().toISOString().replace(/[:.]/g, "-")}`,
  )!;
  const defaultTier = str("tier", "frontier")!;
  const visible: Record<string, true> = {};
  for (const n of str("visible", DEFAULT_VISIBLE)!.split(","))
    if (n.trim()) visible[n.trim()] = true;
  const cases: Cases = await Bun.file(
    str("cases", join(HERE, "steering-cases.json"))!,
  ).json();
  if (args["fixture-state"] && !existsSync(FIXTURE_SERVER))
    throw new Error(`fixture server missing: ${FIXTURE_SERVER}`);

  const rpc = await connect(url, token);
  const bail = async () => {
    await Promise.all(
      [...running].map((s) => rpc.call("cancel", [s]).catch(() => {})),
    );
    rpc.close();
    process.exit(130);
  };
  process.on("SIGINT", bail);
  process.on("SIGTERM", bail);

  try {
    const info = await rpc.call<Info>("info", []);
    const models = (str("models") ?? `${info.default_model}:${defaultTier}`)
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((s) => {
        const m = /^(.*?)(?::(frontier|small))?$/.exec(s)!;
        return { id: m[1]!, tier: m[2] ?? defaultTier };
      });
    const { snapshot, names } = await catalog(
      rpc,
      cases.catalog_required,
      str("fixture-state"),
    );
    const hidden = [...names].filter((n) => !visible[n]);
    const steering =
      suite === "vehicle"
        ? []
        : limit > 0
          ? cases.steering.slice(0, limit)
          : cases.steering;
    const vehicle =
      suite === "steering"
        ? []
        : limit > 0
          ? cases.vehicle.slice(0, limit)
          : cases.vehicle;

    const plan = {
      url,
      data_dir: dataDir,
      out: outDir,
      models,
      suite,
      limit,
      concurrency,
      timeout_ms: timeoutMs,
      effort: effort ?? null,
      app_enabled: info.app_enabled ?? null,
      visible_tools: Object.keys(visible),
      hidden_tools: hidden,
      steering: steering.length,
      vehicle: vehicle.length,
    };
    if (args["dry-run"]) return console.log(JSON.stringify(plan, null, 2));
    mkdirSync(outDir, { recursive: true });

    const perModel = [];
    for (const m of models) {
      const dir = join(outDir, m.id.replace(/[^a-zA-Z0-9._-]+/g, "_"));
      mkdirSync(dir, { recursive: true });
      const turn = async (title: string, prompt: string): Promise<Turn> => {
        try {
          return await runTurn(
            rpc,
            m.id,
            effort,
            hidden,
            visible,
            title,
            prompt,
            timeoutMs,
          );
        } catch (e) {
          return {
            session: "",
            events: [],
            text: "",
            finished: null,
            timed_out: false,
            approvals: [],
            errors: [String(e)],
          };
        }
      };
      const steeringResults = await pool(steering, concurrency, async (c) => {
        const t = await turn(`steering ${m.id} ${c.id}`, c.prompt);
        return analyzeSteering(c, t, snapshot, names, dir);
      });
      const vehicleResults = await pool(vehicle, concurrency, async (c) =>
        analyzeVehicle(c, await turn(`vehicle ${m.id} ${c.id}`, c.prompt)),
      );

      const sValid = steeringResults.filter(
        (r) => r.outcome === "valid",
      ).length;
      const sThreshold =
        cases.thresholds.steering[m.tier] ??
        cases.thresholds.steering.frontier!;
      const vCorrect = vehicleResults.filter((r) => r.correct).length;
      const vThreshold =
        cases.thresholds.vehicle[m.tier] ?? cases.thresholds.vehicle.frontier!;
      const fullS = steering.length === cases.steering.length;
      const fullV = vehicle.length === cases.vehicle.length;
      const byGroup: Record<string, { valid: number; total: number }> = {};
      for (const r of steeringResults) {
        const g = (byGroup[r.group] ??= { valid: 0, total: 0 });
        g.total++;
        if (r.outcome === "valid") g.valid++;
      }
      perModel.push({
        model: m.id,
        tier: m.tier,
        steering: steering.length
          ? {
              valid_first_pass: sValid,
              total: steering.length,
              threshold: sThreshold,
              pass: fullS ? sValid >= sThreshold : null,
              partial: !fullS,
              corrected: steeringResults.filter(
                (r) => r.outcome === "corrected",
              ).length,
              outcomes: Object.fromEntries(
                steeringResults.map((r) => [r.id, r.outcome]),
              ),
              by_group: byGroup,
              missing_tools: [
                ...new Set(steeringResults.flatMap((r) => r.missing_tools)),
              ],
              disagreements: steeringResults
                .filter((r) => r.attempts.some((a) => a.disagreement))
                .map((r) => r.id),
              review_needed: steeringResults
                .filter((r) => r.review.needed)
                .map((r) => r.id),
            }
          : null,
        vehicle: vehicle.length
          ? {
              correct: vCorrect,
              total: vehicle.length,
              threshold: vThreshold,
              pass: fullV ? vCorrect >= vThreshold : null,
              partial: !fullV,
              outcomes: Object.fromEntries(
                vehicleResults.map((r) => [
                  r.id,
                  `${r.expect}→${r.actual}${r.correct ? "" : " ✗"}`,
                ]),
              ),
            }
          : null,
        cases: { steering: steeringResults, vehicle: vehicleResults },
      });
    }

    const results_path = join(outDir, "results.json");
    writeFileSync(
      results_path,
      JSON.stringify({ plan, models: perModel }, null, 2),
    );
    const summary = {
      plan,
      library_hash: check('root = TextContent("x")', []).library_hash,
      models: perModel.map(({ cases: _, ...rest }) => rest),
      results_path,
    };
    console.log(JSON.stringify(summary, null, 2));
    // Partial runs (--limit) never pass: `pass: null` is a failure exit so a smoke run cannot read as success.
    process.exitCode = perModel.every(
      (m) =>
        (m.steering ? m.steering.pass === true : true) &&
        (m.vehicle ? m.vehicle.pass === true : true),
    )
      ? 0
      : 1;
  } finally {
    rpc.close();
  }
}

await main();
