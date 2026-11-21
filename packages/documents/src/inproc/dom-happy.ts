/** SPIKE candidate: happy-dom (imports node: built-ins, so test-only — it cannot go into the embedded bundle). */
import { Window } from "happy-dom";
import type { DomWindow } from "./dom.ts";

export function domWindow(): DomWindow {
  return new Window() as unknown as DomWindow;
}
