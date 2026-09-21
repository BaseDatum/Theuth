import { describe, expect, test } from "bun:test";
import { emitHeader, emitScalar, get, parseHeader, set, splitFrontmatter } from "../src/yaml.ts";

describe("header parse/emit", () => {
  test("every Meta key round-trips, block and flow lists both read, unknown keys survive in place", () => {
    const text = `type: fact
title: "a title: with colon"
description: 'single ''quoted'''
tags: [rust, "tool chain"]
scope: /Users/x/y
source: harvest
timestamp: 2026-09-10T09:41:03Z
custom: kept
`;
    const e = parseHeader(text);
    expect(e.map((x) => x.key)).toEqual(["type", "title", "description", "tags", "scope", "source", "timestamp", "custom"]);
    expect(get(e, "title")).toBe("a title: with colon");
    expect(get(e, "description")).toBe("single 'quoted'");
    expect(e[3]).toEqual({ key: "tags", list: ["rust", "tool chain"] });
    expect(get(e, "timestamp")).toBe("2026-09-10T09:41:03Z");
    expect(emitHeader(e)).toBe(`type: fact
title: "a title: with colon"
description: single 'quoted'
tags:
  - rust
  - tool chain
scope: /Users/x/y
source: harvest
timestamp: 2026-09-10T09:41:03Z
custom: kept
`);
  });

  test("block sequences (okf's emitter style) parse to lists; empty flow list stays a list", () => {
    const e = parseHeader("tags:\n  - a\n  - b\nempty: []\n");
    expect(e).toEqual([
      { key: "tags", list: ["a", "b"] },
      { key: "empty", list: [] },
    ]);
    expect(emitHeader(e)).toBe("tags:\n  - a\n  - b\nempty: []\n");
  });

  test("a nested mapping is kept verbatim as raw lines", () => {
    const text = "type: fact\nmeta:\n  owner: me\n  level: 2\ntitle: t\n";
    const e = parseHeader(text);
    expect(e[1]).toEqual({ key: "meta", raw: ["meta:", "  owner: me", "  level: 2"] });
    expect(emitHeader(e)).toBe(text);
  });

  test("comments and blank lines are dropped; a trailing # comment is stripped from a plain scalar", () => {
    const e = parseHeader("# top\n\ntype: fact # why\n");
    expect(e).toEqual([{ key: "type", value: "fact" }]);
  });

  test("emitScalar quotes what would re-read as another type or break the line", () => {
    expect(emitScalar("plain words")).toBe("plain words");
    expect(emitScalar("")).toBe('""');
    expect(emitScalar("null")).toBe('"null"');
    expect(emitScalar("true")).toBe('"true"');
    expect(emitScalar("42")).toBe('"42"');
    expect(emitScalar("1.5")).toBe('"1.5"');
    expect(emitScalar("007")).toBe("007");
    expect(emitScalar("- dash")).toBe('"- dash"');
    expect(emitScalar("a: b")).toBe('"a: b"');
    expect(emitScalar("a #b")).toBe('"a #b"');
    expect(emitScalar("a#b")).toBe("a#b");
    expect(emitScalar("line\nbreak")).toBe('"line\\nbreak"');
    expect(emitScalar(' lead')).toBe('" lead"');
    expect(emitScalar('say "hi"')).toBe('say "hi"'); // okf keeps inner quotes plain
    expect(emitScalar('"quoted"')).toBe('"\\"quoted\\""');
  });

  test("set/get: replace in place, append new, remove with undefined", () => {
    let e = parseHeader("type: fact\ntitle: t\n");
    e = set(e, "title", "new");
    e = set(e, "tags", ["x"]);
    expect(get(e, "title")).toBe("new");
    expect(get(e, "tags")).toBe("x");
    expect(e.map((x) => x.key)).toEqual(["type", "title", "tags"]);
    e = set(e, "title", undefined);
    expect(e.map((x) => x.key)).toEqual(["type", "tags"]);
    expect(get(e, "title")).toBeNull();
  });

  test("splitFrontmatter separates the header; a --- rule inside the body is left alone; CRLF tolerated", () => {
    expect(splitFrontmatter("---\na: 1\n---\nbody\n\n---\n\nmore")).toEqual({ header: "a: 1", body: "body\n\n---\n\nmore" });
    expect(splitFrontmatter("no header\n---\n")).toEqual({ header: null, body: "no header\n---\n" });
    expect(splitFrontmatter("---\r\na: 1\r\n---\r\nb")).toEqual({ header: "a: 1", body: "b" });
    expect(splitFrontmatter("---\n---\nb")).toEqual({ header: "", body: "b" });
  });
});
