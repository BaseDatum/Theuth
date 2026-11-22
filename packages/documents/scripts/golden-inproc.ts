/**
 * Golden transcripts for crates/document-js (tests/golden.rs): the synthetic parity corpus run through the
 * documents helper (`src/inproc.ts`, in this process) with a fixed request script. Each output line is
 * canonicalised (the `update` bytes of `ev:update` lines dropped, object keys sorted, block ids renamed
 * ID0, ID1, … in order of appearance per input).
 *
 *   bun run scripts/golden-inproc.ts            verify: replay and compare with tests/golden.json (exit 1 on a diff)
 *   bun run scripts/golden-inproc.ts --write    re-record tests/golden.json from the current helper
 *
 * The committed file was recorded from the earlier standalone helper and is the regression baseline:
 * re-record only for an intended output change. Keep `script` in step with `run_script` in
 * crates/document-js/tests/golden.rs.
 */
import { join } from "node:path";
import { call } from "../src/inproc.ts";
import { syntheticCorpus, type Input } from "./parity-corpus.ts";

const goldenFile = join(import.meta.dir, "..", "..", "..", "crates", "document-js", "tests", "golden.json");
const write = process.argv.includes("--write");

/** `Y.encodeStateAsUpdate(new Y.Doc())`. */
const EMPTY_STATE = "AAA=";
const INSERTED = "Inserted **para** with [link](theuth://notes/1)\n\n- x\n  - y";
const REPLACED = "## Replaced `heading`";
const APPENDED = "\n\nAppended paragraph with *emphasis*.\n";

type Line = Record<string, unknown> & { id?: number; ev?: string };
type Case = Input & { transcript: string[] };

function canon(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canon).join(",")}]`;
  if (v !== null && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canon(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;

type OutlineNode = { id: string; type?: string; children: OutlineNode[] };

async function script(input: Input): Promise<string[]> {
  const ids = new Map<string, string>();
  const transcript: string[] = [];
  let id = 0;
  const r = async (op: string, extra: Record<string, unknown> = {}): Promise<Line> => {
    const out = (await call(JSON.stringify({ id: ++id, op, doc: "d", ...extra }))).split("\n");
    for (const l of out) {
      const v = JSON.parse(l) as Line;
      if (v.ev !== undefined) delete v.update;
      transcript.push(
        canon(v).replace(UUID, (u) => {
          let n = ids.get(u);
          if (!n) ids.set(u, (n = `ID${ids.size}`));
          return n;
        }),
      );
    }
    return JSON.parse(out[out.length - 1]!) as Line;
  };
  await r("open", { state: EMPTY_STATE });
  await r("import", { markdown: input.text, mode: "replace", frontmatter: input.frontmatter });
  await r("markdown");
  const ol = (await r("outline")).result as { blocks: OutlineNode[] };
  const body = ol.blocks.filter((b) => b.type !== "frontmatter");
  await r("markdown", { annotate: true, authors: body[0] ? { [body[0].id]: "Ada" } : {} });
  const edits: Record<string, unknown>[] = [{ op: "insert", markdown: INSERTED, ...(body[0] ? { after: body[0].id } : {}) }];
  if (body.length >= 3) {
    edits.push({ op: "replace", blocks: [body[1]!.id], markdown: REPLACED });
    edits.push({ op: "move", blocks: [body[body.length - 1]!.id], at: "start" });
  }
  await r("edit", { edits });
  await r("markdown");
  await r("import", { markdown: `${input.text}${APPENDED}`, mode: "diff", frontmatter: input.frontmatter });
  await r("markdown");
  await r("close");
  return transcript;
}

const cases: Case[] = [];
for (const input of syntheticCorpus()) cases.push({ ...input, transcript: await script(input) });

if (write) {
  await Bun.write(goldenFile, `${JSON.stringify({ peer: "packages/documents/src/inproc.ts", cases }, null, 1)}\n`);
  console.error(`wrote ${goldenFile}: ${cases.length} cases`);
} else {
  const golden = (await Bun.file(goldenFile).json()) as { cases: Case[] };
  const differ = cases.filter((c, i) => JSON.stringify(c.transcript) !== JSON.stringify(golden.cases[i]?.transcript));
  for (const c of differ) console.error(`differs: ${c.name}`);
  console.error(`${cases.length} cases, ${cases.length - differ.length} match ${goldenFile}`);
  if (differ.length || cases.length !== golden.cases.length) process.exit(1);
}
