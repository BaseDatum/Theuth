import { describe, expect, test } from "bun:test";
import * as Y from "yjs";
import { blocks, frontmatterOf, importMarkdown, isOurs, lcs, markdown, open, outline, type Open } from "../src/convert.ts";
import { FRAGMENT } from "../src/schema.ts";

/** A fresh mirror with `text` imported as a knowledge document (frontmatter on). */
async function doc(text: string, frontmatter = true): Promise<Open> {
  const o = open(new Uint8Array());
  await importMarkdown(o, text, "replace", frontmatter);
  return o;
}

const HEADER = `type: fact
title: theuth's Rust toolchain lives under rustup, not on PATH
description: "cargo/rustc are at ~/.rustup: homebrew's rustup does not proxy them"
tags:
  - rust
  - toolchain
scope: /Users/Sky.Moore/Develop/theuth
source: harvest
timestamp: 2026-09-10T09:41:03Z
extra_key: something the store never saw
`;

describe("markdown round trip per block type", () => {
  const cases: [string, string, string | RegExp][] = [
    ["paragraph", "Plain **bold** *italic* ~~strike~~ `code` [link](https://x.y/).", /\*\*bold\*\*.*\*italic\*.*~~strike~~.*`code`.*\[link\]\(https:\/\/x\.y\/\)/],
    ["heading 1", "# One", "# One"],
    ["heading 2", "## Two", "## Two"],
    ["heading 3", "### Three", "### Three"],
    ["heading 4", "#### Four", "#### Four"],
    ["heading 5", "##### Five", "##### Five"],
    ["heading 6", "###### Six", "###### Six"],
    ["bullet list", "- a\n- b\n  - nested", /[*-] a\n[*-] b\n  [*-] nested/],
    ["numbered list", "1. one\n2. two", "1. one\n2. two"],
    ["check list", "- [ ] todo\n- [x] done", /\[ \] todo\n[*-] \[x\] done/],
    ["code block", "```rust\nfn main() {}\n```", "```rust\nfn main() {}\n```"],
    ["mermaid code block keeps its info string", "```mermaid\nflowchart LR\n  A --> B\n```", "```mermaid\nflowchart LR\n  A --> B\n```"],
    ["json-render code block keeps its info string", "```json-render\n{\"root\":\"a\"}\n```", "```json-render\n{\"root\":\"a\"}\n```"],
    ["table", "| a | b |\n| --- | --- |\n| 1 | 2 |", /\| a\s+\| b\s+\|\n\| -+ \| -+ \|\n\| 1\s+\| 2\s+\|/],
    ["quote", "> quoted", "> quoted"],
    ["divider", "para\n\n---\n\npara2", /para\n\n(\*\*\*|---)\n\npara2/],
    // 71 §2.3: a board's prose anchor survives BlockNote's link gate; so does a `theuth:` note citation
    // (schema.ts allowlists the scheme, 144 §6).
    ["code: link", "See [parse()](code:crates/surface/src/lib.rs#L71-L84) and [x](theuth://notes/1).", "See [parse()](code:crates/surface/src/lib.rs#L71-L84) and [x](theuth://notes/1)."],
  ];
  for (const [name, md, expected] of cases) {
    test(name, async () => {
      const o = await doc(md, false);
      const out = (await markdown(o)).trim();
      if (typeof expected === "string") expect(out).toBe(expected);
      else expect(out).toMatch(expected);
    });
  }

  test("a document parses to the schema's block types only", async () => {
    const o = await doc("# T\n\npara\n\n- a\n\n1. b\n\n- [ ] c\n\n```js\nx\n```\n\n| a |\n| - |\n| 1 |\n\n> q\n\n---\n", false);
    const types = blocks(o).map((b) => b.type);
    expect(types).toEqual(["heading", "paragraph", "bulletListItem", "numberedListItem", "checkListItem", "codeBlock", "table", "quote", "divider"]);
  });

  test("a ```mermaid fence imports as codeBlock{language: mermaid}, not the default language", async () => {
    const o = await doc("```mermaid\nflowchart LR\n  A --> B\n```", false);
    const b = blocks(o)[0];
    expect(b?.type).toBe("codeBlock");
    expect(b?.props).toMatchObject({ language: "mermaid" });
  });
});

describe("frontmatter block", () => {
  test("import maps the header onto the first block and export writes it back first, verbatim", async () => {
    const o = await doc(`---\n${HEADER}---\n\nBody para.\n`);
    const bs = blocks(o);
    expect(bs[0]?.type).toBe("frontmatter");
    // The prop holds the header without its trailing newline; export adds it back.
    expect(frontmatterOf(bs)).toBe(HEADER.trimEnd());
    const out = await markdown(o);
    expect(out.startsWith(`---\n${HEADER}---\n`)).toBe(true);
    expect(out).toContain("Body para.");
    // Every Meta key, the tags list and the unknown key survive.
    for (const k of ["type:", "title:", "description:", "tags:", "  - rust", "scope:", "source:", "timestamp:", "extra_key:"]) expect(out).toContain(k);
  });

  test("the exporter tolerates the custom block (the §14 bet): no throw, nothing emitted for it", async () => {
    const o = await doc(`---\ntype: fact\n---\nx\n`);
    const out = await markdown(o, blocks(o).map((b) => b.id));
    expect(out.trim()).toBe("x");
  });

  test("frontmatter: false treats a leading --- block as body, not a header", async () => {
    const o = await doc("---\ntype: fact\n---\nx\n", false);
    expect(frontmatterOf(blocks(o))).toBeNull();
  });

  test("a header-only or empty import still leaves one editable paragraph", async () => {
    const o = await doc("---\ntype: fact\n---\n");
    const bs = blocks(o);
    expect(bs.map((b) => b.type)).toEqual(["frontmatter", "paragraph"]);
  });

  test("diff import updates the header in place, keeping the block id", async () => {
    const o = await doc(`---\ntype: fact\ntitle: old\n---\nx\n`);
    const id = blocks(o)[0]!.id;
    const r = await importMarkdown(o, `---\ntype: fact\ntitle: new\n---\nx\n`, "diff", true);
    expect(blocks(o)[0]!.id).toBe(id);
    expect(frontmatterOf(blocks(o))).toBe("type: fact\ntitle: new");
    expect(r.touched).toEqual([id]);
    expect(r.changed).toBe(0);
  });

  test("a header that differs only in formatting is not a change", async () => {
    const o = await doc(`---\ntype: fact\ntags: [a, b]\n---\nx\n`);
    const r = await importMarkdown(o, `---\ntype: fact\ntags:\n  - a\n  - b\n---\nx\n`, "diff", true);
    expect(r.touched).toEqual([]);
  });
});

const SIX = `---\ntype: fact\n---\n# H\n\nfoo one\n\nfoo two\n\n- bullet\n\n> quote\n\nlast\n`;

describe("diff import", () => {
  test("keeps ids of untouched blocks and replaces only the changed one", async () => {
    const o = await doc(SIX);
    const before = blocks(o).slice(1).map((b) => b.id);
    expect(before).toHaveLength(6);
    const r = await importMarkdown(o, SIX.replace("foo two", "bar two"), "diff", true);
    const after = blocks(o).slice(1).map((b) => b.id);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[2]).not.toBe(before[2]);
    expect(after.slice(3)).toEqual(before.slice(3));
    expect(r).toMatchObject({ changed: 1, total: 6, touched: [after[2]] });
    expect(await markdown(o)).toContain("bar two");
  });

  test("an unchanged file is a no-op: nothing touched, no update emitted", async () => {
    const o = await doc(SIX);
    let updates = 0;
    o.ydoc.on("update", (_u: Uint8Array, origin: unknown) => {
      if (isOurs(origin)) updates++;
    });
    const r = await importMarkdown(o, SIX, "diff", true);
    expect(r).toMatchObject({ changed: 0, total: 6, touched: [] });
    expect(updates).toBe(0);
  });

  test("inserting a block in the middle keeps every neighbour", async () => {
    const o = await doc(SIX);
    const before = blocks(o).slice(1).map((b) => b.id);
    const r = await importMarkdown(o, SIX.replace("foo two\n", "foo two\n\ninserted\n"), "diff", true);
    const after = blocks(o).slice(1);
    expect(after).toHaveLength(7);
    expect(after.map((b) => b.id).filter((id) => before.includes(id))).toHaveLength(6);
    expect(r.changed).toBe(1);
  });

  test("deleting blocks touches nothing new and counts zero changed", async () => {
    const o = await doc(SIX);
    const r = await importMarkdown(o, SIX.replace("foo one\n\n", "").replace("> quote\n\n", ""), "diff", true);
    expect(blocks(o).slice(1)).toHaveLength(4);
    expect(r).toMatchObject({ changed: 0, total: 4, touched: [] });
  });

  test("rewriting more than half the blocks reports it (the snapshot trigger)", async () => {
    const o = await doc(SIX);
    const r = await importMarkdown(o, `---\ntype: fact\n---\nA\n\nB\n\nC\n\nD\n\n- bullet\n\nlast\n`, "diff", true);
    expect(r.total).toBe(6);
    expect(r.changed).toBe(4);
  });

  test("whitespace-only differences do not replace a block", async () => {
    const o = await doc(SIX);
    const r = await importMarkdown(o, SIX.replace("foo   one", "foo one").replace("foo one", "foo    one"), "diff", true);
    expect(r.changed).toBe(0);
  });

  test("one transaction, one update, carrying the touched ids", async () => {
    const o = await doc(SIX);
    const seen: string[][] = [];
    o.ydoc.on("update", (_u: Uint8Array, origin: unknown) => {
      if (isOurs(origin)) seen.push(origin.touched);
    });
    const r = await importMarkdown(o, SIX.replace("foo one", "x").replace("last", "y"), "diff", true);
    expect(seen).toHaveLength(1);
    expect(seen[0]).toEqual(r.touched);
    expect(r.touched).toHaveLength(2);
  });

  test("a doc with no frontmatter block gains one on a frontmatter import", async () => {
    const o = await doc("x\n", false);
    const r = await importMarkdown(o, "---\ntype: fact\n---\nx\n", "diff", true);
    const bs = blocks(o);
    expect(bs[0]?.type).toBe("frontmatter");
    expect(r.touched).toEqual([bs[0]!.id]);
  });
});

describe("append and replace", () => {
  test("append adds after the existing body and leaves it intact", async () => {
    const o = await doc(SIX);
    const before = blocks(o).map((b) => b.id);
    const r = await importMarkdown(o, "tail\n", "append", false);
    const after = blocks(o);
    expect(after.slice(0, before.length).map((b) => b.id)).toEqual(before);
    expect(after).toHaveLength(before.length + 1);
    expect(r).toMatchObject({ changed: 1, total: 1 });
  });

  test("replace discards every body block but keeps the frontmatter id", async () => {
    const o = await doc(SIX);
    const fm = blocks(o)[0]!.id;
    await importMarkdown(o, "---\ntype: fact\n---\nnew\n", "replace", true);
    const bs = blocks(o);
    expect(bs.map((b) => b.type)).toEqual(["frontmatter", "paragraph"]);
    expect(bs[0]!.id).toBe(fm);
  });
});

describe("yjs mirror", () => {
  test("open from a state, apply an authority update, export the merged text", async () => {
    const a = await doc(SIX);
    const state = Y.encodeStateAsUpdate(a.ydoc);
    const b = open(state);
    expect(await markdown(b)).toBe(await markdown(a));
    // The authority accepts a change on its side and forwards the update.
    let upd: Uint8Array | null = null;
    a.ydoc.on("update", (u: Uint8Array) => {
      upd = u;
    });
    await importMarkdown(a, SIX.replace("last", "final"), "diff", true);
    Y.applyUpdate(b.ydoc, upd!, "authority");
    expect(await markdown(b)).toContain("final");
  });

  test("the fragment name is the shared constant", () => {
    const o = open(new Uint8Array());
    expect(o.ydoc.getXmlFragment(FRAGMENT)).toBe(o.fragment);
  });
});

describe("outline", () => {
  test("§5.1 shape: [fm] line, ids first, tags, nesting, code line count", async () => {
    const o = await doc(`---\ntype: project\ntitle: wth apps\ntags:\n  - wth\n  - openui\n---\n## Decisions\n\n- app = artifact\n  - nested child\n\n\`\`\`rust\npub struct A {}\nfn b() {}\n\`\`\`\n\n| step | crates |\n| --- | --- |\n| 1 | document |\n`);
    const { outline: text, blocks: tree } = outline(o);
    const lines = text.split("\n");
    expect(lines[0]).toBe('[fm] type=project title="wth apps" tags=wth,openui');
    expect(lines[1]).toMatch(/^[0-9a-f-]{36} \[h2\] Decisions$/);
    expect(lines[2]).toMatch(/^[0-9a-f-]{36} • app = artifact$/);
    expect(lines[3]).toMatch(/^ {2}[0-9a-f-]{36} • nested child$/);
    expect(lines[4]).toMatch(/\[code:rust\] pub struct A \{\} … \(2 lines\)$/);
    expect(lines[5]).toMatch(/\[table 2×2\] step \| crates$/);
    expect(tree[2]!.children).toHaveLength(1);
    expect(outline(o, 1).blocks[2]!.children).toHaveLength(0);
  });

  test("long text is truncated to 120 chars", async () => {
    const o = await doc(`${"word ".repeat(60)}\n`, false);
    const line = outline(o).outline.split("\n")[0]!;
    expect(line.endsWith("…")).toBe(true);
    expect(line.length).toBeLessThan(36 + 1 + 122);
  });
});

describe("lcs", () => {
  test("aligns equal keys and skips changed ones", () => {
    expect(lcs(["a", "b", "c", "d"], ["a", "x", "c", "d"])).toEqual([
      [0, 0],
      [2, 2],
      [3, 3],
    ]);
    expect(lcs([], ["a"])).toEqual([]);
    expect(lcs(["a", "a"], ["a"])).toHaveLength(1);
  });
});
