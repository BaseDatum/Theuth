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
import { ACTION_STEPS, BUILTIN_SPECS, LIMITS } from "@openuidev/lang-core";
import {
  CURATED_COMPONENTS,
  LIBRARY_ROOT,
  buildCuratedLibrary,
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
const components = [...CURATED_COMPONENTS];

// Closure check: every $ref in the schema must name a curated component.
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
  (r) => !components.includes(r as (typeof components)[number]),
);
if (missing.length)
  throw new Error(
    `schema references non-curated components: ${missing.join(", ")}`,
  );
const extra = defs.filter(
  (d) => !components.includes(d as (typeof components)[number]),
);
if (extra.length)
  throw new Error(
    `schema $defs contain unexpected entries: ${extra.join(", ")}`,
  );
if (defs.length !== components.length)
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
  limits: LIMITS,
};
const hash = sha256(JSON.stringify(hashInput));

const generated: GeneratedLibrary = {
  hash,
  pins,
  patches,
  limits: { ...LIMITS },
  root: LIBRARY_ROOT,
  components,
  componentGroups: spec.componentGroups ?? [],
  signatures: spec.components,
  reactive,
  schema,
};

// ── catalog: compact named signatures for the prompt/skill ──
const lines: string[] = [];
lines.push(
  `# theuth apps component catalog (library ${hash.slice(0, 12)}) — root statement: root = ${LIBRARY_ROOT}([...])`,
);
lines.push(
  "Positional arguments follow each signature; `name?` is optional; `$binding<T>` accepts a $state variable.",
);
lines.push("");
for (const group of generated.componentGroups) {
  lines.push(`## ${group.name}`);
  for (const name of group.components) {
    const sig = spec.components[name];
    if (!sig) continue;
    lines.push(
      sig.description ? `${sig.signature} — ${sig.description}` : sig.signature,
    );
  }
  for (const note of group.notes ?? []) lines.push(note);
  lines.push("");
}
const grouped = new Set(generated.componentGroups.flatMap((g) => g.components));
const ungrouped = components.filter((c) => !grouped.has(c));
if (ungrouped.length) {
  lines.push("## Other");
  for (const name of ungrouped) {
    const sig = spec.components[name]!;
    lines.push(
      sig.description ? `${sig.signature} — ${sig.description}` : sig.signature,
    );
  }
  lines.push("");
}
lines.push("## Data and actions");
lines.push(
  '$name = literal — reactive state; Query("tool", {args}, defaults, refreshSeconds?) — read; Mutation("tool", {args}) — runs only from an action; results expose .status/.data/.error',
);
lines.push(
  "Button(label, Action([steps])) steps: " +
    Object.keys(ACTION_STEPS)
      .map((s) => `@${s}`)
      .join(", ") +
    " — @Run(mutationOrQuery), @Set($var, value), @Reset($var...), @OpenUrl(url), @ToAssistant(message, context?)",
);
lines.push("@Each(array, item, template) — repeat a component per item");
for (const b of Object.values(BUILTIN_SPECS))
  lines.push(`@${b.signature} — ${b.description}`);
lines.push("");
lines.push(
  `Limits: ${LIMITS.sourceBytes} source bytes, ${LIMITS.statements} statements, nesting depth ${LIMITS.depth}, ${LIMITS.expandedNodes} rendered nodes.`,
);

mkdirSync(outDir, { recursive: true });
writeFileSync(
  join(outDir, "library.json"),
  `${JSON.stringify(generated, null, 2)}\n`,
);
writeFileSync(join(outDir, "catalog.txt"), `${lines.join("\n")}\n`);
writeFileSync(join(outDir, "library-hash.txt"), `${hash}\n`);
console.error(`generated ${components.length} components, hash ${hash}`);
