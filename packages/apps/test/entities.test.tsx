/**
 * `Entities` through `AppRenderer` (design 123 §6, the `graph.test.tsx` shape): the wire props reach the installed host
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
lit = [["user"], ["session"]]
diagram = Entities([{id: "user", label: "users", fields: [{name: "id", key: "pk"}]}, {id: "session", label: "sessions", fields: [{name: "user_id", key: "fk"}]}], [{from: "user", to: "session", kind: "one-many", from_field: "id", to_field: "user_id"}], "schema", "Schema", lit[$step], Action([@Set($selected, $value)]))
note = TextContent("Selected: " + $selected)
nav = Buttons([Button("Next", Action([@Set($step, 1)]))])`;

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
  setDiagramRenderer("Entities", null);
});

/** A stand-in host: one button per entity, the highlight as text — what the desktop's `EntitiesView` receives. */
function TestHost({ props, highlight, onSelect }: DiagramHostProps<{ entities: { id: string; label: string }[] }>) {
  return (
    <div data-host data-highlight={(highlight ?? []).join(",")}>
      {props.entities.map((u) => (
        <button key={u.id} type="button" onClick={() => onSelect?.(u.id)}>
          {u.label}
        </button>
      ))}
    </div>
  );
}

describe("Entities in an app", () => {
  test("highlight steps with $step; a unit press reaches @Set with the unit id", async () => {
    setDiagramRenderer("Entities", TestHost);
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
    expect(highlight()).toBe("user");
    await click("Next");
    expect(highlight()).toBe("session");
    await click("sessions");
    expect(el.textContent).toContain("Selected: session");
  });
});
