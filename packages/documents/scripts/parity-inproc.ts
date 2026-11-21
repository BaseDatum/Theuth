/**
 * SPIKE parity driver: the same request script through two stdio peers — the compiled helper
 * (`dist/theuth-documents`, jsdom + Bun) and the deno_core runtime (`document-js-spike serve`, linkedom + V8) —
 * then compare, per input: every reply line with block ids normalised (first-appearance order per side), the
 * `ev:update` metadata, markdown byte-for-byte, and the block JSON of an authority Y.Doc fed each side's updates.
 *
 *   bun run scripts/parity-inproc.ts <helper-bin> <spike-bin> [--report out.json] [--time]
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import type { FileSink, Subprocess } from "bun";
import { homedir } from "node:os";
import { join } from "node:path";
import * as Y from "yjs";
import { ServerBlockNoteEditor } from "../src/inproc/editor.ts";
import { FRAGMENT, links, schema } from "../src/schema.ts";

const [helperBin, spikeBin] = process.argv.slice(2);
if (!helperBin || !spikeBin) throw new Error("usage: parity-inproc.ts <helper-bin> <spike-bin>");
const reportIdx = process.argv.indexOf("--report");
const reportPath = reportIdx !== -1 ? process.argv[reportIdx + 1] : undefined;
const repo = join(import.meta.dir, "..", "..", "..");

// ─── Corpus ────────────────────────────────────────────────────────────────────────────────────
type Input = { name: string; text: string; frontmatter: boolean };
const corpus: Input[] = [];
const add = (name: string, text: string, frontmatter = text.startsWith("---\n")) => corpus.push({ name, text, frontmatter });

// convert.test.ts round-trip cases (verbatim inputs).
const cases: [string, string][] = [
  ["test:paragraph", "Plain **bold** *italic* ~~strike~~ `code` [link](https://x.y/)."],
  ["test:headings", "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six"],
  ["test:bullet list", "- a\n- b\n  - nested"],
  ["test:numbered list", "1. one\n2. two"],
  ["test:check list", "- [ ] todo\n- [x] done"],
  ["test:code block", "```rust\nfn main() {}\n```"],
  ["test:mermaid", "```mermaid\nflowchart LR\n  A --> B\n```"],
  ["test:json-render", '```json-render\n{"root":"a"}\n```'],
  ["test:table", "| a | b |\n| --- | --- |\n| 1 | 2 |"],
  ["test:quote", "> quoted"],
  ["test:divider", "para\n\n---\n\npara2"],
  ["test:code: link", "See [parse()](code:crates/surface/src/lib.rs#L71-L84) and [x](theuth://notes/1)."],
  ["test:all types", "# T\n\npara\n\n- a\n\n1. b\n\n- [ ] c\n\n```js\nx\n```\n\n| a |\n| - |\n| 1 |\n\n> q\n\n---\n"],
];
for (const [n, t] of cases) add(n, t, false);
add(
  "test:frontmatter header",
  `---\ntype: fact\ntitle: theuth's Rust toolchain lives under rustup, not on PATH\ndescription: "cargo/rustc are at ~/.rustup: homebrew's rustup does not proxy them"\ntags:\n  - rust\n  - toolchain\nscope: /Users/Sky.Moore/Develop/theuth\nsource: harvest\ntimestamp: 2026-09-10T09:41:03Z\nextra_key: something the store never saw\n---\n\nBody para.\n`,
);
add(
  "test:outline golden",
  "---\ntype: project\ntitle: wth apps\ntags:\n  - wth\n  - openui\n---\n## Decisions\n\n- app = artifact\n  - nested child\n\n```rust\npub struct A {}\nfn b() {}\n```\n\n| step | crates | done when |\n| --- | --- | --- |\n| 1 | a | b |\n| 2 | c | d |\n",
);

// Synthetic edge cases.
add("syn:deep nesting", "- l1\n  - l2\n    - l3\n      - l4\n        1. n5\n        2. n5b\n  - l2b\n- [ ] check\n  - [x] nested check\n");
add("syn:marks", "***bold italic*** and **bold `code` inside** and ~~*strike italic*~~ and <u>underline?</u> \\*escaped\\* a_b_c snake_case_word\n\nline one  \nhard break\n");
add("syn:links", "[http](https://example.com/a?b=c&d=e) [rel](./docs/x.md) [code](code:crates/a/src/b.rs#L1-L9) [theuth](theuth://notes/42#blk) [mail](mailto:a@b.c) [bad](javascript:alert(1)) [ftp](ftp://h/f) <https://auto.link/>\n");
add("syn:quarto fence", "```python\n#| label: fig-x\n#| echo: false\nimport numpy as np\nprint(np.pi)\n```\n\nText after.\n");
add("syn:json-render big", '```json-render\n{\n  "root": "card",\n  "elements": {\n    "card": {"type": "Card", "props": {"title": "T"}, "children": ["t"]},\n    "t": {"type": "Text", "props": {"text": "hi <b>"}}\n  }\n}\n```\n');
add("syn:table marks", "| **h1** | `h2` | [h3](https://x.y) |\n| :--- | :---: | ---: |\n| *a* | b \\| pipe | ~~c~~ |\n| | empty | cells |\n");
add("syn:unicode", "# Ünïcödé — “quotes” ‘single’ … 🚀\n\nCJK 漢字かな 한국어 עברית العربية\n\n- • bullet char\n- ◦ another\n");
add("syn:html-ish", "Inline <span>html</span> & entities &amp; &lt;tag&gt; &copy; &#x1F600;\n\n<div>block html</div>\n");
add("syn:frontmatter only", "---\ntitle: empty body\ntags: [a, b]\n---\n");
add("syn:empty", "");
add("syn:code fences mixed", "~~~\ntilde fence\n~~~\n\n```\nno lang\n  indented\n\ttab\n```\n\n    indented code block\n\n```ts title=\"x.ts\"\nconst a = 1;\n```\n");
add("syn:numbered start", "3. three\n4. four\n\n10. ten\n");
add("syn:quote nested", "> level one\n> > level two\n>\n> - list in quote\n");

// Real files: design doc 59 sections, and knowledge documents (read-only).
const design = readFileSync(join(repo, "docs", "design", "59-documents.md"), "utf8");
const sections = design.split(/^(?=## )/m).filter((s) => s.startsWith("## "));
for (const i of [0, 2, 3, 5, 8, 12, 16, 20, 24, 27]) if (sections[i]) add(`59:${sections[i]!.split("\n")[0]!.slice(3, 40)}`, sections[i]!, false);
const kroot = join(homedir(), ".theuth", "knowledge");
const kfiles: string[] = [];
const walk = (d: string) => {
  for (const e of readdirSync(d).sort()) {
    const p = join(d, e);
    if (e.startsWith(".")) continue;
    if (statSync(p).isDirectory()) walk(p);
    else if (e.endsWith(".md") && e !== "index.md" && e !== "log.md") kfiles.push(p);
  }
};
walk(kroot);
const stride = Math.max(1, Math.floor(kfiles.length / 14));
for (let i = 0; i < kfiles.length && corpus.filter((c) => c.name.startsWith("kb:")).length < 14; i += stride) {
  const p = kfiles[i]!;
  add(`kb:${p.slice(kroot.length + 1)}`, readFileSync(p, "utf8"));
}

// ─── Peers ─────────────────────────────────────────────────────────────────────────────────────
type Line = Record<string, unknown> & { id?: number; ev?: string };
class Peer {
  private proc: Subprocess<"pipe", "pipe", "inherit">;
  private buf = "";
  private lines: Line[] = [];
  private waiters: (() => void)[] = [];
  private reader: ReadableStreamDefaultReader<Uint8Array>;
  constructor(readonly label: string, cmd: string[]) {
    this.proc = Bun.spawn(cmd, { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
    this.reader = (this.proc.stdout as ReadableStream<Uint8Array>).getReader();
    void this.pump();
  }
  private async pump() {
    const dec = new TextDecoder();
    for (;;) {
      const { value, done } = await this.reader.read();
      if (done) break;
      this.buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = this.buf.indexOf("\n")) !== -1) {
        const l = this.buf.slice(0, nl);
        this.buf = this.buf.slice(nl + 1);
        if (l.trim()) this.lines.push(JSON.parse(l) as Line);
      }
      this.waiters.splice(0).forEach((w) => w());
    }
  }
  /** Send one request; resolve to every line up to and including its reply. */
  async send(req: Record<string, unknown>): Promise<Line[]> {
    const sink = this.proc.stdin as FileSink;
    sink.write(`${JSON.stringify(req)}\n`);
    await sink.flush();
    for (;;) {
      const i = this.lines.findIndex((l) => l.ev === undefined && l.id === req.id);
      if (i !== -1) return this.lines.splice(0, i + 1);
      const { promise, resolve } = Promise.withResolvers<void>();
      this.waiters.push(resolve);
      await promise;
    }
  }
  async close() {
    (this.proc.stdin as FileSink).end();
    await this.proc.exited;
  }
}

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;
class Ids {
  private map = new Map<string, string>();
  norm(s: string): string {
    return s.replace(UUID, (u) => {
      let v = this.map.get(u);
      if (!v) this.map.set(u, (v = `ID${this.map.size}`));
      return v;
    });
  }
}

const ed = ServerBlockNoteEditor.create({ schema, links });
const empty = Buffer.from(Y.encodeStateAsUpdate(new Y.Doc())).toString("base64");

type Side = { peer: Peer; ids: Ids; authority: Y.Doc; transcript: string[]; markdown: string[]; blockJson: string[] };
type OutlineNode = { id: string; children: OutlineNode[] };

async function step(side: Side, req: Record<string, unknown>): Promise<Line> {
  const lines = await side.peer.send(req);
  for (const l of lines) {
    if (l.ev === "update") {
      Y.applyUpdate(side.authority, Buffer.from(l.update as string, "base64"), "helper");
      const { update: _u, ...meta } = l;
      side.transcript.push(side.ids.norm(JSON.stringify(meta)));
    } else side.transcript.push(side.ids.norm(JSON.stringify(l)));
  }
  const reply = lines[lines.length - 1]!;
  const md = (reply.result as { markdown?: string } | undefined)?.markdown;
  // Annotated markdown carries block ids; everything else is compared byte-for-byte as is.
  if (md !== undefined) side.markdown.push(req.annotate ? side.ids.norm(md) : md);
  if (req.op === "import" || req.op === "edit") {
    const frag = side.authority.getXmlFragment(FRAGMENT);
    side.blockJson.push(side.ids.norm(JSON.stringify(frag.length ? ed.yXmlFragmentToBlocks(frag) : [])));
  }
  return reply;
}

/** The request script; anchors come from each side's own outline so both get structurally equal edits. */
async function script(side: Side, input: Input) {
  const doc = "d";
  let id = 0;
  const r = (op: string, extra: Record<string, unknown> = {}) => step(side, { id: ++id, op, doc, ...extra });
  await r("open", { state: empty });
  await r("import", { markdown: input.text, mode: "replace", frontmatter: input.frontmatter });
  await r("markdown");
  const ol = (await r("outline")).result as { blocks: OutlineNode[] };
  const body = ol.blocks.filter((b) => !("type" in b && (b as { type: string }).type === "frontmatter"));
  await r("markdown", { annotate: true, authors: body[0] ? { [body[0].id]: "Ada" } : {} });
  const edits: Record<string, unknown>[] = [{ op: "insert", markdown: "Inserted **para** with [link](theuth://notes/1)\n\n- x\n  - y", ...(body[0] ? { after: body[0].id } : {}) }];
  if (body.length >= 3) {
    edits.push({ op: "replace", blocks: [body[1]!.id], markdown: "## Replaced `heading`" });
    edits.push({ op: "move", blocks: [body[body.length - 1]!.id], at: "start" });
  }
  if (body.length >= 4) edits.push({ op: "delete", blocks: [body[2]!.id] });
  const er = await r("edit", { edits });
  await r("markdown");
  const undo = (er.result as { undo?: unknown[] } | undefined)?.undo;
  if (undo) {
    await r("edit", { edits: undo });
    await r("markdown");
  }
  await r("import", { markdown: `${input.text}\n\nAppended paragraph with *emphasis*.\n`, mode: "diff", frontmatter: input.frontmatter });
  const ol2 = (await r("outline", { depth: 2 })).result as { blocks: OutlineNode[] };
  const nested = ol2.blocks.flatMap((b) => [b, ...b.children]).slice(1, 3).map((b) => b.id);
  await r("markdown", { blocks: nested });
  // Not hex: no prefix matches any id, so `nearest` is the first ids in outline order on both sides.
  await r("edit", { edits: [{ op: "delete", blocks: ["zz-not-a-block"] }] });
  await r("import", { markdown: "Appended via append mode.", mode: "append", frontmatter: false });
  await r("markdown");
  await r("close");
}

// ─── Run ───────────────────────────────────────────────────────────────────────────────────────
const A = new Peer("helper", [helperBin]);
const B = new Peer("inproc", spikeBin.split(" "));
type Row = { name: string; bytes: number; requests: number; markdownEqual: boolean; transcriptEqual: boolean; blocksEqual: boolean; diffs: string[] };
const rows: Row[] = [];
for (const input of corpus) {
  const mk = (peer: Peer): Side => ({ peer, ids: new Ids(), authority: new Y.Doc(), transcript: [], markdown: [], blockJson: [] });
  const a = mk(A);
  const b = mk(B);
  await script(a, input);
  await script(b, input);
  const diffs: string[] = [];
  const cmp = (what: string, x: string[], y: string[]) => {
    let eq = x.length === y.length;
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      if (x[i] !== y[i]) {
        eq = false;
        if (diffs.length < 6) diffs.push(`${what}[${i}]\n  helper: ${JSON.stringify(x[i])?.slice(0, 600)}\n  inproc: ${JSON.stringify(y[i])?.slice(0, 600)}`);
      }
    }
    return eq;
  };
  const row: Row = {
    name: input.name,
    bytes: Buffer.byteLength(input.text),
    requests: a.transcript.filter((l) => !l.startsWith('{"ev"')).length,
    markdownEqual: cmp("markdown", a.markdown, b.markdown),
    transcriptEqual: cmp("transcript", a.transcript, b.transcript),
    blocksEqual: cmp("blocks", a.blockJson, b.blockJson),
    diffs,
  };
  rows.push(row);
  const ok = row.markdownEqual && row.transcriptEqual && row.blocksEqual;
  console.log(`${ok ? "MATCH   " : "MISMATCH"} ${input.name} (${row.bytes} B, ${row.requests} req)${ok ? "" : `\n${diffs.join("\n")}`}`);
}
await A.close();
await B.close();
const matches = rows.filter((r) => r.markdownEqual && r.transcriptEqual && r.blocksEqual).length;
console.log(`\n${rows.length} inputs, ${matches} match, ${rows.length - matches} mismatch`);
if (reportPath) await Bun.write(reportPath, JSON.stringify(rows, null, 2));
