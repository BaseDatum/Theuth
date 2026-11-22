/**
 * Record the reference answers (jsdom-check.ts: jsdom + mermaid.parse, as the former stdio
 * helper runs it) for the corpus into crates/js-runtime/tests/mermaid_golden.json:
 *
 *   bun run test-inproc/mermaid-corpus/reference.ts
 */
import { join } from "node:path";
import { checkMermaid } from "./jsdom-check.ts";
import { corpus } from "./corpus.ts";
import type { MermaidError } from "../../src/protocol.ts";
const cases = corpus();

type Answer = { ok: true } | { ok: false; errors: MermaidError[] };
const out: Record<string, { type: string; source: string; reply: Answer }> = {};
for (const c of cases) {
  out[c.name] = { type: c.type, source: c.source, reply: await checkMermaid(c.source) };
}
const path = join(import.meta.dir, "..", "..", "..", "..", "crates", "js-runtime", "tests", "mermaid_golden.json");
await Bun.write(path, `${JSON.stringify(out, null, 1)}\n`);
const bad = Object.entries(out).filter(([n, v]) => n.endsWith("/valid") && !v.reply.ok);
console.error(`${cases.length} cases → ${path}`);
for (const [n, v] of bad) console.error(`VALID BUT REJECTED ${n}: ${JSON.stringify(v.reply)}`);
