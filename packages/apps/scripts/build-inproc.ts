/**
 * Bundle the OpenUI check/merge validator (`src/inproc.ts`) into ONE runtime-neutral classic script for the
 * deno_core runtime embedded in `t` (crates/app-js; apps-inproc-as-built-P1.md):
 *
 *   bun run scripts/build-inproc.ts            → dist/theuth-apps-inproc.js (minified IIFE)
 *   bun run scripts/build-inproc.ts --no-minify
 *
 * `just apps-bundle` runs this and commits the output as crates/app-js/js/theuth-apps-inproc.js.
 * The build fails when the output names a `node:`/`bun:` module, a `require(` call, or a UI/DOM package
 * (React, the OpenUI React packages, zustand, jsdom, mermaid) the runtime cannot and must not host.
 */
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const pkgDir = join(import.meta.dir, "..");
const distDir = join(pkgDir, "dist");
const minify = !process.argv.includes("--no-minify");
const outfile = join(distDir, "theuth-apps-inproc.js");

const hash = readFileSync(join(pkgDir, "generated", "library-hash.txt"), "utf8").trim();
if (!/^[0-9a-f]{64}$/.test(hash))
  throw new Error(`generated/library-hash.txt is not a sha256 hex: "${hash}" — run \`bun run generate\``);

const options: Bun.BuildConfig = {
  entrypoints: [join(pkgDir, "src", "inproc.ts")],
  target: "browser",
  format: "iife",
  define: { "process.env.NODE_ENV": "'production'" },
};

mkdirSync(distDir, { recursive: true });
// Bun emits a `// node_modules/<pkg>/…` banner per bundled module in an unminified build.
const probe = await Bun.build({ ...options, minify: false });
if (!probe.success) throw new AggregateError(probe.logs, "inproc bundle failed");
const text = await probe.outputs[0]!.text();
const modules = [...new Set([...text.matchAll(/^\s*\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!))].sort();
const forbidden = [
  "react",
  "react-dom",
  "@openuidev/react-ui",
  "@openuidev/react-lang",
  "@openuidev/react-headless",
  "zustand",
  "jsdom",
  "mermaid",
  "zod",
];
for (const name of forbidden) {
  if (modules.includes(name))
    throw new Error(`inproc bundle must not contain ${name}; bundled packages: ${modules.join(", ")}`);
}
const local = [...new Set([...text.matchAll(/^\s*\/\/ src\/([^\n]+)/gm)].map((m) => m[1]!))].sort();
for (const name of local) {
  if (!["check.ts", "inproc.ts", "protocol.ts"].includes(name))
    throw new Error(`inproc bundle must only contain check.ts/inproc.ts/protocol.ts from src/, found src/${name}`);
}
// lang-core's opt-in runtime telemetry probes `process.getBuiltinModule?.("node:child_process")` behind
// `OPENUI_RUNTIME_TELEMETRY_ENABLED`; the isolate has no `process`, so that lookup is unreachable (and not an
// import). Every other `node:`/`bun:` string or `require(` fails the build.
const TELEMETRY_PROBE = /getBuiltinModule\?\.\("node:child_process"\)/g;
const scanned = text.replace(TELEMETRY_PROBE, "");
const builtins = [...scanned.matchAll(/["'](node:[a-z_/]+|bun:[a-z_]+)["']|\brequire\(/g)].map((m) => m[0]);
if (builtins.length)
  throw new Error(`inproc bundle still references runtime built-ins: ${[...new Set(builtins)].join(", ")}`);
if (!text.includes(hash)) throw new Error("inproc bundle does not embed generated/library-hash.txt's hash");

const out = await Bun.build({ ...options, minify });
if (!out.success) throw new AggregateError(out.logs, "inproc bundle failed");
await Bun.write(outfile, out.outputs[0]!);
console.error(
  `built ${outfile}: ${Bun.file(outfile).size} bytes, ${modules.length} packages:\n  ${modules.join("\n  ")}\n  src/: ${local.join(", ")}`,
);
