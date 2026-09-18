/**
 * Apps end-to-end smoke driver (design 46): real `wth --headless` over its WebSocket RPC,
 * a scripted OpenAI-compatible model and an ordinary MCP stdio fixture. No mocks of the app
 * plugin: publishing goes model → `app` tool → artifact store → `ArtifactShown`; every
 * Query/Mutation goes through `app_call_tool` → registry → the MCP fixture.
 *
 * Prerequisites the parent launches (this script starts nothing but the model fixture, and
 * only when `--fixture-url` is absent):
 *
 *   target/debug/wth --headless --data-dir /tmp/wth-apps-e2e.XXXX        (rpc.listen in config.yaml)
 *   bun scripts/apps/fixture-server.ts --mode openai --port 8765          (optional; else spawned here)
 *
 * Run:
 *
 *   bun /Users/Sky.Moore/Develop/wth-apps/scripts/apps/smoke.ts \
 *     --url ws://127.0.0.1:7777 --data-dir /tmp/wth-apps-e2e.XXXX \
 *     [--fixture-url http://127.0.0.1:8765] [--fixture-port 8765] \
 *     [--state /tmp/wth-apps-e2e.XXXX/argo-state.json] \
 *     [--only publish,auto_read,press] [--skip cas_edit] \
 *     [--expect-destructive ask|deny] [--record-call-id <call_id>] [--timeout-ms 30000] [--keep] [--out result.json]
 *
 * Scenarios (in order; `--only` picks a subset, `publish` is implied unless `--record-call-id`):
 *   publish            fake model turn calls `app`; ToolCallProgress for that call arrives before
 *                      ArtifactShown; receipt names the record call_id; kind "app".
 *   get_app            bindings/risks match fixtures.json; library hash matches packages/apps; source
 *                      carries the `{{secret:APP_SMOKE}}` placeholder, never the value.
 *   auto_read          `load` on the read-only health Query succeeds; count equals the state file.
 *   needs_approval     `load` on syncProbe (a Query bound to the Mutating sync tool) → needs_approval,
 *                      zero ApprovalRequested, count unchanged. Skipped when tools.approvals is off.
 *   auto_mutation      `load` on the sync Mutation → unavailable (a mutation never runs without a
 *                      press), is_error, count unchanged.
 *   press              `press` sync → succeeded, count+1 in reply and state file, zero ApprovalRequested.
 *   duplicate          same request_id + args again → same count; app_call_status agrees.
 *   changed_args       same request_id, different args → refused (is_error), counts unchanged.
 *   destroy            `press` destroy (Destructive): `--expect-destructive ask` (default, policy ask)
 *                      → succeeded, count 0, zero ApprovalRequested; `deny` (run against a harness
 *                      whose config has tools.policy.destructive: deny) → unavailable, count unchanged.
 *   echo_auto          `load` on the echo Query whose arg is `{{secret:APP_SMOKE}}` → needs_approval
 *                      (secret use is an Ask without activation); fixture not called. Skipped when
 *                      tools.approvals is off.
 *   echo_press         `press` echo → succeeded; structured.length equals the real value's length
 *                      (the harness expanded the placeholder) while structured.echoed is the placeholder
 *                      again (scrubbed); the value appears in no RPC result, no recorded event, not the
 *                      state file. Zero ApprovalRequested.
 *   stale_generation   in-flight slow read at generation 1 is cancelled when generation 2 reads;
 *                      a late generation-1 read does not succeed.
 *   peer_disconnect    a second WebSocket starts a slow read and drops; the request ends not-running
 *                      (AppCallChanged cancelled or app_call_status not running); fixture still answers.
 *   cas_edit           one model turn with two `app` edits on the same base_call_id: exactly one
 *                      new ArtifactShown, the other call fails.
 *
 * Output: one JSON document on stdout ({ok, scenarios[], blocked[], record_call_id, ...}); exit 1 on any
 * failed assertion or blocked scenario. A scenario is `skipped` (ok) when the harness config makes it
 * moot, `blocked` (not ok) when publish failed so it never ran. `--out <file>` writes the same document
 * plus the raw recorded event stream (`events`) for post-mortems; stdout redacts the fixture secret value.
 * Mutations only touch the fixture state file. Unless `--keep`, the `argo` MCP server, the `fixture`
 * provider and the `APP_SMOKE` secret added here are removed on exit. A turn still running when the
 * driver stops waiting on it (timeout, SIGINT) is cancelled on exit even with `--keep`.
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import fixtures from "./fixtures.json";
import {
  connect,
  parseArgs,
  readToken,
  recorder,
  type Recorder,
  type Rpc,
  type WireEvent,
} from "./rpc.ts";

// ---------------------------------------------------------------------------------------------
// wire types (subset of the Rust structs, fields this driver reads)
// ---------------------------------------------------------------------------------------------

interface Info {
  app_enabled?: boolean;
  approvals: { global: boolean };
}
interface McpServer {
  name: string;
  connected: boolean;
  tools: { name: string; risk: string }[];
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
  source: string;
  library_hash: string;
  bindings: {
    statement_id: string;
    tool: string;
    kind: string;
    risk: string;
  }[];
  diagnostics: unknown[];
}
type CallStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "unknown"
  | "needs_approval"
  | "unavailable"
  | "busy"
  | "cancelled";
interface AppToolResult {
  request_id: string;
  content: string;
  structured: Record<string, unknown> | null;
  is_error: boolean;
  status: CallStatus;
}
interface CallArgs {
  record_call_id: string;
  instance_id: string;
  generation: number;
  statement_id: string;
  request_id: string;
  args: Record<string, unknown>;
  trigger: "load" | "refresh" | "state_change" | "press";
  action_id?: string;
}
interface State {
  counts: Record<string, number>;
  calls: unknown[];
}

// ---------------------------------------------------------------------------------------------
// assertions
// ---------------------------------------------------------------------------------------------

interface Assertion {
  name: string;
  ok: boolean;
  expected?: unknown;
  actual?: unknown;
}
interface ScenarioResult {
  name: string;
  ok: boolean;
  ms: number;
  assertions: Assertion[];
  /** Ran-as-intended skip (e.g. feature off); counts as ok. */
  skipped?: string;
  /** Never ran: an upstream scenario it depends on failed; counts as failed. */
  blocked?: string;
  error?: string;
  notes?: Record<string, unknown>;
}

class Check {
  assertions: Assertion[] = [];
  notes: Record<string, unknown> = {};
  eq(name: string, actual: unknown, expected: unknown) {
    const ok = canonical(actual) === canonical(expected);
    this.assertions.push({ name, ok, expected, actual });
    return ok;
  }
  ok(name: string, ok: boolean, actual?: unknown) {
    this.assertions.push({ name, ok, actual });
    return ok;
  }
  oneOf(name: string, actual: unknown, expected: unknown[]) {
    const ok = expected.includes(actual);
    this.assertions.push({ name, ok, expected, actual });
    return ok;
  }
}

/** JSON with object keys sorted, so `eq` ignores key order. */
function canonical(v: unknown): string {
  return JSON.stringify(v, (_k, x: unknown) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.entries(x as Record<string, unknown>).sort(([a], [b]) =>
            a < b ? -1 : a > b ? 1 : 0,
          ),
        )
      : x,
  );
}

class Skip extends Error {}

// ---------------------------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------------------------

interface Ctx {
  rpc: Rpc;
  rec: Recorder;
  session: string;
  record: string;
  url: string;
  token: string;
  fixtureUrl: string;
  statePath: string;
  timeout: number;
  approvals: boolean;
  expectDestructive: "ask" | "deny";
  /** A model turn was sent and its TurnFinished not yet seen (timed out → still waiting on the harness). */
  turnOpen: boolean;
}

const HERE = dirname(fileURLToPath(import.meta.url));
const APP = fixtures.app;
const APP_NAME = "portal-web";
const SOURCE = APP.source.join("\n");

const { flags } = parseArgs(process.argv.slice(2));
const str = (k: string, d?: string) =>
  typeof flags[k] === "string" ? (flags[k] as string) : d;
const outPath = str("out");
/** The harness session in use, once connected; the crash and SIGINT paths cancel its open turn. */
let live: Ctx | undefined;

/** Final document: stdout redacted (no events, no secret value); `--out` verbatim plus the raw event stream. */
function report(
  summary: Record<string, unknown>,
  events: WireEvent[] | undefined,
) {
  if (outPath)
    writeFileSync(
      outPath,
      JSON.stringify({ ...summary, events: events ?? [] }, null, 2),
    );
  console.log(
    JSON.stringify(summary, null, 2).replaceAll(
      fixtures.secret.value,
      "<redacted:APP_SMOKE>",
    ),
  );
}

async function sleep(ms: number) {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
}

function readState(path: string): State {
  try {
    return JSON.parse(readFileSync(path, "utf8")) as State;
  } catch {
    return { counts: {}, calls: [] };
  }
}

function call(
  ctx: Ctx,
  rpc: Rpc,
  over: Partial<CallArgs> & { statement_id: string },
): Promise<AppToolResult> {
  const args: CallArgs = {
    record_call_id: ctx.record,
    instance_id: over.instance_id ?? `inst-${crypto.randomUUID()}`,
    generation: over.generation ?? 1,
    request_id: over.request_id ?? `req-${crypto.randomUUID()}`,
    args: over.args ?? { name: APP_NAME },
    trigger: over.trigger ?? "load",
    statement_id: over.statement_id,
    ...(over.action_id ? { action_id: over.action_id } : {}),
  };
  return rpc.call<AppToolResult>("app_call_tool", [args]);
}

/** ApprovalRequested events for our session since `from`. */
function approvals(ctx: Ctx, from: number, tool?: string) {
  return ctx.rec.events
    .slice(from)
    .filter(
      (e) =>
        e.type === "ApprovalRequested" &&
        e.session === ctx.session &&
        (!tool || e.tool === tool),
    );
}

async function script(ctx: Ctx, steps: unknown[]) {
  await fetch(`${ctx.fixtureUrl}/fixture/requests`, { method: "DELETE" });
  const r = await fetch(`${ctx.fixtureUrl}/fixture/script`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ steps, default_text: "Done." }),
  });
  if (!r.ok) throw new Error(`fixture script: ${r.status}`);
}

/** One fake-model turn: returns the `app` call ids it produced, in ToolCallPending order. */
async function turn(
  ctx: Ctx,
  text: string,
): Promise<{ from: number; calls: string[] }> {
  const from = ctx.rec.events.length;
  ctx.turnOpen = true;
  await ctx.rpc.call("send_input", [ctx.session, text, []]);
  const ended = await ctx.rec
    .waitFor(
      (e) => e.type === "TurnFinished" && e.session === ctx.session,
      ctx.timeout,
      "TurnFinished",
      from,
    )
    .catch((e: Error) => {
      // The usual reason a scripted turn never ends: a tool call parked on a human approval.
      const pending = approvals(ctx, from).map((a) => ({
        id: a.id,
        tool: a.tool,
        risk: a.risk,
      }));
      throw new Error(
        pending.length
          ? `${e.message}; pending approvals: ${JSON.stringify(pending)}`
          : e.message,
      );
    });
  ctx.turnOpen = false;
  const calls = ctx.rec.events
    .slice(from)
    .filter(
      (e) =>
        e.type === "ToolCallPending" &&
        e.session === ctx.session &&
        e.name === "app",
    )
    .map((e) => String(e.call_id));
  if (ended.stop_reason === "error")
    throw new Error(`turn errored: ${JSON.stringify(ended)}`);
  return { from, calls };
}

/** Abort a turn this driver left running (timed out, or interrupted), so no approval wait or agent outlives us. */
async function cancelOpenTurn(ctx: Ctx | undefined) {
  if (!ctx?.turnOpen) return;
  const from = ctx.rec.events.length;
  await ctx.rpc.call("cancel", [ctx.session]).catch(() => {});
  await ctx.rec
    .waitFor(
      (e) =>
        e.type === "TurnFinished" &&
        e.session === ctx.session &&
        e.stop_reason === "cancelled",
      5000,
      "cancel",
      from,
    )
    .catch(() => {});
  ctx.turnOpen = false;
}

// ---------------------------------------------------------------------------------------------
// scenarios
// ---------------------------------------------------------------------------------------------

const scenarios: Record<string, (ctx: Ctx, c: Check) => Promise<void>> = {
  async publish(ctx, c) {
    await script(ctx, [
      {
        tool_calls: [
          {
            name: "app",
            args: { name: APP.name, title: APP.title, source: SOURCE },
          },
        ],
      },
    ]);
    const { from, calls } = await turn(
      ctx,
      "Publish the ArgoCD fixture panel.",
    );
    c.eq("one app call in the turn", calls.length, 1);
    const callId = calls[0];
    if (!callId)
      throw new Error(
        "no app ToolCallPending; model fixture not wired to this session?",
      );
    const ev = ctx.rec.events.slice(from);
    const progressAt = ev.findIndex(
      (e) => e.type === "ToolCallProgress" && e.call_id === callId,
    );
    const shownAt = ev.findIndex(
      (e) =>
        e.type === "ArtifactShown" &&
        (e.artifact as Artifact).call_id === callId,
    );
    const finished = ev.find(
      (e) => e.type === "ToolCallFinished" && e.call_id === callId,
    );
    c.ok("ToolCallProgress published for the app call", progressAt >= 0, {
      progress_events: ev.filter(
        (e) => e.type === "ToolCallProgress" && e.call_id === callId,
      ).length,
    });
    c.ok("ArtifactShown for the app call", shownAt >= 0);
    c.ok(
      "ToolCallProgress arrives before ArtifactShown",
      progressAt >= 0 && shownAt > progressAt,
      {
        progress_index: progressAt,
        shown_index: shownAt,
      },
    );
    c.eq("app call succeeded", finished?.is_error, false);
    c.ok(
      "receipt names the record call_id",
      String(finished?.output ?? "").includes(callId),
      finished?.output,
    );
    const shown =
      shownAt >= 0 ? (ev[shownAt]!.artifact as Artifact) : undefined;
    c.eq("artifact kind", shown?.kind, "app");
    c.eq("artifact session is ours", shown?.session, ctx.session);
    c.eq("artifact name", shown?.name, APP.name);
    c.notes.app_call_id = callId;
    c.notes.tool_output = finished?.output ?? null;
    if (finished?.is_error) c.notes.tool_error = finished.output;
    // Downstream scenarios need a record that actually exists: only a shown artifact qualifies.
    if (shown) {
      ctx.record = callId;
      c.notes.record_call_id = callId;
      c.notes.artifact = shown;
    }
    // What the agent handed back to the model: the tool message must carry the receipt.
    const reqs = (await (
      await fetch(`${ctx.fixtureUrl}/fixture/requests`)
    ).json()) as { last_role?: string }[];
    c.ok(
      "model saw the tool result round",
      reqs.some((r) => r.last_role === "tool"),
      reqs.map((r) => r.last_role),
    );
  },

  async get_app(ctx, c) {
    const doc = await ctx.rpc.call<AppDoc>("get_app", [ctx.record]);
    c.eq("record call_id", doc.artifact.call_id, ctx.record);
    c.eq("kind", doc.artifact.kind, "app");
    c.eq("no diagnostics", doc.diagnostics, []);
    c.ok(
      "source is the published program",
      doc.source.includes('Query("argo_app_health"'),
      doc.source.slice(0, 80),
    );
    c.ok(
      "source carries the secret placeholder",
      doc.source.includes(fixtures.secret.placeholder),
    );
    c.ok(
      "source never holds the secret value",
      !doc.source.includes(fixtures.secret.value),
    );
    const hashFile = join(
      HERE,
      "../../packages/apps/generated/library-hash.txt",
    );
    if (existsSync(hashFile))
      c.eq(
        "library hash matches packages/apps",
        doc.library_hash,
        readFileSync(hashFile, "utf8").trim(),
      );
    else
      c.ok(
        "library hash present",
        doc.library_hash.length > 0,
        doc.library_hash,
      );
    const got = Object.fromEntries(
      doc.bindings.map((b) => [
        b.statement_id,
        { tool: b.tool, kind: b.kind, risk: b.risk },
      ]),
    );
    c.eq("bindings", got, APP.statements);
  },

  async auto_read(ctx, c) {
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "health",
      trigger: "load",
    });
    c.eq("status", r.status, "succeeded");
    c.eq("is_error", r.is_error, false);
    c.eq("structured.count equals state file", r.structured?.count, before);
    c.eq("structured.synced", r.structured?.synced, before > 0);
    c.eq(
      "no mutation happened",
      readState(ctx.statePath).counts[APP_NAME] ?? 0,
      before,
    );
  },

  async needs_approval(ctx, c) {
    if (!ctx.approvals)
      throw new Skip("tools.approvals is off: every ask runs as allow");
    const from = ctx.rec.events.length;
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "syncProbe",
      trigger: "load",
    });
    c.eq("status", r.status, "needs_approval");
    c.eq("is_error", r.is_error, false);
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    c.eq(
      "count unchanged",
      readState(ctx.statePath).counts[APP_NAME] ?? 0,
      before,
    );
  },

  async auto_mutation(ctx, c) {
    const from = ctx.rec.events.length;
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "syncResult",
      trigger: "load",
    });
    c.eq("status", r.status, "unavailable");
    c.eq("is_error", r.is_error, true);
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    c.eq(
      "count unchanged",
      readState(ctx.statePath).counts[APP_NAME] ?? 0,
      before,
    );
    c.notes.content = r.content;
  },

  async press(ctx, c) {
    const from = ctx.rec.events.length;
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "syncResult",
      trigger: "press",
      action_id: APP.actions.sync,
    });
    c.eq("status", r.status, "succeeded");
    c.eq("structured.count incremented", r.structured?.count, before + 1);
    c.eq(
      "state file incremented",
      readState(ctx.statePath).counts[APP_NAME],
      before + 1,
    );
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    c.notes.content = r.content;
  },

  async duplicate(ctx, c) {
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const request_id = `req-dup-${crypto.randomUUID()}`;
    const instance_id = `inst-dup-${crypto.randomUUID()}`;
    const base = {
      statement_id: "syncResult",
      trigger: "press" as const,
      action_id: APP.actions.sync,
      request_id,
      instance_id,
    };
    const first = await call(ctx, ctx.rpc, base);
    const again = await call(ctx, ctx.rpc, base);
    c.eq("first press succeeded", first.status, "succeeded");
    c.eq("first count", first.structured?.count, before + 1);
    c.oneOf("duplicate is stable", again.status, ["succeeded", "running"]);
    c.eq(
      "duplicate count is the same",
      again.structured?.count,
      first.structured?.count,
    );
    c.eq(
      "state file incremented once",
      readState(ctx.statePath).counts[APP_NAME],
      before + 1,
    );
    const status = await ctx.rpc.call<AppToolResult>("app_call_status", [
      request_id,
    ]);
    c.eq("app_call_status agrees", status.status, "succeeded");
    c.eq("app_call_status echoes request_id", status.request_id, request_id);
    c.notes.changed_args_seed = { request_id, instance_id };
  },

  async changed_args(ctx, c) {
    const counts = readState(ctx.statePath).counts;
    const request_id = `req-chg-${crypto.randomUUID()}`;
    const instance_id = `inst-chg-${crypto.randomUUID()}`;
    const first = await call(ctx, ctx.rpc, {
      statement_id: "syncResult",
      trigger: "press",
      action_id: APP.actions.sync,
      request_id,
      instance_id,
    });
    c.eq("first press succeeded", first.status, "succeeded");
    const after = { ...counts, [APP_NAME]: (counts[APP_NAME] ?? 0) + 1 };
    const changed = await call(ctx, ctx.rpc, {
      statement_id: "syncResult",
      trigger: "press",
      action_id: APP.actions.sync,
      request_id,
      instance_id,
      args: { name: "portal-api" },
    });
    c.eq("changed args refused (is_error)", changed.is_error, true);
    c.oneOf("changed args status", changed.status, ["failed", "unavailable"]);
    c.eq(
      "no count changed by the refused call",
      readState(ctx.statePath).counts,
      after,
    );
    c.notes.refusal = changed.content;
  },

  async destroy(ctx, c) {
    const from = ctx.rec.events.length;
    const before = readState(ctx.statePath).counts[APP_NAME] ?? 0;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "destroyResult",
      trigger: "press",
      action_id: APP.actions.destroy,
    });
    if (ctx.expectDestructive === "deny") {
      c.eq("status under policy deny", r.status, "unavailable");
      c.eq("is_error", r.is_error, true);
      c.eq(
        "count unchanged",
        readState(ctx.statePath).counts[APP_NAME] ?? 0,
        before,
      );
    } else {
      c.eq("status", r.status, "succeeded");
      c.eq("structured.count reset", r.structured?.count, 0);
      c.eq("state file reset", readState(ctx.statePath).counts[APP_NAME], 0);
    }
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    c.notes.content = r.content;
  },

  async echo_auto(ctx, c) {
    if (!ctx.approvals)
      throw new Skip("tools.approvals is off: every ask runs as allow");
    const from = ctx.rec.events.length;
    const callsBefore = readState(ctx.statePath).calls.length;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "echoed",
      trigger: "load",
      args: { secret: fixtures.secret.placeholder },
    });
    c.eq("status", r.status, "needs_approval");
    c.eq("is_error", r.is_error, false);
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    c.eq(
      "fixture not called",
      readState(ctx.statePath).calls.length,
      callsBefore,
    );
    c.ok(
      "value absent from result",
      !JSON.stringify(r).includes(fixtures.secret.value),
    );
  },

  async echo_press(ctx, c) {
    const from = ctx.rec.events.length;
    const { value, placeholder } = fixtures.secret;
    const r = await call(ctx, ctx.rpc, {
      statement_id: "echoed",
      trigger: "press",
      action_id: APP.actions.echo,
      args: { secret: placeholder },
    });
    c.eq("status", r.status, "succeeded");
    c.eq("zero ApprovalRequested", approvals(ctx, from).length, 0);
    // Expansion: the fixture measured the real value, not the 20-char placeholder.
    c.eq(
      "placeholder expanded before dispatch (length)",
      r.structured?.length,
      value.length,
    );
    // Scrubbing: the fixture echoed the raw value; the harness turned it back into the placeholder.
    c.eq(
      "value scrubbed to placeholder in structured",
      r.structured?.echoed,
      placeholder,
    );
    c.ok("value absent from content", !r.content.includes(value), r.content);
    c.ok(
      "placeholder present in content",
      r.content.includes(placeholder),
      r.content,
    );
    c.ok(
      "value absent from every recorded event",
      !JSON.stringify(ctx.rec.events.slice(from)).includes(value),
    );
    c.ok(
      "value absent from state file",
      !readFileSync(ctx.statePath, "utf8").includes(value),
    );
  },

  async stale_generation(ctx, c) {
    const instance_id = `inst-gen-${crypto.randomUUID()}`;
    const slowId = `req-slow-${crypto.randomUUID()}`;
    const slow = call(ctx, ctx.rpc, {
      statement_id: "slow",
      trigger: "load",
      instance_id,
      generation: 1,
      request_id: slowId,
      args: { name: APP_NAME, ms: 2500 },
    });
    await sleep(300);
    const fresh = await call(ctx, ctx.rpc, {
      statement_id: "health",
      trigger: "load",
      instance_id,
      generation: 2,
    });
    c.eq("generation 2 read succeeds", fresh.status, "succeeded");
    const stale = await slow;
    c.eq("in-flight generation 1 read is cancelled", stale.status, "cancelled");
    c.eq("cancelled is_error", stale.is_error, true);
    const late = await call(ctx, ctx.rpc, {
      statement_id: "health",
      trigger: "load",
      instance_id,
      generation: 1,
    });
    c.ok(
      "late generation 1 read does not succeed",
      late.status !== "succeeded",
      late.status,
    );
    await ctx.rpc.call("app_close", [instance_id]);
  },

  async peer_disconnect(ctx, c) {
    const rpc2 = await connect(ctx.url, ctx.token);
    const instance_id = `inst-peer-${crypto.randomUUID()}`;
    const request_id = `req-peer-${crypto.randomUUID()}`;
    const from = ctx.rec.events.length;
    const inflight = call(ctx, rpc2, {
      statement_id: "slow",
      trigger: "load",
      instance_id,
      request_id,
      args: { name: APP_NAME, ms: 3000 },
    }).then(
      (r) => r.status,
      (e: Error) => `rejected: ${e.message}`,
    );
    await sleep(300);
    rpc2.close();
    await rpc2.closed;
    c.notes.inflight_outcome = await inflight;
    const changed = await ctx.rec
      .waitFor(
        (e) =>
          e.type === "AppCallChanged" &&
          e.request_id === request_id &&
          e.status !== "running",
        6000,
        "AppCallChanged after peer disconnect",
      )
      .catch(() => undefined);
    const status = await ctx.rpc.call<AppToolResult>("app_call_status", [
      request_id,
    ]);
    c.notes.app_call_changed = changed;
    c.notes.status_after = status.status;
    c.ok(
      "request not running after its peer disconnected",
      (changed !== undefined && changed.status !== "running") ||
        status.status !== "running",
      { changed: changed?.status, status: status.status },
    );
    c.eq(
      "no AppCallChanged succeeded for the dropped peer",
      ctx.rec.events
        .slice(from)
        .some(
          (e) =>
            e.type === "AppCallChanged" &&
            e.request_id === request_id &&
            e.status === "succeeded",
        ),
      false,
    );
    const health = await call(ctx, ctx.rpc, {
      statement_id: "health",
      trigger: "load",
    });
    c.eq(
      "fixture still answers on the live connection",
      health.status,
      "succeeded",
    );
  },

  async cas_edit(ctx, c) {
    const edit = (source: string) => ({
      name: "app",
      args: { name: APP.name, edit: true, base_call_id: ctx.record, source },
    });
    await script(ctx, [
      { tool_calls: [edit(fixtures.patches.a), edit(fixtures.patches.b)] },
    ]);
    const { from, calls } = await turn(ctx, "Apply both title edits.");
    c.eq("two app calls in the turn", calls.length, 2);
    const ev = ctx.rec.events.slice(from);
    const finished = calls.map((id) =>
      ev.find((e) => e.type === "ToolCallFinished" && e.call_id === id),
    );
    const shown = ev.filter(
      (e) =>
        e.type === "ArtifactShown" &&
        calls.includes((e.artifact as Artifact).call_id),
    );
    c.eq("both calls finished", finished.filter(Boolean).length, 2);
    c.eq("exactly one edit committed (ArtifactShown)", shown.length, 1);
    c.eq(
      "exactly one edit failed",
      finished.filter((f) => f?.is_error === true).length,
      1,
    );
    c.notes.outputs = finished.map((f) => ({
      call_id: f?.call_id,
      is_error: f?.is_error,
      output: f?.output,
    }));
    const winner = shown[0] ? (shown[0].artifact as Artifact) : undefined;
    if (winner) {
      const doc = await ctx.rpc.call<AppDoc>("get_app", [winner.call_id]);
      c.eq("new version", doc.artifact.version, 2);
      c.ok(
        "winner source carries its edit",
        /edit A|edit B/.test(doc.source),
        doc.source.split("\n")[2],
      );
      c.notes.winner_call_id = winner.call_id;
    }
  },
};

const ORDER = Object.keys(scenarios);

// ---------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------

async function main() {
  const url = str("url", "ws://127.0.0.1:7777")!;
  const dataDir = str("data-dir");
  const token = str("token") ?? (dataDir ? readToken(dataDir) : undefined);
  if (!token)
    throw new Error("--data-dir (for <data_dir>/token) or --token required");
  const timeout = Number(str("timeout-ms", "30000"));
  const statePath = str(
    "state",
    join(tmpdir(), "wth-apps-fixture", "argo-state.json"),
  )!;
  const expectDestructive =
    str("expect-destructive", "ask") === "deny" ? "deny" : "ask";
  const keep = flags.keep === true;
  const only = str("only")?.split(",").filter(Boolean);
  const skip = new Set(str("skip")?.split(",").filter(Boolean) ?? []);
  const record = str("record-call-id");
  let selected = only ? ORDER.filter((s) => only.includes(s)) : ORDER;
  if (!record && !selected.includes("publish"))
    selected = ["publish", ...selected];
  if (record) selected = selected.filter((s) => s !== "publish");
  selected = selected.filter((s) => !skip.has(s));

  const owned: { kill(): void }[] = [];
  const cleanup: (() => Promise<unknown>)[] = [];
  const bye = () => {
    for (const p of owned) p.kill();
  };
  process.on("exit", bye);
  process.on("SIGINT", () => {
    // Even when interrupted mid-turn, leave no approval prompt or agent waiting behind us.
    void cancelOpenTurn(live).finally(() => {
      bye();
      process.exit(130);
    });
  });

  // Model fixture: parent-launched (`--fixture-url`) or owned here.
  let fixtureUrl = str("fixture-url");
  if (!fixtureUrl) {
    const port = str("fixture-port", "8765")!;
    const child = Bun.spawn(
      [
        "bun",
        join(HERE, "fixture-server.ts"),
        "--mode",
        "openai",
        "--port",
        port,
      ],
      {
        stdout: "ignore",
        stderr: "inherit",
      },
    );
    owned.push(child);
    fixtureUrl = `http://127.0.0.1:${port}`;
  }
  fixtureUrl = fixtureUrl.replace(/\/+$/, "").replace(/\/v1$/, "");
  for (let i = 0; ; i++) {
    const ok = await fetch(`${fixtureUrl}/fixture/health`).then(
      (r) => r.ok,
      () => false,
    );
    if (ok) break;
    if (i > 50) throw new Error(`model fixture not reachable at ${fixtureUrl}`);
    await sleep(100);
  }

  const rpc = await connect(url, token);
  const info = await rpc.call<Info>("info");
  if (info.app_enabled === false)
    throw new Error(
      "harness reports app_enabled: false (app.enabled or older build)",
    );
  const rec = await recorder(rpc, null);

  // Provider + MCP fixture through the real RPC path.
  const setup: Record<string, unknown> = {};
  await rpc.call("remove_provider", [fixtures.model.provider]).catch(() => {});
  setup.provider = await rpc.call("add_provider", [
    fixtures.model.provider,
    `${fixtureUrl}/v1`,
    fixtures.model.key,
  ]);
  if (!keep)
    cleanup.push(() =>
      rpc.call("remove_provider", [fixtures.model.provider]).catch(() => {}),
    );
  await rpc.call("remove_mcp_server", [fixtures.mcp.name]).catch(() => {});
  const server = await rpc.call<McpServer>("add_mcp_server", [
    fixtures.mcp.name,
    {
      command: "bun",
      args: [
        join(HERE, "fixture-server.ts"),
        "--mode",
        "mcp",
        "--state",
        statePath,
      ],
    },
  ]);
  setup.mcp = {
    connected: server.connected,
    tools: server.tools.map((t) => `${t.name}:${t.risk}`),
  };
  if (!keep)
    cleanup.push(() =>
      rpc.call("remove_mcp_server", [fixtures.mcp.name]).catch(() => {}),
    );
  const toolNames = server.tools.map((t) => t.name).sort();
  if (JSON.stringify(toolNames) !== JSON.stringify(fixtures.mcp.tools))
    throw new Error(
      `fixture tools ${JSON.stringify(toolNames)} != ${JSON.stringify(fixtures.mcp.tools)}`,
    );
  // The vault secret the app's echo Query references as `{{secret:APP_SMOKE}}`; the value never
  // appears in source, args, events or results — only its length proves expansion.
  setup.secret = await rpc.call("set_secret", [
    fixtures.secret.name,
    fixtures.secret.value,
    "apps smoke fixture",
  ]);
  if (!keep)
    cleanup.push(() =>
      rpc.call("delete_secret", [fixtures.secret.name]).catch(() => {}),
    );

  const session = await rpc.call<string>("new_session");
  await rpc.call("set_model", [
    session,
    `${fixtures.model.provider}/${fixtures.model.id}`,
  ]);
  setup.session = session;

  const ctx: Ctx = {
    rpc,
    rec,
    session,
    record: record ?? "",
    url,
    token,
    fixtureUrl,
    statePath,
    timeout,
    approvals: info.approvals.global,
    expectDestructive,
    turnOpen: false,
  };
  live = ctx;
  if (record) {
    // Reusing a record: it must belong to a session; the app_call path does not need ours.
    // A stale/unknown id is a reported failure, not a crash: every scenario then lands `blocked`.
    try {
      const doc = await rpc.call<AppDoc>("get_app", [record]);
      ctx.session = doc.artifact.session;
      setup.reused_record = {
        call_id: record,
        session: doc.artifact.session,
        version: doc.artifact.version,
      };
    } catch (e) {
      ctx.record = "";
      setup.reused_record = {
        call_id: record,
        error: e instanceof Error ? e.message : String(e),
      };
    }
  }

  const results: ScenarioResult[] = [];
  for (const name of selected) {
    const c = new Check();
    const t0 = Date.now();
    const out: ScenarioResult = {
      name,
      ok: false,
      ms: 0,
      assertions: c.assertions,
    };
    if (name !== "publish" && !ctx.record) {
      // Not a pass: the scenario never ran because its prerequisite failed upstream.
      out.blocked = "no app record (publish failed or was skipped)";
      results.push(out);
      continue;
    }
    try {
      await scenarios[name]!(ctx, c);
      out.ok = c.assertions.every((a) => a.ok);
    } catch (e) {
      if (e instanceof Skip) {
        out.skipped = e.message;
        out.ok = true;
      } else {
        out.error = e instanceof Error ? e.message : String(e);
      }
    }
    out.ms = Date.now() - t0;
    if (Object.keys(c.notes).length) out.notes = c.notes;
    results.push(out);
  }

  // Regardless of --keep: a turn we stopped waiting on must not stay parked on an approval.
  await cancelOpenTurn(ctx);
  for (const f of cleanup) await f();
  await rec.stop().catch(() => {});
  rpc.close();

  const ok = results.every((r) => r.ok);
  const summary = {
    ok,
    url,
    fixture_url: fixtureUrl,
    state_path: statePath,
    state: readState(statePath).counts,
    setup,
    record_call_id: ctx.record || null,
    blocked: results.filter((r) => r.blocked).map((r) => r.name),
    scenarios: results,
    event_types: countTypes(rec.events),
    out: outPath ?? null,
  };
  report(summary, rec.events);
  process.exit(ok ? 0 : 1);
}

function countTypes(events: WireEvent[]) {
  const out: Record<string, number> = {};
  for (const e of events) out[e.type] = (out[e.type] ?? 0) + 1;
  return out;
}

main().catch(async (e: Error) => {
  // Setup failed outside the scenario loop: still leave no turn parked, still hand over the event dump.
  await cancelOpenTurn(live);
  report(
    {
      ok: false,
      error: e.message,
      stack: e.stack,
      record_call_id: live?.record || null,
      event_types: live ? countTypes(live.rec.events) : {},
    },
    live?.rec.events,
  );
  process.exit(1);
});
