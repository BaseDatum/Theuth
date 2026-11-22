/**
 * Bundle the documents helper (`src/inproc.ts`) into ONE runtime-neutral classic script for the deno_core
 * runtime embedded in `t` (crates/document-js):
 *
 *   bun run scripts/build-inproc.ts            → dist/theuth-documents-inproc.js (minified IIFE)
 *   bun run scripts/build-inproc.ts --no-minify
 *
 * `just documents-bundle` runs this and commits the output as crates/document-js/js/theuth-documents-inproc.js.
 * The build fails when the output names a `node:`/`bun:` module, a `require(` call, or a DOM/UI package the
 * runtime cannot host.
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const pkgDir = join(import.meta.dir, "..");
const distDir = join(pkgDir, "dist");
const minify = !process.argv.includes("--no-minify");
const outfile = join(distDir, "theuth-documents-inproc.js");

const options: Bun.BuildConfig = {
  entrypoints: [join(pkgDir, "src", "inproc.ts")],
  target: "browser",
  format: "iife",
  define: { "process.env.NODE_ENV": "'production'" },
};

mkdirSync(distDir, { recursive: true });
const probe = await Bun.build({ ...options, minify: false });
if (!probe.success) throw new AggregateError(probe.logs, "inproc bundle failed");
const text = await probe.outputs[0]!.text();
const modules = [...new Set([...text.matchAll(/^\s*\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!))].sort();
for (const forbidden of ["react", "react-dom", "@blocknote/react", "happy-dom"]) {
  if (modules.includes(forbidden)) throw new Error(`inproc bundle must not contain ${forbidden}`);
}
const builtins = [...text.matchAll(/["'](node:[a-z_/]+|bun:[a-z_]+)["']|\brequire\(/g)].map((m) => m[0]);
if (builtins.length) throw new Error(`inproc bundle still references runtime built-ins: ${[...new Set(builtins)].join(", ")}`);

const out = await Bun.build({ ...options, minify });
if (!out.success) throw new AggregateError(out.logs, "inproc bundle failed");
await Bun.write(outfile, out.outputs[0]!);
console.error(`built ${outfile}: ${Bun.file(outfile).size} bytes, ${modules.length} packages:\n  ${modules.join("\n  ")}`);
