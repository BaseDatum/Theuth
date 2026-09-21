import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import * as Y from "yjs";
import { blocks, importMarkdown, open } from "../src/convert.ts";
import { PROTOCOL_VERSION, type Res } from "../src/protocol.ts";

const entry = join(import.meta.dir, "..", "src", "helper.ts");

async function spawnHelper(stdin: string, ...args: string[]): Promise<{ code: number; stdout: string }> {
  const proc = Bun.spawn(["bun", "run", entry, ...args], { stdin: new TextEncoder().encode(stdin), stdout: "pipe", stderr: "pipe" });
  const stdout = await new Response(proc.stdout).text();
  const code = await proc.exited;
  return { code, stdout };
}

async function run(stdin: string): Promise<{ code: number; lines: Res[] }> {
  const { code, stdout } = await spawnHelper(stdin);
  return { code, lines: stdout.split("\n").filter(Boolean).map((l) => JSON.parse(l) as Res) };
}

const req = (o: object) => `${JSON.stringify(o)}\n`;
const OPEN = req({ id: 1, op: "open", doc: "d", state: "" });

describe("theuth-documents protocol", () => {
  test("--version prints the protocol version", async () => {
    const { code, stdout } = await spawnHelper("", "--version");
    expect(code).toBe(0);
    expect(stdout.trim()).toBe(`theuth-documents ${PROTOCOL_VERSION}`);
  });

  test("EOF with no requests exits 0 and prints nothing", async () => {
    const { code, stdout } = await spawnHelper("");
    expect(code).toBe(0);
    expect(stdout).toBe("");
  });

  test("bad JSON and bad shapes yield error lines, not a crash; later requests still serve", async () => {
    const { code, lines } = await run(`{not json\n${req({ id: 7, op: "nope", doc: "d" })}${req({ id: 8, op: "markdown", doc: "missing" })}${req({ id: 9, op: "open", doc: "d" })}${OPEN}`);
    expect(code).toBe(0);
    expect(lines).toHaveLength(5);
    expect(lines[0]).toMatchObject({ id: 0, ok: false });
    expect(lines[1]).toMatchObject({ id: 7, ok: false, error: 'unknown op "nope"' });
    expect(lines[2]).toMatchObject({ id: 8, ok: false, error: "document missing is not open" });
    expect(lines[3]).toMatchObject({ id: 9, ok: false });
    expect(lines[4]).toEqual({ id: 1, ok: true });
  });

  test("open → import emits exactly one ev:update before the reply; markdown reads back; close forgets", async () => {
    const md = "---\ntype: fact\ntitle: t\n---\nhello\n";
    const { lines } = await run(
      `${OPEN}${req({ id: 2, op: "import", doc: "d", markdown: md, mode: "replace", frontmatter: true })}${req({ id: 3, op: "markdown", doc: "d" })}${req({ id: 4, op: "outline", doc: "d" })}${req({ id: 5, op: "close", doc: "d" })}${req({ id: 6, op: "markdown", doc: "d" })}`,
    );
    expect(lines[0]).toEqual({ id: 1, ok: true });
    const ev = lines[1]!;
    expect(ev).toMatchObject({ ev: "update", doc: "d", req: 2, changed: 1, total: 1 });
    if (!("ev" in ev)) throw new Error("unreachable");
    expect(ev.touched).toHaveLength(2);
    expect(Buffer.from(ev.update, "base64").length).toBeGreaterThan(0);
    expect(lines[2]).toMatchObject({ id: 2, ok: true, result: { changed: 1, total: 1 } });
    expect(lines[3]).toMatchObject({ id: 3, ok: true, result: { markdown: "---\ntype: fact\ntitle: t\n---\nhello\n" } });
    const outline = lines[4]!;
    expect(outline).toMatchObject({ id: 4, ok: true });
    if (!("result" in outline) || !outline.result) throw new Error("unreachable");
    expect((outline.result as { outline: string }).outline).toMatch(/^\[fm\] type=fact title="t"\n[0-9a-f-]{36} hello$/);
    expect(lines[5]).toEqual({ id: 5, ok: true });
    expect(lines[6]).toMatchObject({ id: 6, ok: false });
  });

  test("an authority update is applied silently (no ev:update echo)", async () => {
    // Build a state in one process, feed it as `update` to another.
    const first = await run(`${OPEN}${req({ id: 2, op: "import", doc: "d", markdown: "x\n", mode: "replace" })}`);
    const ev = first.lines[1]!;
    if (!("ev" in ev)) throw new Error("expected update event");
    const { lines } = await run(`${OPEN}${req({ id: 2, op: "update", doc: "d", update: ev.update })}${req({ id: 3, op: "markdown", doc: "d" })}`);
    expect(lines).toHaveLength(3);
    expect(lines[1]).toEqual({ id: 2, ok: true });
    expect(lines[2]).toMatchObject({ id: 3, ok: true, result: { markdown: "x\n" } });
  });

  test("edit emits one ev:update (touched = inserted ids) before a result carrying the undo; a bad id is a JSON error", async () => {
    const md = "---\ntype: fact\ntitle: t\n---\nhello\n\nworld\n";
    const o = open(new Uint8Array());
    await importMarkdown(o, md, "replace", true);
    const world = blocks(o)[2]!.id;
    const state = Buffer.from(Y.encodeStateAsUpdate(o.ydoc)).toString("base64");
    const { lines } = await run(
      `${req({ id: 1, op: "open", doc: "d", state })}${req({ id: 2, op: "markdown", doc: "d" })}${req({ id: 3, op: "edit", doc: "d", edits: [{ op: "replace", blocks: [world], markdown: "- W\n" }] })}${req({ id: 4, op: "markdown", doc: "d", annotate: true })}${req({ id: 5, op: "edit", doc: "d", edits: [{ op: "delete", blocks: ["nope"] }] })}`,
    );
    expect(lines[1]).toMatchObject({ id: 2, ok: true, result: { markdown: md } });
    const ev = lines[2]!;
    expect(ev).toMatchObject({ ev: "update", doc: "d", req: 3 });
    if (!("ev" in ev)) throw new Error("unreachable");
    expect(ev.touched).toHaveLength(1);
    const res = lines[3]!;
    expect(res).toMatchObject({ id: 3, ok: true, result: { removed: [world], inserted: ev.touched, touched: ev.touched } });
    if (!("result" in res) || !res.result) throw new Error("unreachable");
    expect((res.result as { undo: unknown[] }).undo).toHaveLength(1);
    expect(lines[4]).toMatchObject({ id: 4, ok: true, result: { markdown: expect.stringContaining(`<!-- ${ev.touched[0]} -->\n* W\n`) } });
    const err = lines[5]!;
    expect(err).toMatchObject({ id: 5, ok: false });
    if (!("error" in err)) throw new Error("unreachable");
    expect(JSON.parse(err.error)).toMatchObject({ code: "unknown_block", id: "nope" });
    expect(lines).toHaveLength(6);
  });
});
