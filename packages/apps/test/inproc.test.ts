/**
 * The in-process entry (`src/inproc.ts`) and its committed bundle (crates/js-runtime/js/theuth-apps-inproc.js):
 * protocol behaviour of `call(line)`, parity of the bundle run in a bare node:vm context (ECMAScript built-ins
 * + crates/js-runtime/js/polyfill.js and polyfill-web.js only) with direct `call()`, the global set it needs, and the golden
 * transcript the Rust tests replay.
 */
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import vm from "node:vm";
import { libraryHash } from "../src/check.ts";
import { MAX_LINE_BYTES, call, version } from "../src/inproc.ts";
import { CASES, livePanel, prototype, tools } from "./inproc-cases.ts";

const root = join(import.meta.dir, "..", "..", "..");
const bundle = readFileSync(join(root, "crates/js-runtime/js/theuth-apps-inproc.js"), "utf8");
/** What the runtime loads before an app bundle: the shared base, then the app bundles' web supplement. */
const polyfill = ["polyfill.js", "polyfill-web.js"]
  .map((f) => readFileSync(join(root, "crates/js-runtime/js", f), "utf8"))
  .join("\n");
const golden = JSON.parse(readFileSync(join(root, "crates/js-runtime/tests/apps_golden.json"), "utf8")) as {
  version: string;
  cases: { name: string; request: string; response: string }[];
};

/** A stand-in for the `Deno.core` the polyfill reads (deno_core's ops are not available under bun). */
const fakeCore = {
  ops: {
    op_jsrt_random_fill: (b: Uint8Array) => crypto.getRandomValues(b),
  },
  createTimer: () => 0,
  cancelTimer: () => {},
  print: () => {},
};

type Entry = { call(line: string): string; version(): string };

function bare(extra: Record<string, unknown>, withPolyfill: boolean): { ctx: vm.Context; entry: Entry } {
  const ctx = vm.createContext({ Deno: { core: fakeCore }, ...extra });
  if (withPolyfill) vm.runInContext(polyfill, ctx, { filename: "polyfill.js" });
  vm.runInContext(bundle, ctx, { filename: "theuth-apps-inproc.js" });
  const entry = vm.runInContext("globalThis.theuthApps", ctx) as Entry;
  return { ctx, entry };
}

describe("inproc call(line)", () => {
  test("version is the helper's --version line", () => {
    expect(version()).toBe(`theuth-app-parser ${libraryHash}`);
  });

  test("check and merge_check answer with the request id and the check result", () => {
    const r = JSON.parse(call(JSON.stringify({ id: "7", op: "check", source: livePanel, tools })));
    expect(r.id).toBe("7");
    expect(r.diagnostics).toEqual([]);
    expect(r.library_hash).toBe(libraryHash);
    expect(r.bindings.map((b: { statement_id: string }) => b.statement_id)).toEqual(["health", "syncResult"]);
    const m = JSON.parse(
      call(JSON.stringify({ id: "8", op: "merge_check", source: 'title = TextContent("## B")', base: prototype, tools: [] })),
    );
    expect(m.id).toBe("8");
    expect(m.diagnostics).toEqual([]);
    expect(m.source).toContain('title = TextContent("## B")');
  });

  test("malformed lines are error responses, never throws", () => {
    expect(JSON.parse(call("{"))).toMatchObject({ id: null });
    expect(JSON.parse(call("null")).error).toBe("request must be a JSON object");
    expect(JSON.parse(call(JSON.stringify({ id: 1, op: "check" }))).error).toBe("missing string field id");
    expect(JSON.parse(call(JSON.stringify({ id: "x", op: "nope" })))).toEqual({
      id: "x",
      error: 'op must be "check" or "merge_check"',
    });
    expect(JSON.parse(call(JSON.stringify({ id: "x", op: "check_mermaid", source: "graph TD" }))).id).toBe("x");
  });

  test("a line over MAX_LINE_BYTES (UTF-8) is refused without parsing", () => {
    const big = JSON.stringify({ id: "b", op: "check", source: "é".repeat(MAX_LINE_BYTES / 2), tools: [] });
    expect(JSON.parse(call(big))).toEqual({ id: null, error: `request line exceeds ${MAX_LINE_BYTES} bytes` });
  });
});

describe("committed bundle in a bare JS context", () => {
  test("the vm context really is bare: no web globals before the polyfill", () => {
    // Bun's vm contexts do carry `console` (deno_core's bare runtime has one too); nothing else web-ish.
    const ctx = vm.createContext({});
    for (const g of ["TextEncoder", "TextDecoder", "structuredClone", "fetch", "process", "URL", "crypto", "atob"])
      expect([g, vm.runInContext(`typeof globalThis.${g}`, ctx)]).toEqual([g, "undefined"]);
  });

  test("with the polyfill, every case matches direct call() byte for byte", () => {
    const { entry } = bare({}, true);
    expect(entry.version()).toBe(version());
    expect(CASES.length).toBeGreaterThanOrEqual(30);
    for (const c of CASES) expect([c.name, entry.call(c.line)]).toEqual([c.name, call(c.line)]);
  });

  test("TextEncoder is the only global the bundle needs beyond ECMAScript", () => {
    // No polyfill at all: loading works, and every case matches once TextEncoder alone is present.
    const { entry } = bare({ TextEncoder }, false);
    for (const c of CASES) expect([c.name, entry.call(c.line)]).toEqual([c.name, call(c.line)]);
    // Without it, the UTF-8 line cap throws for any line over MAX_LINE_BYTES/3 UTF-16 units.
    const { entry: none } = bare({}, false);
    const long = JSON.stringify({ id: "o", op: "check", source: "a".repeat(MAX_LINE_BYTES / 3 + 10), tools: [] });
    expect(() => none.call(long)).toThrow();
  });

  test("telemetry stays off: no fetch is attempted", async () => {
    let fetched = 0;
    const { entry } = bare({ fetch: () => (fetched++, Promise.reject(new Error("no network"))) }, true);
    for (const c of CASES) entry.call(c.line);
    // lang-core decides synchronously (no `process` → no context); its capture would run on later
    // microtasks, so drain a few before looking.
    for (let i = 0; i < 20; i++) await Promise.resolve();
    expect(fetched).toBe(0);
  });
});

describe("golden transcript (crates/js-runtime/tests/apps_golden.json)", () => {
  test("is current: recorded from this TypeScript (run scripts/record-inproc-golden.ts after a change)", () => {
    expect(golden.version).toBe(version());
    expect(golden.cases.map((c) => c.name)).toEqual(CASES.map((c) => c.name));
    for (const g of golden.cases) expect([g.name, call(g.request)]).toEqual([g.name, g.response]);
  });
});
