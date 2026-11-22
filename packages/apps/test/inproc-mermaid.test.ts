/**
 * The committed in-process mermaid bundle (crates/js-runtime/js/theuth-mermaid-inproc.js) in a bare node:vm
 * context (ECMAScript built-ins + crates/js-runtime/js/polyfill.js and polyfill-web.js only) against the parity
 * corpus recorded from the jsdom reference path (crates/js-runtime/tests/mermaid_golden.json;
 * `bun run test-inproc/mermaid-corpus/reference.ts`). The Rust side replays the same golden file in deno_core
 * (crates/js-runtime/tests/mermaid_golden.rs).
 */
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import vm from "node:vm";
import stub from "../src/inproc/stub.ts";
import { answer } from "../src/mermaid-core.ts";

const root = join(import.meta.dir, "..", "..", "..");
const bundle = readFileSync(join(root, "crates/js-runtime/js/theuth-mermaid-inproc.js"), "utf8");
/** What the runtime loads before an app bundle: the shared base, then the app bundles' web supplement. */
const polyfill = ["polyfill.js", "polyfill-web.js"]
  .map((f) => readFileSync(join(root, "crates/js-runtime/js", f), "utf8"))
  .join("\n");
type Reply = { ok: boolean; errors?: { line: number | null; message: string }[] };
const golden = JSON.parse(readFileSync(join(root, "crates/js-runtime/tests/mermaid_golden.json"), "utf8")) as Record<
  string,
  { type: string; source: string; reply: Reply }
>;
/** The message is the JS engine's own TypeError text (and minified identifiers); ok and line must match. */
const ENGINE_WORDING = new Set(["edge/timeline-typeerror"]);

const fakeCore = {
  ops: { op_jsrt_random_fill: (b: Uint8Array) => crypto.getRandomValues(b) },
  createTimer: () => 0,
  cancelTimer: () => {},
  print: () => {},
};

function load(): { call(line: string): Promise<string>; version: string } {
  const ctx = vm.createContext({ Deno: { core: fakeCore } });
  vm.runInContext(polyfill, ctx, { filename: "polyfill.js" });
  vm.runInContext(bundle, ctx, { filename: "theuth-mermaid-inproc.js" });
  return vm.runInContext("globalThis.theuthMermaid", ctx);
}

describe("committed mermaid bundle in a bare JS context", () => {
  test("every corpus case answers like the jsdom reference", async () => {
    const api = load();
    expect(api.version).toBe("theuth-mermaid 1");
    const names = Object.keys(golden);
    expect(names.length).toBe(148);
    let exact = 0;
    for (const name of names) {
      const g = golden[name]!;
      const got = JSON.parse(await api.call(JSON.stringify({ id: name, op: "check_mermaid", source: g.source })));
      expect(got.id).toBe(name);
      delete got.id;
      if (ENGINE_WORDING.has(name)) {
        expect([name, got.ok, got.errors?.[0]?.line]).toEqual([name, g.reply.ok, g.reply.errors?.[0]?.line]);
        continue;
      }
      expect([name, got]).toEqual([name, g.reply]);
      exact++;
    }
    expect(exact).toBe(147);
  }, 120_000);

  test("malformed lines are error responses", async () => {
    const api = load();
    expect(JSON.parse(await api.call("null"))).toEqual({ id: null, error: "request must be a JSON object" });
    expect(JSON.parse(await api.call(JSON.stringify({ id: "x", op: "check", source: "" })))).toEqual({
      id: "x",
      error: 'op must be "check_mermaid"',
    });
  });
});

describe("render-only stub (review m2)", () => {
  test("a parse path that reaches a stubbed library fails the request instead of calling the diagram invalid", async () => {
    // The stub is a callable Proxy; a fake diagram parser that calls into it, like a layout engine would.
    const layoutEngine = stub as () => unknown;
    const parser = { parse: async () => layoutEngine() };
    await expect(answer(parser, "flowchart TD\n  A --> B")).rejects.toThrow("render-only library");
    // Registration at chunk load stays a no-op, and real parse errors are still verdicts.
    expect(() => (stub as { use: (p: unknown) => unknown }).use({})).not.toThrow();
    const broken = { parse: async () => Promise.reject(new Error("Parse error on line 3:\n...")) };
    expect(await answer(broken, "x")).toEqual({ ok: false, errors: [{ line: 3, message: "Parse error on line 3:\n..." }] });
  });
});
