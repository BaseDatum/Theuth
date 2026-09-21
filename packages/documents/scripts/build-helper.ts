/**
 * Compile the documents helper into a standalone executable (design 59 §3.2, after 46 §2.1):
 *
 *   bun run build:helper [--target bun-linux-x64]
 *
 * Outputs (packages/documents/dist/):
 *   theuth-documents              stable name for local use
 *   theuth-documents-<version>    immutable name for release manifests (version = PROTOCOL_VERSION)
 *
 * Unlike the app parser this bundle legitimately contains React and jsdom: `@blocknote/server-util`
 * renders block DOM through them. What must never be in it is `@blocknote/xl-ai` (decision 4).
 *
 * `css-tree` (under jsdom) loads its JSON tables through `createRequire(import.meta.url)` in its
 * ESM build, which a compiled binary cannot resolve; its CJS build uses static `require`s that
 * bundle fine, so the resolver is pointed there.
 */
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { PROTOCOL_VERSION } from "../src/protocol.ts";

const pkgDir = join(import.meta.dir, "..");
const distDir = join(pkgDir, "dist");
const entry = join(pkgDir, "src", "helper.ts");
const targetIdx = process.argv.indexOf("--target");
const target = (targetIdx !== -1 ? process.argv[targetIdx + 1] : undefined) as Bun.Build.CompileTarget | undefined;
const require = createRequire(import.meta.url);
const cssTreeDir = dirname(require.resolve("css-tree/package.json"));
const cssTreeExports = (JSON.parse(readFileSync(join(cssTreeDir, "package.json"), "utf8")) as { exports: Record<string, { require?: string } | string> }).exports;

const cssTreeCjs: Bun.BunPlugin = {
  name: "theuth:css-tree-cjs",
  setup(build) {
    build.onResolve({ filter: /^css-tree(\/.*)?$/ }, (args) => {
      const sub = args.path === "css-tree" ? "." : `.${args.path.slice("css-tree".length)}`;
      const target = cssTreeExports[sub];
      const rel = typeof target === "object" ? target.require : undefined;
      if (!rel) throw new Error(`css-tree export ${sub} has no require build`);
      return { path: join(cssTreeDir, rel) };
    });
  },
};

mkdirSync(distDir, { recursive: true });

const probe = await Bun.build({ entrypoints: [entry], target: "bun", minify: false, plugins: [cssTreeCjs] });
if (!probe.success) throw new AggregateError(probe.logs, "helper bundle failed");
const bundled = await probe.outputs[0]!.text();
const modules = new Set([...bundled.matchAll(/^\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!));
for (const forbidden of ["@blocknote/xl-ai", "y-websocket", "y-webrtc"]) {
  if (modules.has(forbidden)) throw new Error(`helper bundle must not contain ${forbidden}`);
}

const stable = join(distDir, "theuth-documents");
const out = await Bun.build({
  entrypoints: [entry],
  target: "bun",
  minify: true,
  define: { "process.env.NODE_ENV": "'production'" },
  plugins: [cssTreeCjs],
  compile: { outfile: stable, ...(target ? { target } : {}) },
});
if (!out.success) throw new AggregateError(out.logs, "helper compile failed");

const versioned = `${stable}-${PROTOCOL_VERSION}`;
copyFileSync(stable, versioned);
console.error(`built ${stable} and ${versioned}`);
