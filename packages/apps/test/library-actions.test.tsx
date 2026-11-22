/**
 * The typed library action and the bare single-step syntax reach the host through the real curated library:
 * `@CopyToClipboard(text)` arrives in `onAction` as `{ type: "CopyToClipboard", params: { text } }`.
 */
import "./dom.ts";
import { afterEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { library } from "../src/library.ts";
import { AppRenderer } from "../src/renderer.tsx";
import type { AppActionEvent } from "../src/runtime-types.ts";

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
});

async function mount(response: string, events: AppActionEvent[]) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  roots.push({ root, el });
  await act(async () => {
    root.render(
      <AppRenderer
        library={library}
        response={response}
        isStreaming={false}
        generation={1}
        provider={() => Promise.resolve(null)}
        onAction={(e) => events.push(e)}
      />,
    );
  });
  return async (label: string) => {
    const btn = [...el.querySelectorAll("button")].find((b) => b.textContent?.includes(label));
    if (!btn) throw new Error(`no button "${label}"`);
    await act(async () => btn.click());
  };
}

describe("library actions at the host boundary", () => {
  test("@CopyToClipboard is delivered as a typed custom action with the text param", async () => {
    const events: AppActionEvent[] = [];
    const press = await mount(
      ['root = Card([copy])', '$id = "api"', 'copy = Button("Copy id", @CopyToClipboard("id: " + $id))'].join("\n"),
      events,
    );
    await press("Copy id");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ type: "CopyToClipboard", params: { text: "id: api" } });
  });

  test("a bare @ToAssistant is a whole action: it reaches the host as continue_conversation", async () => {
    const events: AppActionEvent[] = [];
    const press = await mount('root = Card([ask])\nask = Button("Ask", @ToAssistant("Tell me more"))', events);
    await press("Ask");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ type: "continue_conversation", humanFriendlyMessage: "Tell me more" });
  });

  test("a pure function renders into text", async () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const root = createRoot(el);
    roots.push({ root, el });
    await act(async () => {
      root.render(
        <AppRenderer
          library={library}
          response={'root = Card([t])\nt = TextContent(@FormatNumber(1234.5, "standard", 2, "USD") + " · " + @Percent(1, 4))'}
          isStreaming={false}
          generation={1}
          provider={() => Promise.resolve(null)}
        />,
      );
    });
    expect(el.textContent).toContain("$1,234.50 · 25%");
  });
});
