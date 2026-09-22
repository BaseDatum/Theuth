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
 *
 * jsdom resolves two files next to `__dirname` at import: `computed-style.js` reads
 * `browser/default-stylesheet.css` with `fs.readFileSync`, and `XMLHttpRequest-impl.js` does
 * `require.resolve("./xhr-sync-worker.js")` for synchronous XHR (which the helper never issues).
 * Both paths are baked in at compile time and do not exist on the user's machine. The loader below
 * embeds the stylesheet and turns the worker path into a bare name. The probe bundle is then scanned
 * for any remaining `readFileSync`/`__dirname`/`createRequire`/`require.resolve`, and the compiled
 * binary is started from `/tmp` with a foreign `HOME` and no `node_modules` for one import → markdown
 * round trip, so a dependency upgrade that reintroduces a disk read fails the build, not the app.
 */
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import * as Y from "yjs";
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

const jsdomLib = join(dirname(require.resolve("jsdom/package.json")), "lib", "jsdom");
const defaultCss = readFileSync(join(jsdomLib, "browser", "default-stylesheet.css"), "utf8");
/** Source patches keyed by file: every `from` must match, or the build stops. */
const patches: Record<string, { from: RegExp; to: string }> = {
  [join(jsdomLib, "living", "css", "helpers", "computed-style.js")]: {
    from: /fs\.readFileSync\(\s*path\.resolve\(__dirname,\s*"\.\.\/\.\.\/\.\.\/browser\/default-stylesheet\.css"\),\s*\{\s*encoding:\s*"utf-8"\s*\}\s*\)/,
    to: JSON.stringify(defaultCss),
  },
  [join(jsdomLib, "living", "xhr", "XMLHttpRequest-impl.js")]: {
    from: /require\.resolve\("\.\/xhr-sync-worker\.js"\)/,
    to: '"xhr-sync-worker.js"',
  },
};

const jsdomEmbed: Bun.BunPlugin = {
  name: "theuth:jsdom-embed",
  setup(build) {
    build.onLoad({ filter: /[\\/]jsdom[\\/]lib[\\/]jsdom[\\/]living[\\/].*\.js$/ }, async (args) => {
      const patch = patches[args.path];
      if (!patch) return undefined;
      const src = await Bun.file(args.path).text();
      if (!patch.from.test(src)) throw new Error(`${args.path} changed; update theuth:jsdom-embed`);
      return { contents: src.replace(patch.from, patch.to), loader: "js" };
    });
  },
};
const plugins = [cssTreeCjs, jsdomEmbed];

mkdirSync(distDir, { recursive: true });

const probe = await Bun.build({ entrypoints: [entry], target: "bun", minify: false, plugins });
if (!probe.success) throw new AggregateError(probe.logs, "helper bundle failed");
const bundled = await probe.outputs[0]!.text();
const modules = new Set([...bundled.matchAll(/^\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map((m) => m[1]!));
for (const forbidden of ["@blocknote/xl-ai", "y-websocket", "y-webrtc"]) {
  if (modules.has(forbidden)) throw new Error(`helper bundle must not contain ${forbidden}`);
}
const diskReads = [...bundled.matchAll(/^.*(?:readFileSync\(|__dirname|createRequire\(|require\.resolve\().*$/gm)].map((m) => m[0].trim().slice(0, 160));
if (diskReads.length) throw new Error(`helper bundle resolves files at import — the compiled binary would not run standalone:\n${diskReads.join("\n")}`);

const stable = join(distDir, "theuth-documents");
const out = await Bun.build({
  entrypoints: [entry],
  target: "bun",
  minify: true,
  define: { "process.env.NODE_ENV": "'production'" },
  plugins,
  compile: { outfile: stable, ...(target ? { target } : {}) },
});
if (!out.success) throw new AggregateError(out.logs, "helper compile failed");

if (!target || target.startsWith(`bun-${process.platform}-`)) await selfTest(stable);
else console.error(`skipping startup self-test: ${target} does not run here`);

const versioned = `${stable}-${PROTOCOL_VERSION}`;
copyFileSync(stable, versioned);
console.error(`built ${stable} and ${versioned}`);

/** Run the binary as the installed app does: from another directory, another HOME, no node_modules. */
async function selfTest(bin: string) {
  const env = { ...process.env, HOME: "/tmp/x" };
  const version = Bun.spawnSync([bin, "--version"], { cwd: "/tmp", env, stderr: "pipe", stdout: "pipe" });
  const said = version.stdout.toString().trim();
  if (version.exitCode !== 0 || said !== `theuth-documents ${PROTOCOL_VERSION}`) {
    throw new Error(`self-test: --version exited ${version.exitCode}: ${said}\n${version.stderr.toString().slice(0, 2000)}`);
  }

  const state = Buffer.from(Y.encodeStateAsUpdate(new Y.Doc())).toString("base64");
  const requests = [
    { id: 1, op: "open", doc: "t", state },
    { id: 2, op: "import", doc: "t", markdown: "# Self-test\n\nOne paragraph.", mode: "replace" },
    { id: 3, op: "markdown", doc: "t" },
  ];
  const run = Bun.spawnSync([bin], { cwd: "/tmp", env, stdin: Buffer.from(requests.map((r) => `${JSON.stringify(r)}\n`).join("")), stderr: "pipe", stdout: "pipe" });
  const replies = run.stdout
    .toString()
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l) as { id?: number; ok?: boolean; error?: string; result?: { markdown?: string } });
  const failed = replies.find((r) => r.ok === false);
  const md = replies.find((r) => r.id === 3)?.result?.markdown ?? "";
  if (run.exitCode !== 0 || failed || !md.includes("# Self-test") || !md.includes("One paragraph.")) {
    throw new Error(`self-test: round trip failed (exit ${run.exitCode}) ${failed?.error ?? ""}\nstdout: ${run.stdout.toString().slice(0, 2000)}\nstderr: ${run.stderr.toString().slice(0, 2000)}`);
  }
  console.error("self-test passed: --version + import/markdown round trip from /tmp");
}
