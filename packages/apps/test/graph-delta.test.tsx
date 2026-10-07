/**
 * `GraphDelta` through `AppRenderer` (design 123 §6, the `graph.test.tsx` shape): the wire props reach the installed host
 * renderer, `highlight` is reactive state, and a unit press runs `onSelect` with the unit id as `$value`.
 */
import "./dom.ts";
import { afterEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { type DiagramHostProps, library, setDiagramRenderer } from "../src/library.ts";
import { AppRenderer } from "../src/renderer.tsx";

const APP = `root = Stack([diagram, note, nav])
$step = 0
$selected = ""
lit = [["web"], ["cache"]]
diagram = GraphDelta({nodes: [{id: "web", label: "Web"}, {id: "db", label: "Postgres"}], edges: [{from: "web", to: "db"}]}, {nodes: [{id: "web", label: "Web"}, {id: "cache", label: "Redis", state: "done"}, {id: "db", label: "Postgres"}], edges: [{from: "web", to: "cache"}, {from: "cache", to: "db"}]}, "LR", "Add a cache", lit[$step], Action([@Set($selected, $value)]))
note = TextContent("Selected: " + $selected)
nav = Buttons([Button("Next", Action([@Set($step, 1)]))])`;

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  setDiagramRenderer("GraphDelta", null);
});

/** A stand-in host: one button per node, the highlight as text — what the desktop's `GraphDeltaView` receives. */
function TestHost({ props, highlight, onSelect }: DiagramHostProps<{ after: { nodes: { id: string; label: string }[] } }>) {
  return (
    <div data-host data-highlight={(highlight ?? []).join(",")}>
      {props.after.nodes.map((u) => (
        <button key={u.id} type="button" onClick={() => onSelect?.(u.id)}>
          {u.label}
        </button>
      ))}
    </div>
  );
}

describe("GraphDelta in an app", () => {
  test("highlight steps with $step; a unit press reaches @Set with the unit id", async () => {
    setDiagramRenderer("GraphDelta", TestHost);
    const el = document.createElement("div");
    document.body.appendChild(el);
    const root = createRoot(el);
    roots.push({ root, el });
    await act(async () => {
      root.render(<AppRenderer library={library} response={APP} isStreaming={false} generation={1} provider={() => Promise.resolve(null)} />);
    });
    const highlight = () => el.querySelector("[data-host]")!.getAttribute("data-highlight");
    const click = async (label: string) => {
      const btn = [...el.querySelectorAll("button")].find((b) => b.textContent === label)!;
      await act(async () => btn.click());
    };
    expect(highlight()).toBe("web");
    await click("Next");
    expect(highlight()).toBe("cache");
    await click("Redis");
    expect(el.textContent).toContain("Selected: cache");
  });
});
