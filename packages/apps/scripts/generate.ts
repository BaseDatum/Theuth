/**
 * Generate `generated/library.json`, `generated/catalog.txt` and `generated/library-hash.txt`
 * from the curated TypeScript library definitions (design 46 §4, decision 7).
 *
 *   OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1 bun run generate
 *
 * The hash covers package pins, every in-tree patch file, component order, the JSON schema
 * (positional argument order = object key order, preserved verbatim) and reactive-prop markers.
 * Rust embeds `library-hash.txt`; the helper and renderer report the same value.
 */
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  CURATED_COMPONENTS,
  LIBRARY_ROOT,
  buildCuratedLibrary,
  promptOptions,
  reactiveProps,
} from "../src/library.ts";
import type { GeneratedLibrary } from "../src/generated-types.ts";

const pkgDir = join(import.meta.dir, "..");
const outDir = join(pkgDir, "generated");

const sha256 = (data: string | Uint8Array) =>
  createHash("sha256").update(data).digest("hex");

const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8")) as {
  dependencies: Record<string, string>;
};
const pins: Record<string, string> = {};
for (const [name, version] of Object.entries(pkg.dependencies))
  if (name.startsWith("@openuidev/")) pins[name] = version;

const patches: Record<string, string> = {};
for (const file of readdirSync(join(pkgDir, "patches")).sort())
  patches[file] = sha256(readFileSync(join(pkgDir, "patches", file)));

const library = buildCuratedLibrary();
const spec = library.toSpec();
const schema = library.toJSONSchema();
const reactive = reactiveProps(library);
const components = Object.keys(library.components);
if (JSON.stringify(components) !== JSON.stringify(CURATED_COMPONENTS))
  throw new Error(
    `library components differ from CURATED_COMPONENTS (order is hashed):\n${components.join(",")}`,
  );

// Closure check: every $ref in the schema must name a curated component (or the shared `ActionExpression`).
const SHARED_DEFS = ["ActionExpression"];
const refs = new Set<string>();
(function walk(node: unknown): void {
  if (Array.isArray(node)) return node.forEach(walk);
  if (!node || typeof node !== "object") return;
  for (const [k, v] of Object.entries(node)) {
    if (k === "$ref" && typeof v === "string")
      refs.add(v.replace(/^#\/\$defs\//, ""));
    else walk(v);
  }
})(schema);
const defs = Object.keys(schema.$defs ?? {});
const missing = [...refs].filter(
  (r) => !components.includes(r) && !SHARED_DEFS.includes(r),
);
if (missing.length)
  throw new Error(
    `schema references non-curated components: ${missing.join(", ")}`,
  );
const extra = defs.filter(
  (d) => !components.includes(d) && !SHARED_DEFS.includes(d),
);
if (extra.length)
  throw new Error(
    `schema $defs contain unexpected entries: ${extra.join(", ")}`,
  );
if (defs.filter((d) => components.includes(d)).length !== components.length)
  throw new Error(
    `schema $defs (${defs.length}) != curated components (${components.length})`,
  );

const hashInput = {
  pins,
  patches,
  root: LIBRARY_ROOT,
  components,
  schema,
  reactive,
};
const hash = sha256(JSON.stringify(hashInput));

const generated: GeneratedLibrary = {
  hash,
  pins,
  patches,
  root: LIBRARY_ROOT,
  components,
  componentGroups: spec.componentGroups ?? [],
  signatures: spec.components,
  reactive,
  schema,
};

// ── catalog: the apps skill's component reference, from upstream's own prompt generation ──
// `library.prompt()` renders signatures, group notes, built-in + library functions and the action steps (custom ones
// included) in the 1.x wording; the skill supplies its own syntax/Query/Mutation guidance, so only those sections are
// kept, followed by the filtered upstream rules and examples (`promptOptions`).
const prompt = library.prompt({ ...promptOptions, toolCalls: true, bindings: true });
const section = (title: string): string => {
  const hit = prompt.split(/\n(?=## )/).find((s) => s.startsWith(`## ${title}`));
  if (!hit) throw new Error(`upstream prompt has no "## ${title}" section`);
  return hit.trimEnd();
};
const lines: string[] = [
  `# theuth apps component catalog (library ${hash.slice(0, 12)}) — root statement: root = ${LIBRARY_ROOT}([...])`,
  "",
  section("Component Signatures"),
  "",
  section("Built-in Functions"),
  "",
  section("Action — Button Behavior"),
  "",
  "## Rules",
  ...promptOptions.additionalRules.map((r) => `- ${r}`),
  "",
  "## Examples",
  "",
  ...promptOptions.examples.flatMap((e) => [e, ""]),
];

mkdirSync(outDir, { recursive: true });
writeFileSync(
  join(outDir, "library.json"),
  `${JSON.stringify(generated, null, 2)}\n`,
);
writeFileSync(join(outDir, "catalog.txt"), `${lines.join("\n")}\n`);
writeFileSync(join(outDir, "library-hash.txt"), `${hash}\n`);
console.error(`generated ${components.length} components, hash ${hash}`);
