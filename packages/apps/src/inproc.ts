/**
 * In-process entry of the headless check/merge validator (design 46 §2.1; apps-inproc-as-built-P1.md).
 *
 * Bundled by `scripts/build-inproc.ts` into one runtime-neutral classic script that `t` runs in a bare
 * deno_core isolate (crates/app-js). Same wire as the former stdio helper (`protocol.ts`), one request line
 * in, one response line out:
 *
 *   globalThis.theuthApps.call(line)  → JSON line (`ParserResponse` or `ParserErrorResponse`)
 *   globalThis.theuthApps.version()   → "theuth-app-parser <library_hash>"
 *
 * Synchronous: `check`/`mergeCheck` never await. Imports `check.ts` only (lang-core + generated JSON): no
 * React, no DOM, no mermaid. `check_mermaid` is not served here (the Rust side owns that seam).
 */
import { check, libraryHash, mergeCheck } from "./check.ts";
import type {
  ParserErrorResponse,
  ParserRequest,
  ParserResponse,
  ToolSnapshot,
} from "./protocol.ts";

/** Largest request line accepted (source + base + tool schemas); longer lines are rejected without being parsed. */
export const MAX_LINE_BYTES = 4 * 1024 * 1024;

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

function parseRequest(line: string): ParserRequest | ParserErrorResponse {
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
  if (op === "check_mermaid")
    return { id, error: "check_mermaid is not served by the in-process OpenUI validator" };
  if (op !== "check" && op !== "merge_check")
    return { id, error: 'op must be "check" or "merge_check"' };
  const source = "source" in raw ? raw.source : undefined;
  if (typeof source !== "string")
    return { id, error: "missing string field source" };
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

/** UTF-8 byte length without materialising the encoding for short lines (UTF-16 length ≤ UTF-8 length). */
function overCap(line: string): boolean {
  if (line.length > MAX_LINE_BYTES) return true;
  if (line.length * 3 <= MAX_LINE_BYTES) return false;
  return new TextEncoder().encode(line).byteLength > MAX_LINE_BYTES;
}

/** One request → its response object. */
export function handle(line: string): ParserResponse | ParserErrorResponse {
  if (overCap(line))
    return { id: null, error: `request line exceeds ${MAX_LINE_BYTES} bytes` };
  const req = parseRequest(line.trim());
  if ("error" in req) return req;
  const result =
    req.op === "check"
      ? check(req.source, req.tools)
      : mergeCheck(req.base!, req.source, req.tools);
  return { id: req.id, ...result };
}

/** One request line → one response line (no trailing newline). */
export function call(line: string): string {
  return JSON.stringify(handle(line));
}

export function version(): string {
  return `theuth-app-parser ${libraryHash}`;
}

Object.assign(globalThis, { theuthApps: { call, version } });
