/**
 * The apps `Graph` (design 65 §3.5): the zod schema accepts and rejects the surface crate's fixtures for the
 * same reasons; the walkthrough app publishes, steps through its three states and a node press reaches
 * `@Set` and `@ToAssistant`. Rendered without a host renderer: the component's adjacency text stands in.
 */
import "./dom.ts";
import { readFileSync, readdirSync } from "node:fs";
import { afterEach, describe, expect, test } from "bun:test";
import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { check } from "../src/check.ts";
import {
  type GraphHostProps,
  graphProps,
  library,
  reactiveProps,
  setGraphRenderer,
} from "../src/library.ts";
import { AppRenderer, type AppRendererProps } from "../src/renderer.tsx";

const HERE = new URL("./fixtures/graph/", import.meta.url);
const SURFACE = new URL(
  "../../../crates/surface/fixtures/json-render/",
  import.meta.url,
);
const read = (dir: URL, name: string) =>
  JSON.parse(readFileSync(new URL(`${name}.json`, dir), "utf8"));

describe("Graph schema parity with crates/surface", () => {
  test("the fixture props are the surface fixtures' Graph props", () => {
    const names = readdirSync(HERE)
      .filter((f) => f.endsWith(".json") && f !== "graph-broken-over-cap.json")
      .map((f) => f.slice(0, -5));
    expect(names).toHaveLength(5);
    for (const n of names)
      expect(read(HERE, n), n).toEqual(read(SURFACE, n).elements.g.props);
  });

  test.each(["graph-architecture", "graph-dependency-lr", "graph-pipeline-tb"])(
    "%s is accepted",
    (name) => {
      expect(graphProps.safeParse(read(HERE, name)).success).toBeTrue();
    },
  );

  test("the broken fixtures are rejected at the surface crate's paths", () => {
    const first = (name: string) => {
      const r = graphProps.safeParse(read(HERE, name));
      const i = r.error!.issues[0]!;
      return `${i.path.join("/")}: ${i.message}`;
    };
    expect(first("graph-broken-unknown-endpoint")).toBe(
      'edges/1/to: unknown node "db2"',
    );
    expect(first("graph-broken-dup-id")).toBe(
      'nodes/2/id: duplicate node id "api"',
    );
    expect(first("graph-broken-over-cap")).toBe("nodes: 121 nodes, limit 120");
  });

  test("highlight is the reactive prop; Graph closes every container child union", () => {
    expect(reactiveProps(library).Graph).toEqual(["highlight"]);
    expect(library.toSpec().components.Card!.signature).toContain("Graph");
    const sig = library.toSpec().components.Graph!.signature;
    expect(sig).toStartWith(
      'Graph(nodes: {id: string, label: string, kind?: "box" | "rounded" | "pill" | "diamond" | "cylinder" | "actor" | "hexagon" | "circle" | "note", group?: string, state?: "default" | "active" | "done" | "muted" | "danger", sub?: string, tag?: string, icon?: "server" |',
    );
    expect(sig).toContain(
      '}[], edges: {from: string, to: string, label?: string, style?: "solid" | "dashed", state?: "default" | "active" | "done" | "muted" | "danger", arrow?: "forward" | "both" | "none"}[], groups?: {id: string, label: string, parent?: string, kind?: "zone" | "boundary" | "cluster"}[], ',
    );
    expect(sig).toContain('direction?: "LR" | "TB", highlight?: $binding<string[]>, onSelect?: ActionExpression, legend?: {');
    expect(sig).toEndWith(', title?: string)');
  });
});

/** Design 65 §3.5 walkthrough, in the form the spike verified (`steps[$step].ids`, `$value` in onSelect). */
export const walkthrough = `root = Stack([title, graph, narrative, nav])
$step = 0
$selected = ""
title = TextContent("## One request")
steps = [{ids: ["client"], text: "The client sends the request."}, {ids: ["client", "api"], text: "The API authenticates it."}, {ids: ["api", "db"], text: "The API reads the record."}]
nodes = [{id: "client", label: "Client", kind: "actor"}, {id: "api", label: "API", kind: "rounded"}, {id: "db", label: "Postgres", kind: "cylinder"}]
edges = [{from: "client", to: "api", label: "HTTPS"}, {from: "api", to: "db", label: "reads"}]
graph = Graph(nodes, edges, null, "LR", steps[$step].ids, Action([@Set($selected, $value), @ToAssistant("Explain the selected node", $selected)]))
narrative = TextContent(steps[$step].text + ($selected == "" ? "" : " Selected: " + $selected))
nav = Buttons([Button("Back", Action([@Set($step, $step > 0 ? $step - 1 : 0)]), "secondary"), Button("Next", Action([@Set($step, $step < 2 ? $step + 1 : 2)]))])`;

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  setGraphRenderer(null);
});

/** A stand-in host: one button per node, the highlight as text — what the desktop's `GraphView` receives. */
function TestHost({ props, highlight, onSelect }: GraphHostProps) {
  return (
    <div data-graph-host data-highlight={(highlight ?? []).join(",")}>
      {props.nodes.map((n) => (
        <button key={n.id} type="button" onClick={() => onSelect?.(n.id)}>
          {n.label}
        </button>
      ))}
    </div>
  );
}

async function mount(props: Omit<AppRendererProps, "library">) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  roots.push({ root, el });
  await act(async () => {
    root.render(
      <StrictMode>
        <AppRenderer library={library} {...props} />
      </StrictMode>,
    );
  });
  const click = async (label: string) => {
    const btn = [...el.querySelectorAll("button")].find(
      (b) => b.textContent === label,
    );
    if (!btn) throw new Error(`no button "${label}"`);
    await act(async () => btn.click());
  };
  return { el, click };
}

describe("walkthrough app", () => {
  test("publishes: steps[$step].ids resolves and $value is scoped to onSelect", () => {
    const r = check(walkthrough, []);
    expect(r.diagnostics).toEqual([]);
    expect(r.statements.map((s) => s.id)).toContain("graph");
    // `$value` outside a Graph onSelect is still undeclared state.
    const stray = check(
      `root = Stack([b])\nb = Button("x", Action([@Set($v, $value)]))\n$v = ""`,
      [],
    );
    expect(stray.diagnostics.map((d) => d.code)).toEqual(["state-undeclared"]);
  });

  test("steps through three highlight states and a node press reaches @Set and @ToAssistant", async () => {
    setGraphRenderer(TestHost);
    const actions: { type: string; params?: Record<string, unknown>; message?: string }[] = [];
    const view = await mount({
      response: walkthrough,
      isStreaming: false,
      generation: 1,
      provider: () => Promise.resolve(null),
      onAction: (e) =>
        actions.push({
          type: e.type,
          params: e.params as Record<string, unknown> | undefined,
          message: (e as { humanFriendlyMessage?: string }).humanFriendlyMessage,
        }),
    });
    const highlight = () =>
      view.el.querySelector("[data-graph-host]")!.getAttribute("data-highlight");
    const narrative = () =>
      [...view.el.querySelectorAll("p, div")]
        .map((n) => n.textContent ?? "")
        .find((t) => t.startsWith("The ")) ?? "";
    expect(highlight()).toBe("client");
    expect(narrative()).toContain("The client sends the request.");
    await view.click("Next");
    expect(highlight()).toBe("client,api");
    expect(narrative()).toContain("The API authenticates it.");
    await view.click("Next");
    expect(highlight()).toBe("api,db");
    await view.click("Next");
    expect(highlight()).toBe("api,db"); // clamped at the last step
    await view.click("Back");
    expect(highlight()).toBe("client,api");

    await view.click("Postgres");
    expect(narrative()).toContain("Selected: db");
    expect(actions.map((a) => a.type)).toEqual(["continue_conversation"]);
    expect(actions[0]!.message).toBe("Explain the selected node");
  });

  test("without a host renderer the component shows the adjacency text", async () => {
    const view = await mount({
      response: walkthrough,
      isStreaming: false,
      generation: 1,
      provider: () => Promise.resolve(null),
    });
    expect(view.el.querySelector("pre[data-graph]")!.textContent).toBe(
      "• Client → API (HTTPS)\n• API → Postgres (reads)\n• Postgres",
    );
  });
});
