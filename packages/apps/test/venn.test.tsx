/**
 * Venn (design 123 §6): the zod mirror agrees with the surface fixtures; the apps component carries the wire
 * props, `highlight` and `onSelect` (`$value` = pressed set id) to a host renderer, else shows its text form.
 */
import "./dom.ts";
import { readFileSync, readdirSync } from "node:fs";
import { afterEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DIAGRAMS } from "../src/diagrams.ts";
import { type DiagramHostProps, library, setDiagramRenderer } from "../src/library.ts";
import { AppRenderer } from "../src/renderer.tsx";

const NAME = "Venn";
const SURFACE = new URL("../../../crates/surface/fixtures/json-render/", import.meta.url);
const read = (name: string) =>
  JSON.parse(readFileSync(new URL(`${name}.json`, SURFACE), "utf8")) as {
    root: string;
    elements: Record<string, { type: string; props: Record<string, unknown> }>;
  };
const fixture = read("valid-venn");
const props = fixture.elements[fixture.root]!.props as Record<string, unknown> & { sets: { id: string }[] };
const broken = readdirSync(SURFACE)
  .filter((f) => f.startsWith("broken-venn-") && f.endsWith(".json"))
  .map((f) => f.slice(0, -5));

describe("Venn parity", () => {
  test("the zod mirror accepts the valid fixture", () => {
    expect(DIAGRAMS[NAME]!.props.safeParse(props).success).toBe(true);
  });
  test.each(broken)("rejects %s", (name) => {
    const f = read(name);
    const el = Object.values(f.elements).find((e) => e.type === NAME)!;
    expect(DIAGRAMS[NAME]!.props.safeParse(el.props).success).toBe(false);
  });
});

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  setDiagramRenderer(NAME, null);
});

async function mount(response: string) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  roots.push({ root, el });
  await act(async () => {
    root.render(<AppRenderer library={library} response={response} isStreaming={false} generation={1} provider={() => Promise.resolve(null)} />);
  });
  return el;
}

const HIGHLIGHT = ["tests"];
const units = props.sets.map((s) => s.id);
const TestHost = ({ props: p, highlight, onSelect }: DiagramHostProps) => (
  <div data-host="venn" data-highlight={(highlight ?? []).join(",")} data-props={JSON.stringify(p)}>
    {units.map((id) => (
      <button key={id} data-unit={id} onClick={() => onSelect?.(id)}>
        {id}
      </button>
    ))}
  </div>
);
// Positional args in catalog order (`src/diagrams.ts`): sets, overlaps, title, then highlight, onSelect.
const program = `root = Stack([diagram, TextContent("Selected: " + $sel)])
$sel = ""
diagram = ${NAME}(${JSON.stringify(props.sets)}, ${JSON.stringify(props.overlaps ?? null)}, ${JSON.stringify(props.title ?? null)}, ${JSON.stringify(HIGHLIGHT)}, Action([@Set($sel, $value)]))`;

describe("Venn app", () => {
  test("wire props, highlight and onSelect reach the host; a press binds $value", async () => {
    setDiagramRenderer(NAME, TestHost);
    const el = await mount(program);
    const host = el.querySelector("[data-host='venn']")!;
    expect(host).not.toBeNull();
    expect(JSON.parse(host.getAttribute("data-props")!)).toEqual(props);
    expect(host.getAttribute("data-highlight")).toBe(HIGHLIGHT.join(","));
    expect(el.textContent).toContain("Selected:");
    await act(async () => {
      (host.querySelector("button[data-unit='review']") as HTMLButtonElement).click();
    });
    expect(el.textContent).toContain("Selected: review");
  });

  test("without a host it shows its text form", async () => {
    const el = await mount(program);
    expect(el.querySelector(`pre[data-diagram='${NAME}']`)?.textContent).toBe("Venn: Release gates — 3 sets");
  });
});
