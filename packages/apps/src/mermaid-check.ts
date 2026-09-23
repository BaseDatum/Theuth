/**
 * Headless mermaid syntax check for reply fences (design 30 §5). Helper-only: mermaid wants a
 * DOM at import time, so a JSDOM window is installed on `globalThis` and mermaid is imported
 * lazily on first use (seconds under load) — `warmMermaid()` starts that at helper startup.
 * Only `mermaid.parse` runs: nothing is rendered, no SVG, no user content reaches a DOM.
 */
import type { MermaidError } from "./protocol.ts";

type Parser = { parse(text: string): Promise<unknown> };

let loading: Promise<Parser> | undefined;

function load(): Promise<Parser> {
  loading ??= (async () => {
    // Dynamic on purpose: jsdom is a multi-second load from source and `--version` must
    // answer before the Rust side's spawn deadline; nothing needs it until the first check.
    const { JSDOM } = await import("jsdom");
    const dom = new JSDOM("<!DOCTYPE html><body></body>");
    const g = globalThis as Record<string, unknown>;
    g.window ??= dom.window;
    g.document ??= dom.window.document;
    // Dynamic on purpose: mermaid reads `window`/`document` while its module evaluates, so a
    // static import would be hoisted ahead of the JSDOM setup above and throw.
    const mermaid = (await import("mermaid")).default;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      htmlLabels: false,
    });
    return mermaid;
  })();
  return loading;
}

/** Start loading in the background; errors surface on the first real check instead. */
export function warmMermaid(): void {
  load().catch(() => {});
}

/** mermaid's messages read `Parse error on line 4:` (flowchart/sequence) or `... line 4 ...` for some grammars. */
const LINE = /\bline (\d+)/i;
/** jison lists every acceptable token after "Expecting"; the model needs the location and the head of the list. */
const MAX_MESSAGE = 400;

export async function checkMermaid(
  source: string,
): Promise<{ ok: true } | { ok: false; errors: MermaidError[] }> {
  const mermaid = await load();
  try {
    await mermaid.parse(source);
    return { ok: true };
  } catch (e) {
    const raw = (e instanceof Error ? e.message : String(e)).trim();
    const message = raw.length > MAX_MESSAGE ? `${raw.slice(0, MAX_MESSAGE)}…` : raw;
    const m = LINE.exec(message);
    const line = m ? Number(m[1]) : null;
    return { ok: false, errors: [{ line, message }] };
  }
}
