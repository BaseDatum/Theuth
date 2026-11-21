/**
 * SPIKE: bundle the in-process documents helper (`src/inproc.ts`) into ONE runtime-neutral classic script
 * for a bare deno_core JsRuntime:
 *
 *   bun run scripts/build-inproc.ts            → dist/theuth-documents-inproc.js (minified IIFE)
 *   bun run scripts/build-inproc.ts --no-minify
 *
 * `@blocknote/server-util` (jsdom + React) is swapped for `src/inproc/editor.ts` (linkedom, no React) at
 * resolve time, so `convert.ts` is the shipped file unchanged. The build fails when the output still names a
 * `node:`/`bun:` module, a `require(` call, or one of the heavy packages the swap is meant to drop.
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const pkgDir = join(import.meta.dir, "..");
const distDir = join(pkgDir, "dist");
const minify = !process.argv.includes("--no-minify");
const outfile = join(distDir, "theuth-documents-inproc.js");

const serverUtilShim: Bun.BunPlugin = {
  name: "theuth:server-util-inproc",
  setup(build) {
    build.onResolve({ filter: /^@blocknote\/server-util$/ }, () => ({ path: join(pkgDir, "src", "inproc", "editor.ts") }));
  },
};

mkdirSync(distDir, { recursive: true });
const probe = await Bun.build({
  entrypoints: [join(pkgDir, "src", "inproc.ts")],
  target: "browser",
  format: "iife",
  minify: false,
  define: { "process.env.NODE_ENV": "'production'" },
  plugins: [serverUtilShim],
});
if (!probe.success) throw new AggregateError(probe.logs, "inproc bundle failed");
const text = await probe.outputs[0]!.text();
const modules = [...new Set([...text.matchAll(/^\s*\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!))].sort();
for (const forbidden of ["jsdom", "react", "react-dom", "@blocknote/react", "@blocknote/server-util", "happy-dom"]) {
  if (modules.includes(forbidden)) throw new Error(`inproc bundle must not contain ${forbidden}`);
}
const builtins = [...text.matchAll(/["'](node:[a-z_/]+|bun:[a-z_]+)["']|\brequire\(/g)].map((m) => m[0]);
if (builtins.length) throw new Error(`inproc bundle still references runtime built-ins: ${[...new Set(builtins)].join(", ")}`);

const out = await Bun.build({
  entrypoints: [join(pkgDir, "src", "inproc.ts")],
  target: "browser",
  format: "iife",
  minify,
  define: { "process.env.NODE_ENV": "'production'" },
  plugins: [serverUtilShim],
});
if (!out.success) throw new AggregateError(out.logs, "inproc bundle failed");
await Bun.write(outfile, out.outputs[0]!);
console.error(`built ${outfile}: ${Bun.file(outfile).size} bytes, ${modules.length} packages:\n  ${modules.join("\n  ")}`);
