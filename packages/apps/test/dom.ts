/**
 * jsdom globals for tests that mount React. Import before anything that touches `react-dom`.
 */
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  pretendToBeVisual: true,
});
const g = globalThis as Record<string, unknown>;
g.window = dom.window;
g.document = dom.window.document;
g.navigator = dom.window.navigator;
for (const key of [
  "HTMLElement",
  "HTMLInputElement",
  "Element",
  "Node",
  "Text",
  "Event",
  "MouseEvent",
  "KeyboardEvent",
  "getComputedStyle",
])
  g[key] ??= (dom.window as unknown as Record<string, unknown>)[key];
g.IS_REACT_ACT_ENVIRONMENT = true;
