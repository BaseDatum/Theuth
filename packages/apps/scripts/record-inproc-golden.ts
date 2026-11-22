/**
 * Record crates/js-runtime/tests/apps_golden.json: every request line of test/inproc-cases.ts and the response line the
 * current TypeScript (`src/inproc.ts` `call`) gives for it. The Rust tests replay it through the committed
 * bundle in deno_core; test/inproc.test.ts fails when it is stale.
 *
 *   bun run scripts/record-inproc-golden.ts
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { call, version } from "../src/inproc.ts";
import { CASES } from "../test/inproc-cases.ts";

const out = join(import.meta.dir, "..", "..", "..", "crates", "js-runtime", "tests", "apps_golden.json");
const cases = CASES.map((c) => ({ name: c.name, request: c.line, response: call(c.line) }));
writeFileSync(out, `${JSON.stringify({ version: version(), cases }, null, 1)}\n`);
console.error(`recorded ${cases.length} cases to ${out}`);
