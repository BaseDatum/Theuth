/**
 * SPIKE: run the unchanged suite (`test/*.test.ts`) against the in-process editor shim instead of jsdom.
 *   DOM=linkedom bun test --preload ./test-inproc/preload.ts test/
 *   DOM=happy-dom bun test --preload ./test-inproc/preload.ts test/
 */
import { mock } from "bun:test";

const dom = process.env.DOM ?? "linkedom";
if (dom === "happy-dom") {
  const happy = await import("../src/inproc/dom-happy.ts");
  mock.module("../src/inproc/dom.ts", () => happy);
}
const shim = await import("../src/inproc/editor.ts");
mock.module("@blocknote/server-util", () => shim);
console.error(`[inproc preload] @blocknote/server-util → src/inproc/editor.ts, DOM=${dom}`);
