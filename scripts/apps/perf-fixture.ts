/**
 * Deterministic OpenAI-compatible model for the progressive-UI / 50-session performance probes
 * (design 46). Unlike fixture-server.ts's single shared queue, every request is multiplexed by a
 * marker in the user prompt, so many sessions can stream concurrently without racing each other:
 *
 *   send_input(session, "[[probe:app]] Publish the perf panel.", [])
 *
 * The last user message names the key (`[[probe:<key>]]`); requests without a marker use key
 * `default`. Nothing here reaches a real service; no paid model is ever called.
 *
 *   bun /Users/Sky.Moore/Develop/wth-apps/scripts/apps/perf-fixture.ts --port 18790
 *
 * Register: add_provider("perf", "http://127.0.0.1:18790/v1", "perf-key") → set_model(s, "perf/app-fixture").
 *
 *   GET  /v1/models                    → {data:[{id:"app-fixture"}]}
 *   POST /v1/chat/completions          → SSE (`stream:true`) or a plain completion.
 *        No `tools` in the request (title/summary rounds) → `{text:"ok"}`, never gated.
 *        `tools` present and last message is a user message → next scripted step of the key,
 *        else the key's `default_text` (tool-result rounds, exhausted queue).
 *        Text and tool-call arguments stream `chunk` bytes every `delay_ms` (per key).
 *   POST /fixture/script/:key          → {steps?: Step[], default_text?: string, chunk?: number,
 *                                          delay_ms?: number, gate?: boolean} replaces the key.
 *        Step = {text?: string, tool_calls?: {id?: string, name: string, args: unknown}[]}
 *        `gate:true` holds every stream of the key (after the request is accepted, before the
 *        first byte) until POST /fixture/release/:key. Re-posting the script closes the gate again.
 *   POST /fixture/release/:key         → {released: n} opens the gate; later requests do not wait.
 *   GET  /fixture/held                 → {key: {waiting: n, released: bool, arrived: [ms...]}}
 *   GET  /fixture/requests             → recorded chat requests, newest last (cap 200; bodies omitted)
 *   DELETE /fixture/requests           → clear the record
 *   GET  /fixture/health               → {ok:true, mode:"perf"}
 *   DELETE /fixture/script             → drop every key (gates, queues, defaults)
 */
import { parseArgs } from "./rpc.ts";

interface ToolCallStep {
  id?: string;
  name: string;
  args: unknown;
}
interface Step {
  text?: string;
  tool_calls?: ToolCallStep[];
}
interface Script {
  steps: Step[];
  default_text: string;
  chunk: number;
  delay_ms: number;
  gate: boolean;
  released: boolean;
  waiting: (() => void)[];
  arrived: number[];
}
interface ChatMessage {
  role: string;
  content?: unknown;
}
interface ChatRequest {
  model?: string;
  stream?: boolean;
  messages?: ChatMessage[];
  tools?: { function?: { name?: string } }[];
}

const flags = parseArgs(process.argv.slice(2)).flags;
const port = Number(flags.port ?? 18790);
const MODEL = "app-fixture";
const MARKER = /\[\[probe:([\w.-]+)\]\]/;
const USAGE = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
const scripts = new Map<string, Script>();
const requests: { at: number; key: string; last_role?: string; tools: number; stream?: boolean; gated: boolean }[] = [];
let calls = 0;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function script(key: string): Script {
  let s = scripts.get(key);
  if (!s) {
    s = { steps: [], default_text: "ok", chunk: 200, delay_ms: 60, gate: false, released: false, waiting: [], arrived: [] };
    scripts.set(key, s);
  }
  return s;
}

function textOf(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content))
    return content.map((p) => (p && typeof p === "object" && typeof p.text === "string" ? p.text : "")).join("\n");
  return "";
}

function keyOf(messages: ChatMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!;
    if (m.role !== "user") continue;
    const hit = MARKER.exec(textOf(m.content));
    if (hit) return hit[1]!;
  }
  return "default";
}

/** Resolves once the key's gate is open (immediately when not gated or already released). */
function gate(s: Script): Promise<void> {
  if (!s.gate || s.released) return Promise.resolve();
  s.arrived.push(Date.now());
  return new Promise<void>((resolve) => s.waiting.push(resolve));
}

Bun.serve({
  port,
  hostname: "127.0.0.1",
  idleTimeout: 255,
  async fetch(req) {
    const url = new URL(req.url);
    const path = url.pathname.replace(/\/+$/, "");
    if (req.method === "GET" && path === "/fixture/health") return json({ ok: true, mode: "perf" });
    if (req.method === "GET" && path === "/v1/models")
      return json({ object: "list", data: [{ id: MODEL, object: "model", owned_by: "perf-fixture" }] });

    if (path === "/fixture/requests") {
      if (req.method === "DELETE") requests.length = 0;
      return json(req.method === "DELETE" ? { ok: true } : requests);
    }
    if (path === "/fixture/held") {
      const out: Record<string, unknown> = {};
      for (const [k, s] of scripts) out[k] = { waiting: s.waiting.length, released: s.released, gate: s.gate, arrived: s.arrived };
      return json(out);
    }
    if (req.method === "DELETE" && path === "/fixture/script") {
      for (const s of scripts.values()) for (const w of s.waiting.splice(0)) w();
      scripts.clear();
      return json({ ok: true });
    }
    const scriptKey = /^\/fixture\/script\/([\w.-]+)$/.exec(path)?.[1];
    if (scriptKey) {
      const s = script(scriptKey);
      if (req.method === "POST") {
        const body = (await req.json()) as Partial<Pick<Script, "steps" | "default_text" | "chunk" | "delay_ms" | "gate">>;
        if (body.steps) s.steps = body.steps;
        if (typeof body.default_text === "string") s.default_text = body.default_text;
        if (typeof body.chunk === "number" && body.chunk > 0) s.chunk = body.chunk;
        if (typeof body.delay_ms === "number" && body.delay_ms >= 0) s.delay_ms = body.delay_ms;
        if (typeof body.gate === "boolean") s.gate = body.gate;
        // A fresh script closes the gate: streams hold again until the next release.
        s.released = false;
        s.arrived = [];
        return json({ ok: true, key: scriptKey, remaining: s.steps.length, gate: s.gate, chunk: s.chunk, delay_ms: s.delay_ms });
      }
      const { waiting, ...rest } = s;
      return json({ ...rest, waiting: waiting.length });
    }
    const releaseKey = /^\/fixture\/release\/([\w.-]+)$/.exec(path)?.[1];
    if (releaseKey && req.method === "POST") {
      const s = script(releaseKey);
      s.released = true;
      const n = s.waiting.length;
      for (const w of s.waiting.splice(0)) w();
      return json({ ok: true, key: releaseKey, released: n, at: Date.now() });
    }

    if (req.method === "POST" && path === "/v1/chat/completions") {
      const body = (await req.json()) as ChatRequest;
      const messages = body.messages ?? [];
      const lastRole = messages[messages.length - 1]?.role;
      const tools = body.tools?.length ?? 0;
      const key = keyOf(messages);
      const id = `chatcmpl-perf-${++calls}`;
      // Title/summary rounds carry no tools: answer at once, never hold them at a gate.
      if (tools === 0) {
        requests.push({ at: Date.now(), key, last_role: lastRole, tools, stream: body.stream, gated: false });
        const s: Script = { ...script("default"), chunk: 1000, delay_ms: 0 };
        return body.stream ? sse(id, { text: "ok" }, s) : json(completion(id, { text: "ok" }));
      }
      const s = script(key);
      const step: Step = (lastRole === "user" ? s.steps.shift() : undefined) ?? { text: s.default_text };
      const gated = s.gate && !s.released;
      requests.push({ at: Date.now(), key, last_role: lastRole, tools, stream: body.stream, gated });
      if (requests.length > 200) requests.shift();
      if (body.stream) return sse(id, step, s);
      await gate(s);
      return json(completion(id, step));
    }
    return json({ error: { message: `no route ${req.method} ${path}` } }, 404);
  },
});
console.log(JSON.stringify({ mode: "perf", url: `http://127.0.0.1:${port}/v1`, model: `perf/${MODEL}` }));

function toolCallsOf(step: Step) {
  return (step.tool_calls ?? []).map((tc, i) => ({
    id: tc.id ?? `call_perf_${calls}_${i}`,
    type: "function" as const,
    function: { name: tc.name, arguments: JSON.stringify(tc.args) },
  }));
}

function completion(id: string, step: Step) {
  const toolCalls = toolCallsOf(step);
  return {
    id,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model: MODEL,
    choices: [
      {
        index: 0,
        message: { role: "assistant", content: step.text ?? null, tool_calls: toolCalls.length ? toolCalls : undefined },
        finish_reason: toolCalls.length ? "tool_calls" : "stop",
      },
    ],
    usage: USAGE,
  };
}

function sse(id: string, step: Step, s: Script) {
  const created = Math.floor(Date.now() / 1000);
  const enc = new TextEncoder();
  const { chunk, delay_ms } = s;
  const frame = (delta: unknown, finish: string | null = null, extra: Record<string, unknown> = {}) =>
    enc.encode(
      `data: ${JSON.stringify({
        id,
        object: "chat.completion.chunk",
        created,
        model: MODEL,
        choices: [{ index: 0, delta, finish_reason: finish }],
        ...extra,
      })}\n\n`,
    );
  const stream = new ReadableStream<Uint8Array>({
    async start(ctrl) {
      try {
        // Headers went out already (the agent sees an accepted request); the body waits at the gate.
        await gate(s);
        if (step.text) {
          for (let i = 0; i < step.text.length; i += chunk) {
            ctrl.enqueue(frame({ role: "assistant", content: step.text.slice(i, i + chunk) }));
            if (delay_ms) await sleep(delay_ms);
          }
        }
        const toolCalls = toolCallsOf(step);
        for (let index = 0; index < toolCalls.length; index++) {
          const tc = toolCalls[index]!;
          const argText = tc.function.arguments;
          ctrl.enqueue(frame({ tool_calls: [{ index, id: tc.id, type: "function", function: { name: tc.function.name, arguments: "" } }] }));
          for (let i = 0; i < argText.length; i += chunk) {
            if (delay_ms) await sleep(delay_ms);
            ctrl.enqueue(frame({ tool_calls: [{ index, id: tc.id, function: { arguments: argText.slice(i, i + chunk) } }] }));
          }
        }
        ctrl.enqueue(frame({}, toolCalls.length ? "tool_calls" : "stop", { usage: USAGE }));
        ctrl.enqueue(enc.encode("data: [DONE]\n\n"));
      } finally {
        ctrl.close();
      }
    },
  });
  return new Response(stream, {
    headers: { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" },
  });
}
