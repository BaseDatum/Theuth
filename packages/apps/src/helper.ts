/**
 * `theuth-app-parser`: headless final-validation helper (design 46 §2.1).
 *
 * JSON lines on stdin → one JSON line per request on stdout; logs go to stderr only.
 * Compiles the library schema once at startup. Trusted, harness-owned parser code: it never
 * executes app tools, model code, or the source as JavaScript, and never loads React.
 * Requests are served strictly in order, one at a time; the Rust side owns admission (≤8
 * pending) and the 2 s deadline/kill policy (15 s for the first `check_mermaid`, whose
 * parser loads lazily and is warmed in the background from startup; design 30 §5).
 *
 *   theuth-app-parser --version      → "theuth-app-parser <library_hash>"
 *   printf '' | theuth-app-parser    → exit 0 on EOF
 */
import { check, libraryHash, mergeCheck } from "./check.ts";
import { checkMermaid, warmMermaid } from "./mermaid-check.ts";
import type {
  MermaidRequest,
  MermaidResponse,
  ParserErrorResponse,
  ParserRequest,
  ParserResponse,
  ToolSnapshot,
} from "./protocol.ts";

const argv = process.argv.slice(2);
if (argv.includes("--version") || argv.includes("-V")) {
  process.stdout.write(`theuth-app-parser ${libraryHash}\n`);
  process.exit(0);
}
if (argv.length) {
  process.stderr.write(
    `theuth-app-parser: unknown arguments ${argv.join(" ")} (stdin JSON lines only; --version)\n`,
  );
  process.exit(2);
}

function isToolSnapshot(v: unknown): v is ToolSnapshot {
  return (
    typeof v === "object" &&
    v !== null &&
    "name" in v &&
    typeof v.name === "string" &&
    "risk" in v &&
    typeof v.risk === "string" &&
    "identity" in v &&
    typeof v.identity === "string" &&
    "schema" in v
  );
}

function parseRequest(
  line: string,
): ParserRequest | MermaidRequest | ParserErrorResponse {
  let raw: unknown;
  try {
    raw = JSON.parse(line);
  } catch (e) {
    return {
      id: null,
      error: `invalid JSON: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
  if (typeof raw !== "object" || raw === null)
    return { id: null, error: "request must be a JSON object" };
  const id = "id" in raw && typeof raw.id === "string" ? raw.id : null;
  if (id === null) return { id: null, error: "missing string field id" };
  const op = "op" in raw ? raw.op : undefined;
  if (op !== "check" && op !== "merge_check" && op !== "check_mermaid")
    return {
      id,
      error: 'op must be "check", "merge_check" or "check_mermaid"',
    };
  const source = "source" in raw ? raw.source : undefined;
  if (typeof source !== "string")
    return { id, error: "missing string field source" };
  if (op === "check_mermaid") return { id, op, source };
  const base = "base" in raw ? raw.base : undefined;
  if (op === "merge_check" && typeof base !== "string")
    return { id, error: "merge_check requires string field base" };
  if (base !== undefined && typeof base !== "string")
    return { id, error: "base must be a string" };
  const tools = "tools" in raw ? raw.tools : undefined;
  if (!Array.isArray(tools) || !tools.every(isToolSnapshot))
    return {
      id,
      error: "tools must be an array of {name, schema, risk, identity}",
    };
  return { id, op, source, base, tools };
}

async function handle(
  line: string,
): Promise<ParserResponse | MermaidResponse | ParserErrorResponse> {
  if (new TextEncoder().encode(line).byteLength > MAX_LINE_BYTES) {
    return { id: null, error: `request line exceeds ${MAX_LINE_BYTES} bytes` };
  }
  const req = parseRequest(line);
  if ("error" in req) return req;
  if (req.op === "check_mermaid")
    return { id: req.id, ...(await checkMermaid(req.source)) };
  const result =
    req.op === "check"
      ? check(req.source, req.tools)
      : mergeCheck(req.base!, req.source, req.tools);
  return { id: req.id, ...result };
}

/** Largest request line accepted (source + base + tool schemas); longer lines are rejected without being parsed. */
const MAX_LINE_BYTES = 4 * 1024 * 1024;

warmMermaid();

/** Awaited so the explicit exit below never drops a response the pipe has not taken yet. */
const reply = (r: unknown) => Bun.write(Bun.stdout, `${JSON.stringify(r)}\n`);

const decoder = new TextDecoder();
let pending = "";
let pendingBytes = 0;
let discarding = false;
for await (const chunk of Bun.stdin.stream()) {
  if (discarding) {
    const nl = chunk.indexOf(10);
    if (nl === -1) continue;
    discarding = false;
    pending = decoder.decode(chunk.subarray(nl + 1), { stream: true });
    pendingBytes = chunk.byteLength - nl - 1;
  } else {
    pending += decoder.decode(chunk, { stream: true });
    pendingBytes += chunk.byteLength;
  }
  let nl: number;
  while ((nl = pending.indexOf("\n")) !== -1) {
    const line = pending.slice(0, nl).trim();
    pending = pending.slice(nl + 1);
    pendingBytes = new TextEncoder().encode(pending).byteLength;
    if (!line) continue;
    await reply(await handle(line));
  }
  if (pendingBytes > MAX_LINE_BYTES) {
    await reply({
      id: null,
      error: `request line exceeds ${MAX_LINE_BYTES} bytes`,
    } satisfies ParserErrorResponse);
    pending = "";
    pendingBytes = 0;
    discarding = true;
  }
}
if (!discarding && pending.trim()) await reply(await handle(pending.trim()));
// EOF is the end of service; a still-loading mermaid warm-up must not keep the child alive.
process.exit(0);
