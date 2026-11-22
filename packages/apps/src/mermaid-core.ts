/**
 * The DOM-free half of the mermaid check: run `parse`, map a throw to `{line, message}`. Shared by the
 * in-process bundle (`inproc-mermaid.ts`, crates/app-js) and the jsdom reference that records the parity golden
 * file (`test-inproc/mermaid-corpus/jsdom-check.ts`), so both answer identically for the same parser.
 */
import type { MermaidError } from "./protocol.ts";

export type MermaidParser = { parse(text: string): Promise<unknown> };

export type MermaidAnswer = { ok: true } | { ok: false; errors: MermaidError[] };

/** mermaid's messages read `Parse error on line 4:` (flowchart/sequence) or `... line 4 ...` for some grammars. */
const LINE = /\bline (\d+)/i;
/** jison lists every acceptable token after "Expecting"; the model needs the location and the head of the list. */
const MAX_MESSAGE = 400;

/** mermaid's `initialize` options for a parse-only check. */
export const MERMAID_CONFIG = {
  startOnLoad: false,
  securityLevel: "strict",
  htmlLabels: false,
} as const;

/** Marker property on the error `src/inproc/stub.ts` throws when a parse path reaches a render-only library. */
export const RENDER_ONLY_UNAVAILABLE: unique symbol = Symbol.for("theuth.mermaid.renderOnlyUnavailable");

function isRenderOnlyUnavailable(e: unknown): boolean {
  return typeof e === "object" && e !== null && RENDER_ONLY_UNAVAILABLE in e;
}

/**
 * `{ok:true}`, or `{ok:false}` with mermaid's message and line. A throw from the render-only stub is NOT a
 * verdict on the diagram (our bundle lacks a library), so it is rethrown: the request fails as a protocol
 * error and the reply fence fails open instead of a valid diagram being reported invalid.
 */
export async function answer(parser: MermaidParser, source: string): Promise<MermaidAnswer> {
  try {
    await parser.parse(source);
    return { ok: true };
  } catch (e) {
    if (isRenderOnlyUnavailable(e)) throw e;
    const raw = (e instanceof Error ? e.message : String(e)).trim();
    const message = raw.length > MAX_MESSAGE ? `${raw.slice(0, MAX_MESSAGE)}…` : raw;
    const m = LINE.exec(message);
    const line = m ? Number(m[1]) : null;
    return { ok: false, errors: [{ line, message }] };
  }
}
