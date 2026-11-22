/**
 * The former stdio helper's mermaid check (jsdom window on `globalThis`, then `mermaid.parse`), kept ONLY as the
 * reference that `reference.ts` records crates/js-runtime/tests/mermaid_golden.json from. Production runs the DOM-free bundle
 * (`src/inproc-mermaid.ts` in crates/js-runtime); both format errors with `src/mermaid-core.ts`.
 * Only `mermaid.parse` runs: nothing is rendered, no SVG, no user content reaches a DOM.
 */
import { answer, MERMAID_CONFIG, type MermaidAnswer, type MermaidParser } from "../../src/mermaid-core.ts";

let loading: Promise<MermaidParser> | undefined;

function load(): Promise<MermaidParser> {
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
    mermaid.initialize(MERMAID_CONFIG);
    return mermaid;
  })();
  return loading;
}

export async function checkMermaid(source: string): Promise<MermaidAnswer> {
  return answer(await load(), source);
}
