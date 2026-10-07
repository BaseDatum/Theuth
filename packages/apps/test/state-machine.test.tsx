/**
 * `StateMachine` through `AppRenderer` (design 123 §6, the `graph.test.tsx` shape): the wire props reach the installed host
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
lit = [["draft"], ["review"]]
diagram = StateMachine([{id: "draft", label: "Draft", kind: "initial"}, {id: "review", label: "Review"}, {id: "done", label: "Done", kind: "final"}], [{from: "draft", to: "review", label: "submit"}, {from: "review", to: "done", label: "approve", guard: "all green"}], "LR", "Lifecycle", lit[$step], Action([@Set($selected, $value)]))
note = TextContent("Selected: " + $selected)
nav = Buttons([Button("Next", Action([@Set($step, 1)]))])`;

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  setDiagramRenderer("StateMachine", null);
});

/** A stand-in host: one button per state, the highlight as text — what the desktop's `StateMachineView` receives. */
function TestHost({ props, highlight, onSelect }: DiagramHostProps<{ states: { id: string; label: string }[] }>) {
  return (
    <div data-host data-highlight={(highlight ?? []).join(",")}>
      {props.states.map((u) => (
        <button key={u.id} type="button" onClick={() => onSelect?.(u.id)}>
          {u.label}
        </button>
      ))}
    </div>
  );
}

describe("StateMachine in an app", () => {
  test("highlight steps with $step; a unit press reaches @Set with the unit id", async () => {
    setDiagramRenderer("StateMachine", TestHost);
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
    expect(highlight()).toBe("draft");
    await click("Next");
    expect(highlight()).toBe("review");
    await click("Done");
    expect(el.textContent).toContain("Selected: done");
  });
});
