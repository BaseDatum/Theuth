import { describe, expect, test } from "bun:test";
import * as Y from "yjs";
import { blocks, importMarkdown, open } from "../src/convert.ts";
import { call, version } from "../src/inproc.ts";
import { PROTOCOL_VERSION, type Res } from "../src/protocol.ts";

/** One request line → the helper's output lines for it, parsed (events first, reply last). */
async function run(req: object | string): Promise<Res[]> {
  const out = await call(typeof req === "string" ? req : JSON.stringify(req));
  return out.split("\n").map((l) => JSON.parse(l) as Res);
}

/** A document name no other test uses (the module keeps its open documents across calls). */
let n = 0;
const fresh = () => `d${++n}`;

describe("theuth-documents protocol (in-process entry)", () => {
  test("the version names the protocol", () => {
    expect(version).toBe(`theuth-documents ${PROTOCOL_VERSION}`);
    expect((globalThis as { theuthDocuments?: { version: string } }).theuthDocuments?.version).toBe(version);
  });

  test("a request that changes nothing answers exactly one reply line", async () => {
    const doc = fresh();
    expect(await run({ id: 1, op: "open", doc, state: "" })).toEqual([{ id: 1, ok: true }]);
    expect(await run({ id: 2, op: "close", doc })).toEqual([{ id: 2, ok: true }]);
  });

  test("bad JSON and bad shapes yield error replies, not a crash; later requests still serve", async () => {
    const doc = fresh();
    expect(await run("{not json")).toMatchObject([{ id: 0, ok: false }]);
    expect(await run({ id: 7, op: "nope", doc })).toMatchObject([{ id: 7, ok: false, error: 'unknown op "nope"' }]);
    expect(await run({ id: 8, op: "markdown", doc: "missing" })).toMatchObject([{ id: 8, ok: false, error: "document missing is not open" }]);
    expect(await run({ id: 9, op: "open", doc })).toMatchObject([{ id: 9, ok: false }]);
    expect(await run({ id: 1, op: "open", doc, state: "" })).toEqual([{ id: 1, ok: true }]);
  });

  test("open → import emits exactly one ev:update before the reply; markdown reads back; close forgets", async () => {
    const doc = fresh();
    const md = "---\ntype: fact\ntitle: t\n---\nhello\n";
    await run({ id: 1, op: "open", doc, state: "" });
    const imported = await run({ id: 2, op: "import", doc, markdown: md, mode: "replace", frontmatter: true });
    expect(imported).toHaveLength(2);
    const ev = imported[0]!;
    expect(ev).toMatchObject({ ev: "update", doc, req: 2, changed: 1, total: 1 });
    if (!("ev" in ev)) throw new Error("unreachable");
    expect(ev.touched).toHaveLength(2);
    expect(Buffer.from(ev.update, "base64").length).toBeGreaterThan(0);
    expect(imported[1]).toMatchObject({ id: 2, ok: true, result: { changed: 1, total: 1 } });
    expect(await run({ id: 3, op: "markdown", doc })).toMatchObject([{ id: 3, ok: true, result: { markdown: md } }]);
    const [outline] = await run({ id: 4, op: "outline", doc });
    if (!outline || !("result" in outline) || !outline.result) throw new Error("unreachable");
    expect((outline.result as { outline: string }).outline).toMatch(/^\[fm\] type=fact title="t"\n[0-9a-f-]{36} hello$/);
    expect(await run({ id: 5, op: "close", doc })).toEqual([{ id: 5, ok: true }]);
    expect(await run({ id: 6, op: "markdown", doc })).toMatchObject([{ id: 6, ok: false }]);
  });

  test("an authority update is applied silently (no ev:update echo)", async () => {
    const a = fresh();
    await run({ id: 1, op: "open", doc: a, state: "" });
    const [ev] = await run({ id: 2, op: "import", doc: a, markdown: "x\n", mode: "replace" });
    if (!ev || !("ev" in ev)) throw new Error("expected update event");
    const b = fresh();
    await run({ id: 1, op: "open", doc: b, state: "" });
    expect(await run({ id: 2, op: "update", doc: b, update: ev.update })).toEqual([{ id: 2, ok: true }]);
    expect(await run({ id: 3, op: "markdown", doc: b })).toMatchObject([{ id: 3, ok: true, result: { markdown: "x\n" } }]);
  });

  test("edit emits one ev:update (touched = inserted ids) before a result carrying the undo; a bad id is a JSON error", async () => {
    const md = "---\ntype: fact\ntitle: t\n---\nhello\n\nworld\n";
    const o = open(new Uint8Array());
    await importMarkdown(o, md, "replace", true);
    const world = blocks(o)[2]!.id;
    const state = Buffer.from(Y.encodeStateAsUpdate(o.ydoc)).toString("base64");
    const doc = fresh();
    await run({ id: 1, op: "open", doc, state });
    expect(await run({ id: 2, op: "markdown", doc })).toMatchObject([{ id: 2, ok: true, result: { markdown: md } }]);
    const edited = await run({ id: 3, op: "edit", doc, edits: [{ op: "replace", blocks: [world], markdown: "- W\n" }] });
    expect(edited).toHaveLength(2);
    const ev = edited[0]!;
    expect(ev).toMatchObject({ ev: "update", doc, req: 3 });
    if (!("ev" in ev)) throw new Error("unreachable");
    expect(ev.touched).toHaveLength(1);
    const res = edited[1]!;
    expect(res).toMatchObject({ id: 3, ok: true, result: { removed: [world], inserted: ev.touched, touched: ev.touched } });
    if (!("result" in res) || !res.result) throw new Error("unreachable");
    expect((res.result as { undo: unknown[] }).undo).toHaveLength(1);
    expect(await run({ id: 4, op: "markdown", doc, annotate: true })).toMatchObject([
      { id: 4, ok: true, result: { markdown: expect.stringContaining(`<!-- ${ev.touched[0]} -->\n* W\n`) } },
    ]);
    const [err] = await run({ id: 5, op: "edit", doc, edits: [{ op: "delete", blocks: ["nope"] }] });
    expect(err).toMatchObject({ id: 5, ok: false });
    if (!err || !("error" in err)) throw new Error("unreachable");
    expect(JSON.parse(err.error)).toMatchObject({ code: "unknown_block", id: "nope" });
  });
});
