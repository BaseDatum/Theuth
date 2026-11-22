/**
 * The DOM the documents helper renders and parses through — linkedom (pure JS, no Node built-ins),
 * plus the three gaps BlockNote 0.54.2 hits on the helper's paths, each patched here and nowhere else:
 *
 * 1. `document.implementation.createHTMLDocument` (parsers/html/util/nestedLists.ts: detached parse root);
 *    linkedom's Document has no `implementation`.
 * 2. `createTreeWalker(root, whatToShow, filter)` (normalizeWhitespace.ts): linkedom ignores `filter`, so
 *    whitespace inside `<pre>`/`<code>` was collapsed and code blocks lost their newlines.
 * 3. `HTMLInputElement.checked` (CheckListItem parse): linkedom has no such property, so `- [x]` read
 *    as unchecked. Reflected from the `checked` attribute (the only state a parsed/serialised node has).
 */
import { parseHTML } from "linkedom";

export type DomWindow = { document: Document };

const PAGE = "<!doctype html><html><head></head><body></body></html>";
const FILTER_ACCEPT = 1;
const FILTER_REJECT = 2;

type Filter = ((n: Node) => number) | { acceptNode(n: Node): number } | null | undefined;
type Walker = { nextNode(): Node | null; currentNode: Node | null };
type Patched = Document & { implementation?: unknown; __theuthPatched?: true };

const patchedProtos = new WeakSet<object>();

function patch(doc: Document): Document {
  const d = doc as Patched;
  if (d.__theuthPatched) return doc;
  d.__theuthPatched = true;
  if (!d.implementation) {
    Object.defineProperty(d, "implementation", {
      value: { createHTMLDocument: () => patch((parseHTML(PAGE) as unknown as DomWindow).document) },
    });
  }

  const native = doc.createTreeWalker.bind(doc) as (root: Node, show?: number) => Walker;
  Object.defineProperty(d, "createTreeWalker", {
    value: (root: Node, whatToShow?: number, filter?: Filter): Walker => {
      const inner = native(root, whatToShow);
      if (!filter) return inner;
      const accept = typeof filter === "function" ? filter : (n: Node) => filter.acceptNode(n);
      const rejected: Node[] = [];
      const walker: Walker = {
        currentNode: root,
        nextNode() {
          for (let n = inner.nextNode(); n; n = inner.nextNode()) {
            if (rejected.some((r) => r.contains(n))) continue;
            const verdict = accept(n);
            if (verdict === FILTER_ACCEPT) return (walker.currentNode = n);
            if (verdict === FILTER_REJECT) rejected.push(n);
          }
          return (walker.currentNode = null);
        },
      };
      return walker;
    },
  });

  const inputProto = Object.getPrototypeOf(doc.createElement("input")) as object;
  if (!patchedProtos.has(inputProto) && !("checked" in inputProto)) {
    patchedProtos.add(inputProto);
    Object.defineProperty(inputProto, "checked", {
      get(this: Element) {
        return this.hasAttribute("checked");
      },
      set(this: Element, v: boolean) {
        if (v) this.setAttribute("checked", "");
        else this.removeAttribute("checked");
      },
    });
  }
  return doc;
}

export function domWindow(): DomWindow {
  const w = parseHTML(PAGE) as unknown as DomWindow;
  patch(w.document);
  return w;
}
