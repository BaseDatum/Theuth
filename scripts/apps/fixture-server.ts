/**
 * Local fixtures for the apps acceptance drivers (design 46). Two modes, one file, no
 * shared abstraction between them. Nothing here reaches a real service.
 *
 * ## `--mode openai` — scripted OpenAI-compatible model
 *
 *   bun /Users/Sky.Moore/Develop/wth-apps/scripts/apps/fixture-server.ts --mode openai --port 8765 [--chunk 200] [--delay-ms 60]
 *
 * Register in wth with `add_provider("fixture", "http://127.0.0.1:8765/v1", "fixture-key")`,
 * then `set_model(session, "fixture/app-fixture")`.
 *
 *   GET  /v1/models                → {data:[{id:"app-fixture"}]}
 *   POST /v1/chat/completions      → SSE stream (`stream:true`) or a plain completion.
 *        A scripted step is consumed only when the request carries tool definitions and its
 *        last message is a user message (a real agent round). Tool-result rounds, title
 *        requests and an empty script get `{text: default_text}` with finish_reason "stop".
 *        Tool-call arguments stream progressively: `--chunk` bytes every `--delay-ms`, so the
 *        agent publishes ToolCallProgress (≥512 new bytes and ≥250 ms) before the call ends.
 *   POST /fixture/script           → {steps: Step[], default_text?: string} replaces the queue.
 *        Step = {text?: string, tool_calls?: {id?: string, name: string, args: unknown}[]}
 *   GET  /fixture/script           → remaining steps.
 *   GET  /fixture/requests         → recorded chat requests, newest last (cap 50).
 *   DELETE /fixture/requests       → clear the record.
 *   GET  /fixture/health           → {ok:true, mode:"openai"}
 *
 * ## `--mode mcp` — ordinary MCP stdio server with a durable counter
 *
 *   bun /Users/Sky.Moore/Develop/wth-apps/scripts/apps/fixture-server.ts --mode mcp --state /abs/path/argo-state.json
 *
 * Register through the real RPC:
 *   add_mcp_server("argo", {command:"bun", args:[<this file>, "--mode", "mcp", "--state", <state path>]})
 * Risk comes from MCP annotations (crates/tools/src/mcp.rs `risk_of`): readOnlyHint → ReadOnly,
 * destructiveHint → Destructive, else Mutating.
 *
 *   argo_app_health {name}        ReadOnly    → {status, synced, count}
 *   argo_sync       {name}        Mutating    → {message, count}   count[name] += 1 (durable)
 *   argo_destroy    {name}        Destructive → {message, count:0} count[name] = 0
 *   argo_slow       {name, ms?}   ReadOnly    → sleeps ms (default 3000) then like argo_app_health
 *   argo_echo       {secret}      ReadOnly    → {echoed: <input verbatim>, length}. Deliberately raw:
 *                                                the harness expands `{{secret:NAME}}` before dispatch and
 *                                                must scrub the value out of content/structured (37 §3.4).
 *
 * Every result is text (the JSON) plus structuredContent. The state file is the only side
 * effect: {counts: {name: n}, calls: [{tool, name?, at}]} written atomically (tmp + rename);
 * the echo input is never written to it. Read it directly to assert durability.
 */
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
// The MCP SDK is installed once, under packages/apps; no second dependency root for scripts.
import { Server } from "../../packages/apps/node_modules/@modelcontextprotocol/sdk/dist/esm/server/index.js";
import { StdioServerTransport } from "../../packages/apps/node_modules/@modelcontextprotocol/sdk/dist/esm/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "../../packages/apps/node_modules/@modelcontextprotocol/sdk/dist/esm/types.js";
import { parseArgs } from "./rpc.ts";

const args = parseArgs(process.argv.slice(2)).flags;
const mode = args.mode;
if (mode === "openai") openai();
else if (mode === "mcp") await mcp();
else {
  console.error("usage: fixture-server.ts --mode openai --port N | --mode mcp --state FILE");
  process.exit(2);
}

// ---------------------------------------------------------------------------------------------
// openai
// ---------------------------------------------------------------------------------------------

interface ToolCallStep {
  id?: string;
  name: string;
  args: unknown;
}
interface Step {
  text?: string;
  tool_calls?: ToolCallStep[];
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

function openai() {
  const port = Number(args.port ?? 8765);
  const chunkBytes = Number(args.chunk ?? 200);
  const delayMs = Number(args["delay-ms"] ?? 60);
  const MODEL = "app-fixture";
  let steps: Step[] = [];
  let defaultText = "ok";
  const requests: { at: number; model?: string; last_role?: string; tools: string[]; body: ChatRequest }[] = [];
  let calls = 0;

  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

  Bun.serve({
    port,
    hostname: "127.0.0.1",
    idleTimeout: 120,
    async fetch(req) {
      const url = new URL(req.url);
      const path = url.pathname.replace(/\/+$/, "");
      if (req.method === "GET" && path === "/fixture/health") return json({ ok: true, mode: "openai" });
      if (req.method === "GET" && path === "/v1/models")
        return json({ object: "list", data: [{ id: MODEL, object: "model", owned_by: "fixture" }] });
      if (path === "/fixture/script") {
        if (req.method === "POST") {
          const body = (await req.json()) as { steps?: Step[]; default_text?: string };
          steps = body.steps ?? [];
          if (typeof body.default_text === "string") defaultText = body.default_text;
          return json({ ok: true, remaining: steps.length });
        }
        return json({ steps, default_text: defaultText });
      }
      if (path === "/fixture/requests") {
        if (req.method === "DELETE") {
          requests.length = 0;
          return json({ ok: true });
        }
        return json(requests);
      }
      if (req.method === "POST" && path === "/v1/chat/completions") {
        const body = (await req.json()) as ChatRequest;
        const messages = body.messages ?? [];
        const lastRole = messages[messages.length - 1]?.role;
        const toolNames = (body.tools ?? []).map((t) => t.function?.name ?? "?");
        requests.push({ at: Date.now(), model: body.model, last_role: lastRole, tools: toolNames, body });
        if (requests.length > 50) requests.shift();
        const scripted = lastRole === "user" && toolNames.length > 0 ? steps.shift() : undefined;
        const step: Step = scripted ?? { text: defaultText };
        const id = `chatcmpl-fixture-${++calls}`;
        if (body.stream) return sse(id, step);
        return json(completion(id, step));
      }
      return json({ error: { message: `no route ${req.method} ${path}` } }, 404);
    },
  });
  console.log(JSON.stringify({ mode: "openai", url: `http://127.0.0.1:${port}/v1`, model: `fixture/${MODEL}` }));

  const USAGE = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };

  function completion(id: string, step: Step) {
    const toolCalls = (step.tool_calls ?? []).map((tc, i) => ({
      id: tc.id ?? `call_fixture_${calls}_${i}`,
      type: "function",
      function: { name: tc.name, arguments: JSON.stringify(tc.args) },
    }));
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

  function sse(id: string, step: Step) {
    const created = Math.floor(Date.now() / 1000);
    const enc = new TextEncoder();
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
    const sleep = (ms: number) => {
      const { promise, resolve } = Promise.withResolvers<void>();
      setTimeout(resolve, ms);
      return promise;
    };
    const stream = new ReadableStream<Uint8Array>({
      async start(ctrl) {
        try {
          if (step.text) {
            for (let i = 0; i < step.text.length; i += chunkBytes) {
              ctrl.enqueue(frame({ role: "assistant", content: step.text.slice(i, i + chunkBytes) }));
              await sleep(delayMs);
            }
          }
          const toolCalls = step.tool_calls ?? [];
          for (let index = 0; index < toolCalls.length; index++) {
            const tc = toolCalls[index]!;
            const callId = tc.id ?? `call_fixture_${calls}_${index}`;
            const argText = JSON.stringify(tc.args);
            ctrl.enqueue(
              frame({
                tool_calls: [{ index, id: callId, type: "function", function: { name: tc.name, arguments: "" } }],
              }),
            );
            for (let i = 0; i < argText.length; i += chunkBytes) {
              await sleep(delayMs);
              ctrl.enqueue(
                frame({ tool_calls: [{ index, id: callId, function: { arguments: argText.slice(i, i + chunkBytes) } }] }),
              );
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
}

// ---------------------------------------------------------------------------------------------
// mcp
// ---------------------------------------------------------------------------------------------

interface State {
  counts: Record<string, number>;
  calls: { tool: string; name?: string; at: number }[];
}

async function mcp() {
  const statePath =
    typeof args.state === "string" ? args.state : join(tmpdir(), "wth-apps-fixture", "argo-state.json");
  mkdirSync(dirname(statePath), { recursive: true });

  const load = (): State => {
    try {
      return JSON.parse(readFileSync(statePath, "utf8")) as State;
    } catch {
      return { counts: {}, calls: [] };
    }
  };
  const save = (s: State) => {
    if (s.calls.length > 200) s.calls.splice(0, s.calls.length - 200);
    writeFileSync(`${statePath}.tmp`, JSON.stringify(s, null, 2));
    renameSync(`${statePath}.tmp`, statePath);
  };
  const health = (s: State, name: string) => {
    const count = s.counts[name] ?? 0;
    return { status: count > 0 ? "Healthy" : "Degraded", synced: count > 0, count };
  };

  const named = {
    type: "object",
    properties: { name: { type: "string", description: "Application name" } },
    required: ["name"],
    additionalProperties: false,
  };
  const tools = [
    {
      name: "argo_app_health",
      description: "Fixture: health of an application (read-only).",
      inputSchema: named,
      annotations: { readOnlyHint: true },
    },
    {
      name: "argo_sync",
      description: "Fixture: sync an application; increments its durable sync count.",
      inputSchema: named,
      annotations: { readOnlyHint: false, destructiveHint: false },
    },
    {
      name: "argo_destroy",
      description: "Fixture: destroy an application; resets its sync count to zero.",
      inputSchema: named,
      annotations: { readOnlyHint: false, destructiveHint: true },
    },
    {
      name: "argo_slow",
      description: "Fixture: health after sleeping `ms` milliseconds (default 3000).",
      inputSchema: {
        type: "object",
        properties: { name: { type: "string" }, ms: { type: "integer", minimum: 0, maximum: 60000 } },
        required: ["name"],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true },
    },
    {
      name: "argo_echo",
      description: "Fixture: returns its input verbatim so the harness's secret scrubbing is observable.",
      inputSchema: {
        type: "object",
        properties: { secret: { type: "string" } },
        required: ["secret"],
        additionalProperties: false,
      },
      annotations: { readOnlyHint: true },
    },
  ];

  const server = new Server({ name: "wth-apps-fixture", version: "0.0.0" }, { capabilities: { tools: {} } });
  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools }));
  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const tool = req.params.name;
    const a: Record<string, unknown> = req.params.arguments ?? {};
    const appName = typeof a.name === "string" ? a.name : undefined;
    const s = load();
    let out: Record<string, unknown>;
    switch (tool) {
      case "argo_app_health":
        if (!appName) return fail("name required");
        out = health(s, appName);
        break;
      case "argo_sync":
        if (!appName) return fail("name required");
        s.counts[appName] = (s.counts[appName] ?? 0) + 1;
        out = { message: `synced ${appName}`, count: s.counts[appName] };
        break;
      case "argo_destroy":
        if (!appName) return fail("name required");
        s.counts[appName] = 0;
        out = { message: `destroyed ${appName}`, count: 0 };
        break;
      case "argo_slow": {
        if (!appName) return fail("name required");
        const ms = typeof a.ms === "number" ? a.ms : 3000;
        const { promise, resolve } = Promise.withResolvers<void>();
        setTimeout(resolve, ms);
        await promise;
        out = health(load(), appName);
        break;
      }
      case "argo_echo":
        if (typeof a.secret !== "string") return fail("secret required");
        out = { echoed: a.secret, length: a.secret.length };
        break;
      default:
        return fail(`unknown tool ${tool}`);
    }
    s.calls.push({ tool, name: appName, at: Date.now() });
    save(s);
    return { content: [{ type: "text", text: JSON.stringify(out) }], structuredContent: out };
  });
  await server.connect(new StdioServerTransport());

  function fail(message: string) {
    return { content: [{ type: "text", text: message }], isError: true };
  }
}
