/**
 * Golden transcripts for crates/document-js (tests/golden.rs): the synthetic parity corpus run through a stdio
 * peer — by default the shipped jsdom helper source (`bun src/helper.ts`) — with a fixed request script.
 * Each output line is canonicalised (the `update` bytes of `ev:update` lines dropped, object keys sorted, block
 * ids renamed ID0, ID1, … in order of appearance per input), so the Rust test can replay the same script in
 * deno_core and compare byte-for-byte.
 *
 *   bun run scripts/golden-inproc.ts [--peer "<cmd>"]   → ../../crates/document-js/tests/golden.json
 *
 * Keep `script` in step with `run_script` in crates/document-js/tests/golden.rs.
 */
import type { FileSink } from "bun";
import { join } from "node:path";
import { syntheticCorpus, type Input } from "./parity-corpus.ts";

const pkgDir = join(import.meta.dir, "..");
const peerIdx = process.argv.indexOf("--peer");
const peerCmd = peerIdx !== -1 ? process.argv[peerIdx + 1]!.split(" ") : ["bun", join(pkgDir, "src", "helper.ts")];
const outfile = join(pkgDir, "..", "..", "crates", "document-js", "tests", "golden.json");

/** `Y.encodeStateAsUpdate(new Y.Doc())`. */
const EMPTY_STATE = "AAA=";
const INSERTED = "Inserted **para** with [link](theuth://notes/1)\n\n- x\n  - y";
const REPLACED = "## Replaced `heading`";
const APPENDED = "\n\nAppended paragraph with *emphasis*.\n";

type Line = Record<string, unknown> & { id?: number; ev?: string };

const proc = Bun.spawn(peerCmd, { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
const reader = (proc.stdout as ReadableStream<Uint8Array>).getReader();
const dec = new TextDecoder();
let buf = "";
const lines: string[] = [];

async function send(req: Record<string, unknown>): Promise<string[]> {
  const sink = proc.stdin as FileSink;
  sink.write(`${JSON.stringify(req)}\n`);
  await sink.flush();
  const out: string[] = [];
  for (;;) {
    while (lines.length) {
      const l = lines.shift()!;
      out.push(l);
      const v = JSON.parse(l) as Line;
      if (v.ev === undefined && v.id === req.id) return out;
    }
    const { value, done } = await reader.read();
    if (done) throw new Error("peer closed stdout");
    buf += dec.decode(value, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const l = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (l) lines.push(l);
    }
  }
}

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
    const out = await send({ id: ++id, op, doc: "d", ...extra });
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

const cases = [];
for (const input of syntheticCorpus()) cases.push({ ...input, transcript: await script(input) });
(proc.stdin as FileSink).end();
await proc.exited;
await Bun.write(outfile, `${JSON.stringify({ peer: peerCmd.join(" ").replace(pkgDir, "packages/documents"), cases }, null, 1)}\n`);
console.error(`wrote ${outfile}: ${cases.length} cases`);
