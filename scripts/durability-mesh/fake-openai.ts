// 139 Z: the two-engine smoke's model. An OpenAI-compatible SSE server the
// engine reaches as the gateway `fake` (design 85 `add_provider`). The 109
// `BatchFake` rule, driven by the prompt instead of compiled in: a user
// message `SMOKE {"calls":[{"name":..,"args":{..}}]}` is answered with
// exactly those tool calls (ids `smoke_<i>_<name>`); a request that already
// carries a tool result (or no tools) gets the final text. Every request body
// is appended to the log file as one JSON line so a row can count re-issues.
//
//   bun fake-openai.ts <port> <log-file>

const [port, logFile] = Bun.argv.slice(2);
const USAGE = { prompt_tokens: 37, completion_tokens: 7, total_tokens: 44 };
export const FINAL = "done.";

type Msg = { role?: string; content?: unknown };
type Call = { name: string; args?: unknown };

const chunk = (choices: unknown, usage?: unknown) =>
  `data: ${JSON.stringify({ id: "c", object: "chat.completion.chunk", model: "m", choices, ...(usage ? { usage } : {}) })}\n\n`;

function textOf(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((p: unknown) => (p && typeof p === "object" && "text" in p && typeof p.text === "string" ? p.text : ""))
      .join("");
  }
  return "";
}

function sse(body: string) {
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
}

function finalText(text: string) {
  return sse(
    chunk([{ index: 0, delta: { role: "assistant", content: text }, finish_reason: null }]) +
      chunk([{ index: 0, delta: {}, finish_reason: "stop" }]) +
      chunk([], USAGE) +
      "data: [DONE]\n\n",
  );
}

function toolCalls(calls: Call[]) {
  let body = "";
  calls.forEach((c, i) => {
    body += chunk([
      {
        index: 0,
        delta: {
          role: "assistant",
          tool_calls: [{ index: i, id: `smoke_${i}_${c.name}`, type: "function", function: { name: c.name, arguments: JSON.stringify(c.args ?? {}) } }],
        },
        finish_reason: null,
      },
    ]);
  });
  body += chunk([{ index: 0, delta: {}, finish_reason: "tool_calls" }]) + chunk([], USAGE) + "data: [DONE]\n\n";
  return sse(body);
}

function callsOf(spec: unknown): Call[] | null {
  if (!spec || typeof spec !== "object" || !("calls" in spec) || !Array.isArray(spec.calls)) return null;
  const calls = spec.calls.filter((c: unknown): c is Call => !!c && typeof c === "object" && "name" in c && typeof c.name === "string");
  return calls.length ? calls : null;
}

const log = Bun.file(logFile).writer();

Bun.serve({
  port: Number(port),
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.endsWith("/models")) {
      return Response.json({ object: "list", data: [{ id: "batch", object: "model", owned_by: "smoke" }] });
    }
    if (!url.pathname.endsWith("/chat/completions")) return new Response("not found", { status: 404 });
    const body: unknown = await req.json();
    log.write(JSON.stringify(body) + "\n");
    log.flush();
    const b = body && typeof body === "object" ? (body as { messages?: unknown; tools?: unknown }) : {};
    const msgs: Msg[] = Array.isArray(b.messages) ? (b.messages as Msg[]) : [];
    // Only this turn counts: a tool result *after* the last user message means the batch ran.
    const lastUserAt = msgs.map((m) => m.role).lastIndexOf("user");
    const answered = msgs.slice(lastUserAt + 1).some((m) => m.role === "tool");
    const hasTools = Array.isArray(b.tools) && b.tools.length > 0;
    const lastUser = lastUserAt >= 0 ? msgs[lastUserAt] : undefined;
    const text = textOf(lastUser?.content).trim();
    const marker = text.indexOf("SMOKE ");
    if (!answered && hasTools && marker >= 0) {
      try {
        const calls = callsOf(JSON.parse(text.slice(marker + 6)));
        if (calls) return toolCalls(calls);
      } catch (e) {
        return finalText(`bad SMOKE spec: ${e}`);
      }
    }
    return finalText(answered ? FINAL : "smoke");
  },
});
console.log(`fake-openai on 127.0.0.1:${port}`);
