/**
 * The `check_mermaid` op for an in-process runtime (bare deno_core isolate; no window, no document):
 * `scripts/build-inproc-mermaid.ts` bundles this file into one classic script that installs
 *
 *   globalThis.theuthMermaid.call(line: string) → Promise<string>
 *
 * `line` is a `MermaidRequest` JSON line (protocol.ts); the result is the `MermaidResponse` line, or a
 * `ParserErrorResponse` line for a malformed request. Only `mermaid.parse` runs. mermaid's module body is
 * DOM-free (every DOM touch is `typeof document`-guarded); the one DOM user on the parse path, DOMPurify
 * (`sanitizeText`), is aliased to an identity shim at bundle time (`src/inproc/dompurify-shim.ts`) — nothing
 * is rendered, so nothing needs sanitising.
 */
import mermaid from "mermaid";
import { answer, MERMAID_CONFIG } from "./mermaid-core.ts";
import type { MermaidResponse, ParserErrorResponse } from "./protocol.ts";

mermaid.initialize(MERMAID_CONFIG);

function bad(id: string | null, error: string): string {
  const out: ParserErrorResponse = { id, error };
  return JSON.stringify(out);
}

async function call(line: string): Promise<string> {
  let raw: unknown;
  try {
    raw = JSON.parse(line);
  } catch (e) {
    return bad(null, `invalid JSON: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (typeof raw !== "object" || raw === null) return bad(null, "request must be a JSON object");
  const id = "id" in raw && typeof raw.id === "string" ? raw.id : null;
  if (id === null) return bad(null, "missing string field id");
  if (!("op" in raw) || raw.op !== "check_mermaid") return bad(id, 'op must be "check_mermaid"');
  if (!("source" in raw) || typeof raw.source !== "string") return bad(id, "missing string field source");
  const reply: MermaidResponse = { id, ...(await answer(mermaid, raw.source)) };
  return JSON.stringify(reply);
}

(globalThis as typeof globalThis & { theuthMermaid: unknown }).theuthMermaid = { call, version: "theuth-mermaid 1" };
