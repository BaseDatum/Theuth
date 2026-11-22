/**
 * The apps diagram components (design 123 §6): the zod mirrors in `src/diagrams.ts` accept every `valid-*` fixture
 * of `crates/surface` for their components and reject the `broken-*` ones at the crate's JSON-Pointer path; each
 * is registered after `OptionCard` with reactive `highlight` and `onSelect`, and renders its text form without a
 * host renderer. `graph.test.tsx` covers `Graph`.
 */
import "./dom.ts";
import { readFileSync, readdirSync } from "node:fs";
import { afterEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DIAGRAMS, diagramText } from "../src/diagrams.ts";
import {
  CURATED_COMPONENTS,
  type DiagramHostProps,
  library,
  reactiveProps,
  setDiagramRenderer,
} from "../src/library.ts";
import { AppRenderer } from "../src/renderer.tsx";

const SURFACE = new URL(
  "../../../crates/surface/fixtures/json-render/",
  import.meta.url,
);
const fixtures = readdirSync(SURFACE)
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.slice(0, -5));
/** A fixture's body; the repair-only fixtures (`broken-python-literals` …) are not strict JSON and parse as empty. */
const read = (name: string) => {
  try {
    return JSON.parse(readFileSync(new URL(`${name}.json`, SURFACE), "utf8")) as {
      elements: Record<string, { type: string; props: unknown }>;
    };
  } catch {
    return { elements: {} };
  }
};

/** Every element of a fixture whose type is one of ours: `[fixture, elementId, type, props]`. */
const elements = (prefix: string) =>
  fixtures
    .filter((f) => f.startsWith(prefix))
    .flatMap((f) =>
      Object.entries(read(f).elements)
        .filter(([, el]) => el.type in DIAGRAMS)
        .map(([id, el]) => [f, id, el.type, el.props] as const),
    );

const NAMES = Object.keys(DIAGRAMS);

describe("diagram schema parity with crates/surface", () => {
  test("the fifteen names end the catalog order, after Graph", () => {
    expect(CURATED_COMPONENTS.slice(-NAMES.length)).toEqual(NAMES as never);
    expect(CURATED_COMPONENTS.at(-NAMES.length - 1)).toBe("Graph");
    expect(NAMES).toEqual([
      "StateMachine",
      "Entities",
      "Swimlane",
      "GraphDelta",
      "Waterfall",
      "Funnel",
      "Gantt",
      "Quadrant",
      "Treemap",
      "Sankey",
      "Venn",
      "Loop",
      "Layers",
      "Journey",
      "Fishbone",
    ]);
  });

  const valid = elements("valid-");
  test("every component has a valid fixture", () => {
    expect(new Set(valid.map(([, , t]) => t))).toEqual(new Set(NAMES));
  });
  test.each(valid)("%s/%s (%s) is accepted", (_f, _id, type, props) => {
    const r = DIAGRAMS[type]!.props.safeParse(props);
    expect(r.error?.issues ?? []).toEqual([]);
  });

  const broken = elements("broken-");
  test("every component has a broken fixture", () => {
    expect(new Set(broken.map(([, , t]) => t))).toEqual(new Set(NAMES));
  });
  test.each(broken)("%s/%s (%s) is rejected at the crate's path", (f, id, type, props) => {
    const r = DIAGRAMS[type]!.props.safeParse(props);
    expect(r.success).toBeFalse();
    // The crate pins one path per broken fixture (`lib.rs` LOCATED); here it must be among zod's issues.
    const paths = r.error!.issues.map((i) => `/elements/${id}/props/${i.path.join("/")}`);
    expect(paths.length).toBeGreaterThan(0);
    expect(RUST_PATHS[f], `${f}: ${paths.join(", ")}`).toBeDefined();
    expect(paths).toContain(RUST_PATHS[f]!);
  });

  test("the exact messages of the design's three shapes", () => {
    const first = (type: string, props: unknown) => {
      const i = DIAGRAMS[type]!.props.safeParse(props).error!.issues[0]!;
      return `${i.path.join("/")}: ${i.message}`;
    };
    expect(
      first("StateMachine", { states: [{ id: "a", label: "A" }], transitions: [{ from: "a", to: "x" }] }),
    ).toBe('transitions/0/to: unknown state "x"');
    expect(
      first("Sankey", {
        nodes: [{ id: "a", label: "A" }, { id: "b", label: "B" }],
        flows: [{ from: "a", to: "b", value: 1 }, { from: "b", to: "a", value: 1 }],
      }),
    ).toBe("flows/1: circular flow a → b → a");
    expect(first("Venn", { sets: [{ id: "a", label: "A" }] })).toBe("sets: 1 sets, expected 2..=3");
  });

  test("highlight is the reactive prop of every diagram; each closes the container child union", () => {
    const rp = reactiveProps(library);
    const spec = library.toSpec();
    for (const name of NAMES) {
      expect(rp[name], name).toEqual(["highlight"]);
      expect(spec.components[name]!.signature, name).toContain("highlight?: $binding<string[]>");
      expect(spec.components[name]!.signature, name).toContain("onSelect?: ActionExpression");
      expect(spec.components.Card!.signature).toContain(name);
    }
  });
});

/** The path `crates/surface/src/lib.rs` LOCATED pins for each broken diagram fixture. */
const RUST_PATHS: Record<string, string> = Object.fromEntries(
  [
    ...readFileSync(new URL("../../../crates/surface/src/lib.rs", import.meta.url), "utf8").matchAll(
      /fixture!\("(broken-[\w-]+)"\),\s*"(\/elements\/[^"]+)"/g,
    ),
  ].map((m) => [m[1] as string, m[2] as string]),
);

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  for (const name of NAMES) setDiagramRenderer(name, null);
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

describe("diagram components render", () => {
  test("Gantt without a host shows its text form; with a host the wire props reach it", async () => {
    const el = await mount(
      `root = Gantt([{id: "a", label: "Kernel", start: "2026-10-08", end: "2026-10-10"}, {id: "b", label: "Charts", start: "2026-10-10", end: "2026-10-14", depends: ["a"]}], "2026-10-11", "Wave")`,
    );
    expect(el.querySelector("pre[data-diagram='Gantt']")?.textContent).toBe("Gantt: Wave — 2 tasks");
    let seen: DiagramHostProps | null = null;
    setDiagramRenderer("Gantt", (p: DiagramHostProps) => {
      seen = p;
      return <div data-host="gantt" />;
    });
    const el2 = await mount(`root = Gantt([{id: "a", label: "Kernel", start: "2026-10-08", end: "2026-10-10"}], null, null, ["a"])`);
    expect(el2.querySelector("[data-host='gantt']")).not.toBeNull();
    expect((seen as unknown as DiagramHostProps).highlight).toEqual(["a"]);
    expect(((seen as unknown as DiagramHostProps).props as { tasks: unknown[] }).tasks).toHaveLength(1);
  });

  test("diagramText singularises and prefixes the title", () => {
    expect(diagramText("Loop", { steps: [{ label: "x" }] })).toBe("Loop: 1 step");
    expect(diagramText("Entities", { entities: [{}] })).toBe("Entities: 1 entity");
    expect(diagramText("Venn", { title: "Gates", sets: [1, 2] })).toBe("Venn: Gates — 2 sets");
  });
});
