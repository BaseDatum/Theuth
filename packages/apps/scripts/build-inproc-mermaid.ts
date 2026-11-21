/**
 * Bundle the parse-only mermaid check (`src/inproc-mermaid.ts`) into ONE runtime-neutral classic script for a
 * bare deno_core isolate (no window/document, no node:/bun: modules):
 *
 *   bun run scripts/build-inproc-mermaid.ts               → dist/theuth-mermaid-inproc.js (minified IIFE)
 *   bun run scripts/build-inproc-mermaid.ts --no-minify
 *   bun run scripts/build-inproc-mermaid.ts --no-trim     → keep the render-only layout engines (5.4 MB instead of 2.9 MB)
 *   bun run scripts/build-inproc-mermaid.ts --out <file>
 *
 * `dompurify` is always aliased to the identity shim (src/inproc/dompurify-shim.ts). The build fails when the
 * output names a `node:`/`bun:` module, a `require(` call, jsdom, or React.
 */
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";

const pkgDir = join(import.meta.dir, "..");
const minify = !process.argv.includes("--no-minify");
/** Render-only engines are stubbed (src/inproc/stub.ts) unless `--no-trim`: same parity-corpus answers, half the bytes. */
const trim = !process.argv.includes("--no-trim");
const outIdx = process.argv.indexOf("--out");
const outfile = outIdx !== -1 ? process.argv[outIdx + 1]! : join(pkgDir, "dist", "theuth-mermaid-inproc.js");

/** Render-only: layout engines and hand-drawn rendering the parse path never calls (verified by the parity corpus). */
const TRIMMED = /^(cytoscape|cytoscape-fcose|cytoscape-cose-bilkent|elkjs(\/.*)?|roughjs(\/.*)?|katex(\/.*)?|@upsetjs\/venn\.js)$/;

const alias: Bun.BunPlugin = {
  name: "theuth-inproc-alias",
  setup(build) {
    build.onResolve({ filter: /^dompurify$/ }, () => ({ path: join(pkgDir, "src", "inproc", "dompurify-shim.ts") }));
    if (trim) build.onResolve({ filter: TRIMMED }, () => ({ path: join(pkgDir, "src", "inproc", "stub.ts") }));
  },
};

const options: Bun.BuildConfig = {
  entrypoints: [join(pkgDir, "src", "inproc-mermaid.ts")],
  target: "browser",
  format: "iife",
  define: { "process.env.NODE_ENV": "'production'" },
  plugins: [alias],
};

mkdirSync(dirname(outfile), { recursive: true });
const probe = await Bun.build({ ...options, minify: false });
if (!probe.success) throw new AggregateError(probe.logs, "inproc mermaid bundle failed");
const text = await probe.outputs[0]!.text();
const modules = [...new Set([...text.matchAll(/^\s*\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!))].sort();
for (const forbidden of ["react", "react-dom", "jsdom", "dompurify"]) {
  if (modules.includes(forbidden)) throw new Error(`inproc mermaid bundle must not contain ${forbidden}`);
}
// A bare `require(` call; lodash's guarded `freeModule.require("util")` probe (a member call) is inert here.
const builtins = [...text.matchAll(/["'](node:[a-z_/]+|bun:[a-z_]+)["']|(?<![.\w$])require\(/g)].map((m) => m[0]);
if (builtins.length) throw new Error(`inproc mermaid bundle references runtime built-ins: ${[...new Set(builtins)].join(", ")}`);

const out = await Bun.build({ ...options, minify });
if (!out.success) throw new AggregateError(out.logs, "inproc mermaid bundle failed");
if (out.outputs.length !== 1) throw new Error(`expected one output file, got ${out.outputs.map((o) => o.path).join(", ")}`);
await Bun.write(outfile, out.outputs[0]!);
console.error(`built ${outfile}${trim ? " (trimmed)" : ""}: ${Bun.file(outfile).size} bytes, ${modules.length} packages:\n  ${modules.join("\n  ")}`);
