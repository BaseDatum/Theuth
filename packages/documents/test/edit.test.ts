import { describe, expect, test } from "bun:test";
import { EditFailure, blocks, edit, flatten, importMarkdown, isOurs, markdown, nearest, open, outline, type Open } from "../src/convert.ts";
import type { EditError, Restore } from "../src/protocol.ts";

/** A knowledge document: frontmatter + heading + three bullets (the middle one nested) + a paragraph. */
const DOC = `---
type: fact
title: Edits
tags:
  - a
---
## Heading

- one
- two
  - two point one
- three

last paragraph
`;

async function doc(text = DOC, frontmatter = true): Promise<Open> {
  const o = open(new Uint8Array());
  await importMarkdown(o, text, "replace", frontmatter);
  return o;
}

/** Top-level body ids (frontmatter skipped). */
const ids = (o: Open) => blocks(o).slice(1).map((b) => b.id);
/** Body text lines of the outline, ids stripped. */
const lines = (o: Open) =>
  outline(o)
    .outline.split("\n")
    .slice(1)
    .map((l) => l.replace(/^\s*[0-9a-f-]{36} /, (m) => m.replace(/[0-9a-f-]{36} /, "")));

/** Blocks with ids stripped, for before/after comparison of an undo. */
const shape = (o: Open) => JSON.parse(JSON.stringify(blocks(o), (k, v) => (k === "id" ? undefined : v)));

async function failure(p: Promise<unknown>): Promise<EditError> {
  try {
    await p;
  } catch (e) {
    if (e instanceof EditFailure) return e.detail;
    throw e;
  }
  throw new Error("expected an EditFailure");
}

describe("edit: insert", () => {
  test("after a block", async () => {
    const o = await doc();
    const [h, one] = ids(o);
    const r = await edit(o, [{ op: "insert", markdown: "inserted\n", after: h }]);
    expect(r.inserted).toHaveLength(1);
    expect(r.removed).toEqual([]);
    expect(r.touched).toEqual(r.inserted);
    expect(ids(o)[1]).toBe(r.inserted[0]!);
    expect(ids(o)[2]).toBe(one!);
    expect(lines(o)[1]).toBe("inserted");
  });

  test("before a block", async () => {
    const o = await doc();
    const [h] = ids(o);
    const r = await edit(o, [{ op: "insert", markdown: "first\n", before: h }]);
    expect(ids(o)[0]).toBe(r.inserted[0]!);
    expect(blocks(o)[0]!.type).toBe("frontmatter");
  });

  test("at start keeps the frontmatter first", async () => {
    const o = await doc();
    const r = await edit(o, [{ op: "insert", markdown: "start\n", at: "start" }]);
    expect(blocks(o)[0]!.type).toBe("frontmatter");
    expect(blocks(o)[1]!.id).toBe(r.inserted[0]!);
  });

  test("before the frontmatter lands after it", async () => {
    const o = await doc();
    const fm = blocks(o)[0]!.id;
    const r = await edit(o, [{ op: "insert", markdown: "x\n", before: fm }]);
    expect(blocks(o)[0]!.id).toBe(fm);
    expect(blocks(o)[1]!.id).toBe(r.inserted[0]!);
  });

  test("at end (the default)", async () => {
    const o = await doc();
    const r = await edit(o, [{ op: "insert", markdown: "tail\n" }]);
    expect(ids(o).at(-1)).toBe(r.inserted[0]!);
    expect(lines(o).at(-1)).toBe("tail");
  });

  test("several top-level blocks from one markdown, in order", async () => {
    const o = await doc();
    const r = await edit(o, [{ op: "insert", markdown: "### Sub\n\npara\n\n- bullet\n", at: "end" }]);
    expect(r.inserted).toHaveLength(3);
    expect(ids(o).slice(-3)).toEqual(r.inserted);
    expect(lines(o).slice(-3)).toEqual(["[h3] Sub", "para", "• bullet"]);
  });

  test("after a nested block inserts a sibling inside the same parent", async () => {
    const o = await doc();
    const nested = flatten(blocks(o)).find((b) => outline(o).blocks.some((t) => t.children.some((c) => c.id === b.id)))!;
    const r = await edit(o, [{ op: "insert", markdown: "two point two\n", after: nested.id }]);
    const two = blocks(o).find((b) => b.children.some((c) => c.id === nested.id))!;
    expect(two.children.map((c) => c.id)).toEqual([nested.id, r.inserted[0]!]);
  });

  test("two of after/before/at is a bad_edit; empty markdown too", async () => {
    const o = await doc();
    const [h] = ids(o);
    expect(await failure(edit(o, [{ op: "insert", markdown: "x", after: h, at: "end" }]))).toMatchObject({ code: "bad_edit" });
    expect(await failure(edit(o, [{ op: "insert", markdown: "   \n" }]))).toMatchObject({ code: "bad_edit" });
  });
});

describe("edit: replace and delete", () => {
  test("replace one block in place, keeping its neighbours' ids", async () => {
    const o = await doc();
    const before = ids(o);
    const r = await edit(o, [{ op: "replace", blocks: [before[1]!], markdown: "- uno\n" }]);
    expect<unknown[]>(r.removed).toEqual([before[1]]);
    expect(r.inserted).toHaveLength(1);
    const after = ids(o);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(r.inserted[0]!);
    expect(after.slice(2)).toEqual(before.slice(2));
    expect(lines(o)[1]).toBe("• uno");
  });

  test("replace a run with fewer blocks", async () => {
    const o = await doc();
    const before = ids(o);
    const r = await edit(o, [{ op: "replace", blocks: [before[1]!, before[2]!, before[3]!], markdown: "one paragraph instead\n" }]);
    expect<unknown[]>(r.removed).toEqual([before[1], before[2], before[3]]);
    expect<unknown[]>(ids(o)).toEqual([before[0], r.inserted[0]!, before[4]]);
  });

  test("replace ids given out of order removes them all and inserts at the first", async () => {
    const o = await doc();
    const before = ids(o);
    const r = await edit(o, [{ op: "replace", blocks: [before[3]!, before[1]!], markdown: "a\n\nb\n" }]);
    expect<unknown[]>(ids(o)).toEqual([before[0], ...r.inserted, before[2], before[4]]);
  });

  test("delete one and a run", async () => {
    const o = await doc();
    const before = ids(o);
    const r = await edit(o, [{ op: "delete", blocks: [before[4]!] }]);
    expect(r).toMatchObject({ inserted: [], touched: [], removed: [before[4]] });
    expect(ids(o)).toEqual(before.slice(0, 4));
    await edit(o, [{ op: "delete", blocks: [before[1]!, before[2]!] }]);
    expect<unknown[]>(ids(o)).toEqual([before[0], before[3]]);
  });

  test("deleting a parent takes its children; naming both is fine", async () => {
    const o = await doc();
    const two = ids(o)[2]!;
    const child = blocks(o)[3]!.children[0]!.id;
    const r = await edit(o, [{ op: "delete", blocks: [child, two] }]);
    expect(r.removed).toEqual([two]);
    expect(flatten(blocks(o)).some((b) => b.id === child)).toBe(false);
  });

  test("deleting every body block leaves one empty paragraph", async () => {
    const o = await doc();
    await edit(o, [{ op: "delete", blocks: ids(o) }]);
    const bs = blocks(o);
    expect(bs).toHaveLength(2);
    expect(bs[1]!.type).toBe("paragraph");
  });

  test("several edits are one transaction: one update, all ids touched", async () => {
    const o = await doc();
    const before = ids(o);
    let updates = 0;
    o.ydoc.on("update", (_u: Uint8Array, origin: unknown) => {
      if (isOurs(origin)) updates++;
    });
    const r = await edit(o, [
      { op: "replace", blocks: [before[2]!], markdown: "- X\n" },
      { op: "insert", markdown: "- Y\n", after: before[2]! === before[2] ? before[1]! : before[1]! },
      { op: "delete", blocks: [before[4]!] },
    ]);
    expect(updates).toBe(1);
    expect(r.touched.sort()).toEqual([...r.inserted].sort());
    expect(r.touched).toHaveLength(2);
    expect<unknown[]>(r.removed).toEqual([before[2], before[4]]);
  });
});

describe("edit: errors", () => {
  test("unknown id lists the nearest ids in outline order", async () => {
    const o = await doc();
    const all = flatten(blocks(o)).map((b) => b.id);
    const target = all[3]!;
    const bogus = `${target.slice(0, 8)}-0000-0000-0000-000000000000`;
    const e = await failure(edit(o, [{ op: "delete", blocks: [bogus] }]));
    expect(e).toMatchObject({ code: "unknown_block", id: bogus });
    if (e.code !== "unknown_block") throw new Error("unreachable");
    expect(e.nearest).toEqual(all.slice(0, 7));
    expect(e.nearest).toContain(target);
    expect(nearest(blocks(o), "zzz", 1)).toHaveLength(2);
  });

  test("an unknown insert anchor fails the same way and changes nothing", async () => {
    const o = await doc();
    const before = shape(o);
    let updates = 0;
    o.ydoc.on("update", () => updates++);
    const e = await failure(edit(o, [{ op: "insert", markdown: "x", after: "nope" }]));
    expect(e.code).toBe("unknown_block");
    expect(updates).toBe(0);
    expect(shape(o)).toEqual(before);
  });

  test("the frontmatter block cannot be replaced or deleted", async () => {
    const o = await doc();
    const fm = blocks(o)[0]!.id;
    expect(await failure(edit(o, [{ op: "replace", blocks: [fm], markdown: "x" }]))).toEqual({ code: "frontmatter_immutable", id: fm });
    expect(await failure(edit(o, [{ op: "delete", blocks: [fm, ids(o)[0]!] }]))).toEqual({ code: "frontmatter_immutable", id: fm });
    expect(blocks(o)[0]!.id).toBe(fm);
  });

  test("a failing edit later in the batch applies nothing from the batch", async () => {
    const o = await doc();
    const before = shape(o);
    await failure(edit(o, [{ op: "insert", markdown: "x" }, { op: "delete", blocks: ["nope"] }]));
    expect(shape(o)).toEqual(before);
  });
});

describe("edit: undo payload", () => {
  const roundTrip = async (o: Open, edits: Parameters<typeof edit>[1]) => {
    const before = shape(o);
    const md = await markdown(o);
    const r = await edit(o, edits);
    expect(shape(o)).not.toEqual(before);
    const u = await edit(o, r.undo);
    expect(shape(o)).toEqual(before);
    expect(await markdown(o)).toBe(md);
    return { r, u };
  };

  test("undo of a replace restores the exact prior blocks and ids", async () => {
    const o = await doc();
    const before = ids(o);
    const { r, u } = await roundTrip(o, [{ op: "replace", blocks: [before[2]!], markdown: "- changed\n" }]);
    expect(ids(o)).toEqual(before);
    expect(u.inserted).toEqual([before[2]!]);
    expect(u.removed).toEqual(r.inserted);
  });

  test("undo of an insert removes it; undo of a delete puts the run back where it was", async () => {
    const o = await doc();
    const before = ids(o);
    await roundTrip(o, [{ op: "insert", markdown: "a\n\nb\n", after: before[0]! }]);
    expect(ids(o)).toEqual(before);
    await roundTrip(o, [{ op: "delete", blocks: [before[1]!, before[2]!] }]);
    expect(ids(o)).toEqual(before);
    await roundTrip(o, [{ op: "delete", blocks: [before[0]!] }]);
    expect(ids(o)).toEqual(before);
  });

  test("undo of a delete at the very end and of a nested child", async () => {
    const o = await doc();
    const before = ids(o);
    await roundTrip(o, [{ op: "delete", blocks: [before[4]!] }]);
    const child = blocks(o)[3]!.children[0]!.id;
    await roundTrip(o, [{ op: "delete", blocks: [child] }]);
    expect(blocks(o)[3]!.children[0]!.id).toBe(child);
  });

  test("undo of a batch is one restore per edit, newest first, and reverts all of it", async () => {
    const o = await doc();
    const before = ids(o);
    const { r } = await roundTrip(o, [
      { op: "replace", blocks: [before[1]!], markdown: "- r\n" },
      { op: "insert", markdown: "i\n", at: "end" },
      { op: "delete", blocks: [before[3]!] },
    ]);
    expect(r.undo.map((u: Restore) => u.op)).toEqual(["restore", "restore", "restore"]);
    expect(r.undo[0]!.blocks).toHaveLength(1);
    expect(r.undo[1]!.remove).toHaveLength(1);
    expect(ids(o)).toEqual(before);
  });

  test("undo whose anchor is gone fails as unknown_block", async () => {
    const o = await doc();
    const before = ids(o);
    const r = await edit(o, [{ op: "delete", blocks: [before[4]!] }]);
    await edit(o, [{ op: "delete", blocks: [before[3]!] }]);
    expect((await failure(edit(o, r.undo))).code).toBe("unknown_block");
  });
});

describe("outline and annotated markdown (§5.1)", () => {
  test("outline golden", async () => {
    const o = await doc(`---
type: project
title: wth apps — design doc 46
tags:
  - wth
  - openui
---
## Decisions

- app = artifact Kind::App (blob = OpenUI Lang v0.5 source)
- one new tool app{name, source, edit?} ${"x".repeat(200)}

## Open questions

\`\`\`rust
pub struct ArtifactActor {
  x: u8,
}
\`\`\`

| step | crates | done when |
| --- | --- | --- |
| 1 | a | b |
| 2 | c | d |
`);
    const text = outline(o).outline;
    const golden = [
      '[fm] type=project title="wth apps — design doc 46" tags=wth,openui',
      "ID [h2] Decisions",
      "ID • app = artifact Kind::App (blob = OpenUI Lang v0.5 source)",
      `ID • ${"one new tool app{name, source, edit?} ".concat("x".repeat(200)).slice(0, 120)}…`,
      "ID [h2] Open questions",
      "ID [code:rust] pub struct ArtifactActor { … (3 lines)",
      "ID [table 3×3] step | crates | done when",
    ].join("\n");
    expect(text.replace(/[0-9a-f-]{36}/g, "ID")).toBe(golden);
    expect(text.split("\n").slice(1).every((l) => /^ *[0-9a-f-]{36} /.test(l))).toBe(true);
  });

  test("children are indented two spaces per level and depth cuts them", async () => {
    const o = await doc();
    const text = outline(o).outline.split("\n");
    expect(text[4]).toMatch(/^ {2}[0-9a-f-]{36} • two point one$/);
    expect(outline(o, 1).outline.split("\n")).toHaveLength(6);
  });

  test("annotated markdown prefixes every body block with its id, and the author when known", async () => {
    const o = await doc();
    const [h, one] = ids(o);
    const md = await markdown(o, undefined, true, { [one!]: "Ana" });
    expect(md.startsWith("---\ntype: fact\n")).toBe(true);
    expect(md).toContain(`<!-- ${h} -->\n## Heading\n`);
    expect(md).toContain(`<!-- ${one} by Ana -->\n`);
    expect(md.match(/<!-- [0-9a-f-]{36}( by \w+)? -->/g)).toHaveLength(ids(o).length);
    expect(await markdown(o)).not.toContain("<!--");
  });

  test("markdown of some blocks reaches nested ids and keeps document order", async () => {
    const o = await doc();
    const child = blocks(o)[3]!.children[0]!.id;
    const last = ids(o).at(-1)!;
    const md = await markdown(o, [last, child], true);
    const order = [...md.matchAll(/<!-- ([0-9a-f-]{36}) -->/g)].map((m) => m[1]);
    expect(order).toEqual([child, last]);
    expect(md).not.toContain("---\n");
  });
});
