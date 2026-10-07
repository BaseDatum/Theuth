/**
 * 123 W2: the parity shape of `graph.test.tsx` for one diagram component — the zod mirror accepts every surface
 * `valid-<prefix>*` fixture and rejects the `broken-<prefix>*` ones at the crate's path; through `AppRenderer` the
 * host installed by `setDiagramRenderer(name)` receives the wire props, the reactive `highlight`, and a unit press
 * runs `onSelect` with the unit id as `$value`.
 */
import "./dom.ts";
import { readFileSync, readdirSync } from "node:fs";
import { afterEach, describe, expect, test } from "bun:test";
import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { check } from "../src/check.ts";
import { DIAGRAMS } from "../src/diagrams.ts";
import { type DiagramHostProps, library, reactiveProps, setDiagramRenderer } from "../src/library.ts";
import { AppRenderer } from "../src/renderer.tsx";

const SURFACE = new URL("../../../crates/surface/fixtures/json-render/", import.meta.url);
const read = (name: string) =>
  JSON.parse(readFileSync(new URL(`${name}.json`, SURFACE), "utf8")) as {
    elements: Record<string, { type: string; props: Record<string, unknown> }>;
  };

/** The path `crates/surface/src/lib.rs` LOCATED pins for each broken fixture. */
const RUST_PATHS: Record<string, string> = Object.fromEntries(
  [
    ...readFileSync(new URL("../../../crates/surface/src/lib.rs", import.meta.url), "utf8").matchAll(
      /fixture!\("(broken-[\w-]+)"\),\s*"(\/elements\/[^"]+)"/g,
    ),
  ].map((m) => [m[1] as string, m[2] as string]),
);

let roots: { root: Root; el: HTMLElement }[] = [];

export interface ParityCase {
  /** Component name (`Gantt`). */
  name: string;
  /** Fixture stem (`gantt` → `valid-gantt*`, `broken-gantt-*`). */
  prefix: string;
  /** An app whose root is the component with `highlight = $lit` and `onSelect = Action([@Set($selected, $value)])`. */
  app: string;
  /** Unit ids the host renders buttons for, read off the wire props. */
  units: (props: Record<string, unknown>) => string[];
  /** The id the test presses and expects as `$value`. */
  press: string;
  /** What `$lit` starts as. */
  lit: string[];
}

export function parity(c: ParityCase) {
  const fixtures = readdirSync(SURFACE)
    .filter((f) => f.endsWith(".json"))
    .map((f) => f.slice(0, -5));
  const of = (kind: "valid" | "broken") =>
    fixtures
      .filter((f) => f.startsWith(`${kind}-${c.prefix}`))
      .flatMap((f) =>
        Object.entries(read(f).elements)
          .filter(([, el]) => el.type === c.name)
          .map(([id, el]) => [f, id, el.props] as const),
      );

  describe(`${c.name} schema parity with crates/surface`, () => {
    const valid = of("valid");
    test("has valid fixtures of its own", () => {
      expect(valid.length).toBeGreaterThanOrEqual(2);
    });
    test.each(valid)("%s/%s is accepted", (_f, _id, props) => {
      expect(DIAGRAMS[c.name]!.props.safeParse(props).error?.issues ?? []).toEqual([]);
    });
    const broken = of("broken");
    test("has broken fixtures", () => {
      expect(broken.length).toBeGreaterThan(0);
    });
    test.each(broken)("%s/%s is rejected at the crate's path", (f, id, props) => {
      const r = DIAGRAMS[c.name]!.props.safeParse(props);
      expect(r.success).toBeFalse();
      const paths = r.error!.issues.map((i) => `/elements/${id}/props/${i.path.join("/")}`);
      expect(RUST_PATHS[f], f).toBeDefined();
      expect(paths).toContain(RUST_PATHS[f]!);
    });
    test("highlight is the reactive prop; the signature ends with highlight and onSelect", () => {
      expect(reactiveProps(library)[c.name]).toEqual(["highlight"]);
      const sig = library.toSpec().components[c.name]!.signature;
      expect(sig).toContain("highlight?: $binding<string[]>, onSelect?: ActionExpression)");
    });
  });

  describe(`${c.name} through AppRenderer`, () => {
    afterEach(async () => {
      for (const { root, el } of roots) {
        await act(async () => root.unmount());
        el.remove();
      }
      roots = [];
      setDiagramRenderer(c.name, null);
    });

    test("publishes without diagnostics", () => {
      expect(check(c.app, []).diagnostics).toEqual([]);
    });

    test("the host receives the wire props and highlight; a unit press sets $selected to the unit id", async () => {
      let seen: DiagramHostProps | null = null;
      setDiagramRenderer(c.name, ({ props, highlight, onSelect }: DiagramHostProps) => {
        seen = { props, highlight, onSelect };
        return (
          <div data-host={c.name} data-highlight={(highlight ?? []).join(",")}>
            {c.units(props as Record<string, unknown>).map((id) => (
              <button key={id} type="button" data-unit={id} onClick={() => onSelect?.(id)}>
                {id}
              </button>
            ))}
          </div>
        );
      });
      const el = document.createElement("div");
      document.body.appendChild(el);
      const root = createRoot(el);
      roots.push({ root, el });
      await act(async () => {
        root.render(
          <StrictMode>
            <AppRenderer library={library} response={c.app} isStreaming={false} generation={1} provider={() => Promise.resolve(null)} />
          </StrictMode>,
        );
      });
      const host = el.querySelector(`[data-host='${c.name}']`)!;
      expect(host).not.toBeNull();
      expect(host.getAttribute("data-highlight")).toBe(c.lit.join(","));
      expect(seen).not.toBeNull();
      expect(Object.keys((seen as unknown as DiagramHostProps).props)).not.toContain("highlight");
      expect(Object.keys((seen as unknown as DiagramHostProps).props)).not.toContain("onSelect");
      const btn = el.querySelector<HTMLButtonElement>(`[data-unit='${c.press}']`)!;
      expect(btn).not.toBeNull();
      await act(async () => btn.click());
      const text = [...el.querySelectorAll("p, div")].map((n) => n.textContent ?? "").find((t) => t.startsWith("Selected: "));
      expect(text).toBe(`Selected: ${c.press}`);
    });
  });
}
