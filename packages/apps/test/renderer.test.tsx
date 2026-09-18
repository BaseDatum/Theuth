/**
 * `AppRenderer` lifecycle under a real React mount (design 46 §6.2, §7): StrictMode effect
 * replay, pause before the first fetch, deferred generation adoption pinning the provider,
 * `$state` reconciliation across generations, and inert fallback frames.
 * Observes provider calls and DOM/state only; requires the react-lang patch (`bun install`).
 */
import "./dom.ts";
import { afterEach, describe, expect, test } from "bun:test";
import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { tagSchemaId } from "@openuidev/lang-core";
import {
  createLibrary,
  defineComponent,
  useTriggerAction,
} from "@openuidev/react-lang";
import { z } from "zod/v4";
import { AppRenderer, type AppRendererProps } from "../src/renderer.tsx";
import type { ProviderContext } from "../src/runtime-types.ts";

const actionSchema = z.object({ type: z.string() }).loose();
tagSchemaId(actionSchema, "ActionExpression");

const Text = defineComponent({
  name: "Text",
  props: z.object({ value: z.string() }),
  description: "text",
  component: ({ props }) => <span data-text>{props.value}</span>,
});
const Button = defineComponent({
  name: "Button",
  props: z.object({ label: z.string(), action: actionSchema.optional() }),
  description: "button",
  component: ({ props }) => {
    const trigger = useTriggerAction();
    return (
      <button
        type="button"
        onClick={() => trigger(props.label, undefined, props.action as never)}
      >
        {props.label}
      </button>
    );
  },
});
const Root = defineComponent({
  name: "Root",
  props: z.object({ children: z.array(z.union([Text.ref, Button.ref])) }),
  description: "root",
  component: ({ props, renderNode }) => (
    <div data-root>{renderNode(props.children)}</div>
  ),
});
const library = createLibrary({
  components: [Root, Text, Button],
  root: "Root",
});

interface Call {
  tool: string;
  args: unknown;
  context: ProviderContext;
  resolve: (value: unknown) => void;
  reject: (err: unknown) => void;
}

function recordingProvider() {
  const calls: Call[] = [];
  const provider: AppRendererProps["provider"] = (tool, args, context) => {
    const { promise, resolve, reject } = Promise.withResolvers<unknown>();
    calls.push({ tool, args, context, resolve, reject });
    return promise;
  };
  const live = () => calls.filter((c) => !c.context.signal?.aborted);
  return { calls, provider, live };
}

async function settle() {
  await act(async () => {
    for (let i = 0; i < 16; i++) await Promise.resolve();
  });
}

let roots: { root: Root; el: HTMLElement }[] = [];
afterEach(async () => {
  for (const { root, el } of roots) {
    await act(async () => root.unmount());
    el.remove();
  }
  roots = [];
});

type Props = Omit<AppRendererProps, "library">;

async function mount(props: Props, strict = true) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  roots.push({ root, el });
  const render = async (next: Props) => {
    const tree = <AppRenderer library={library} {...next} />;
    await act(async () => {
      root.render(strict ? <StrictMode>{tree}</StrictMode> : tree);
    });
  };
  await render(props);
  return {
    el,
    render,
    texts: () =>
      [...el.querySelectorAll("[data-text]")].map((n) => n.textContent),
    click: async (label: string) => {
      const btn = [...el.querySelectorAll("button")].find(
        (b) => b.textContent === label,
      );
      if (!btn) throw new Error(`no button "${label}"`);
      await act(async () => btn.click());
    },
  };
}

describe("StrictMode lifecycle", () => {
  test("the initial load survives effect replay: exactly one live call, and its result reaches the view", async () => {
    const { calls, provider, live } = recordingProvider();
    const runtime: AppRendererProps["runtimeRef"] = { current: null };
    const view = await mount({
      response:
        'root = Root([t])\nq = Query("fetch", {}, {v: "default"})\nt = Text(q.v)',
      isStreaming: false,
      generation: 1,
      provider,
      runtimeRef: runtime,
    });
    expect(view.texts()).toEqual(["default"]);
    expect(live()).toHaveLength(1);
    expect(live()[0]!.context).toMatchObject({
      statement_id: "q",
      trigger: "load",
    });
    expect(runtime.current!.query_status("q")!.state).toBe("loading");
    live()[0]!.resolve({ v: "loaded" });
    await settle();
    expect(view.texts()).toEqual(["loaded"]);
    expect(runtime.current!.query_status("q")).toMatchObject({
      state: "idle",
      has_data: true,
      stale: false,
    });
    // an aborted duplicate (if any) never reached the view
    for (const c of calls)
      if (c.context.signal?.aborted) c.resolve({ v: "late" });
    await settle();
    expect(view.texts()).toEqual(["loaded"]);
  });

  test("a paused mount and a paused generation change start no fetch; resuming loads once", async () => {
    const { calls, provider, live } = recordingProvider();
    const base: Props = {
      response: 'root = Root([t])\nq = Query("fetch", {})\nt = Text("" + q)',
      isStreaming: false,
      generation: 1,
      provider,
      paused: true,
    };
    const view = await mount(base);
    expect(calls).toHaveLength(0);
    await view.render({ ...base, generation: 2 });
    expect(calls).toHaveLength(0);
    await view.render({ ...base, generation: 2, paused: false });
    expect(live()).toHaveLength(1);
    expect(live()[0]!.context.trigger).toBe("load");
  });
});

describe("generation adoption", () => {
  test("a generation update deferred by a running action keeps the old provider and callbacks for the remaining steps and adopts the new ones afterwards", async () => {
    const gen1 = recordingProvider();
    const gen2 = recordingProvider();
    const events: string[] = [];
    const callbacks = (tag: string): Partial<Props> => ({
      onActionStart: (s) => events.push(`${tag}:start:${s.steps}`),
      onActionEnd: (r) => events.push(`${tag}:end:${r.ok}`),
      onAction: (e) => events.push(`${tag}:action:${e.type}`),
    });
    const base: Props = {
      response: [
        "root = Root([b])",
        'm1 = Mutation("save", {n: 1})',
        'm2 = Mutation("save", {n: 2})',
        'b = Button("Go", Action([@Run(m1), @Run(m2), @ToAssistant("done")]))',
      ].join("\n"),
      isStreaming: false,
      generation: 1,
      provider: gen1.provider,
      ...callbacks("v1"),
    };
    const view = await mount(base);
    await view.click("Go");
    expect(gen1.calls).toHaveLength(1);
    expect(gen1.calls[0]).toMatchObject({ tool: "save", args: { n: 1 } });
    expect(events).toEqual(["v1:start:3"]);

    await view.render({
      ...base,
      generation: 2,
      provider: gen2.provider,
      response:
        'root = Root([b])\nq = Query("fetch", {})\nb = Button("Ask", Action([@ToAssistant("" + q)]))',
      ...callbacks("v2"),
    });
    expect(gen2.calls).toHaveLength(0);
    gen1.calls[0]!.resolve({ ok: true });
    await settle();
    expect(gen1.calls).toHaveLength(2);
    expect(gen1.calls[1]).toMatchObject({ tool: "save", args: { n: 2 } });
    expect(gen1.calls[1]!.context.action_id).toBe(
      gen1.calls[0]!.context.action_id,
    );
    expect(gen2.calls).toHaveLength(0);
    gen1.calls[1]!.resolve({ ok: true });
    await settle();
    // the deferred frame's action finished against the callbacks passed with it
    expect(events).toEqual([
      "v1:start:3",
      "v1:action:continue_conversation",
      "v1:end:true",
    ]);
    expect(gen2.live()).toHaveLength(1);
    expect(gen2.live()[0]).toMatchObject({
      tool: "fetch",
      context: { statement_id: "q", trigger: "load" },
    });
    expect(gen1.calls).toHaveLength(2);
    // once adopted, the new frame dispatches to the new callbacks
    await view.click("Ask");
    expect(events.slice(3)).toEqual([
      "v2:start:1",
      "v2:action:continue_conversation",
      "v2:end:true",
    ]);
  });

  test("pressing another still-visible v1 control while v2 is deferred dispatches to v1's provider and callbacks, including implicit conversation actions", async () => {
    const gen1 = recordingProvider();
    const gen2 = recordingProvider();
    const events: string[] = [];
    const callbacks = (tag: string): Partial<Props> => ({
      onActionStart: (s) => events.push(`${tag}:start:${s.action_id}`),
      onActionEnd: (r) => events.push(`${tag}:end:${r.action_id}`),
      onAction: (e) =>
        events.push(
          `${tag}:action:${e.type}:${e.humanFriendlyMessage}:${e.action_id}`,
        ),
    });
    const base: Props = {
      response: [
        "root = Root([slow, fast, ask])",
        'm1 = Mutation("save", {n: 1})',
        'm2 = Mutation("save", {n: 2})',
        'slow = Button("Slow", Action([@Run(m1)]))',
        'fast = Button("Fast", Action([@Run(m2), @ToAssistant("fast done")]))',
        'ask = Button("Ask", {type: "continue_conversation"})',
      ].join("\n"),
      isStreaming: false,
      generation: 1,
      provider: gen1.provider,
      ...callbacks("v1"),
    };
    const view = await mount(base);
    await view.click("Slow");
    expect(gen1.calls).toHaveLength(1);
    const slowId = gen1.calls[0]!.context.action_id!;
    expect(events).toEqual([`v1:start:${slowId}`]);

    await view.render({
      ...base,
      generation: 2,
      provider: gen2.provider,
      response: 'root = Root([t])\nt = Text("v2")',
      ...callbacks("v2"),
    });
    // v1 is still on screen
    expect(view.el.querySelectorAll("button")).toHaveLength(3);

    await view.click("Fast");
    expect(gen1.calls).toHaveLength(2);
    expect(gen1.calls[1]).toMatchObject({ tool: "save", args: { n: 2 } });
    const fastId = gen1.calls[1]!.context.action_id!;
    expect(fastId).not.toBe(slowId);
    expect(events).toEqual([`v1:start:${slowId}`, `v1:start:${fastId}`]);
    gen1.calls[1]!.resolve({ ok: true });
    await settle();
    expect(events.slice(2)).toEqual([
      `v1:action:continue_conversation:fast done:${fastId}`,
      `v1:end:${fastId}`,
    ]);
    // implicit (non-plan) conversation action while still deferred
    await view.click("Ask");
    const askEvent = events.at(-1)!;
    expect(askEvent.startsWith("v1:action:continue_conversation:Ask:")).toBe(
      true,
    );
    expect(askEvent.endsWith(":undefined")).toBe(false);
    expect(view.el.querySelectorAll("button")).toHaveLength(3);
    expect(gen2.calls).toHaveLength(0);

    gen1.calls[0]!.resolve({ ok: true });
    await settle();
    expect(events.at(-1)).toBe(`v1:end:${slowId}`);
    expect(events.filter((e) => e.startsWith("v2:"))).toEqual([]);
    expect(view.texts()).toEqual(["v2"]);
    expect(view.el.querySelectorAll("button")).toHaveLength(0);
  });

  test("$state survives only for unchanged declarations: changed defaults reset, deleted names are cleared and come back with their default", async () => {
    const { provider } = recordingProvider();
    const program = (target: string | null, keep = '"k"') =>
      [
        "root = Root([t, b])",
        ...(target === null ? [] : [`$target = ${target}`]),
        `$keep = ${keep}`,
        target === null
          ? 'b = Button("Edit", Action([@Set($keep, "kept-edit")]))'
          : 'b = Button("Edit", Action([@Set($target, "edited"), @Set($keep, "kept-edit")]))',
        target === null ? "t = Text($keep)" : 't = Text($target + "|" + $keep)',
      ].join("\n");
    const base: Props = {
      response: program('"dev"'),
      isStreaming: false,
      generation: 1,
      provider,
    };
    const view = await mount(base);
    await view.click("Edit");
    expect(view.texts()).toEqual(["edited|kept-edit"]);

    await view.render({ ...base, generation: 2, response: program('"prod"') });
    expect(view.texts()).toEqual(["prod|kept-edit"]);

    await view.render({ ...base, generation: 3, response: program(null) });
    expect(view.texts()).toEqual(["kept-edit"]);

    await view.render({ ...base, generation: 4, response: program('"dev"') });
    expect(view.texts()).toEqual(["dev|kept-edit"]);
  });

  test("a streamed preview does not reset edits of declarations the commit leaves unchanged", async () => {
    const { provider } = recordingProvider();
    const src = (extra: string) =>
      [
        "root = Root([t, b])",
        '$target = "dev"',
        'b = Button("Edit", Action([@Set($target, "edited")]))',
        "t = Text($target)",
        extra,
      ].join("\n");
    const base: Props = {
      response: src(""),
      isStreaming: false,
      generation: 1,
      provider,
    };
    const view = await mount(base);
    await view.click("Edit");
    expect(view.texts()).toEqual(["edited"]);
    await view.render({
      ...base,
      isStreaming: true,
      response: "root = Root([t",
    });
    await view.render({
      ...base,
      isStreaming: true,
      response: 'root = Root([t, b])\n$target = "dev"\nb = Button("Edit"',
    });
    await view.render({
      ...base,
      generation: 2,
      response: src('$extra = "x"'),
    });
    expect(view.texts()).toEqual(["edited"]);
  });

  test("a new generation that exceeds the budget shows nothing rather than the old generation's controls", async () => {
    const { calls, provider, live } = recordingProvider();
    const errors: { code: string }[][] = [];
    const rows = Array.from({ length: 3000 }, (_, i) => i);
    const base: Props = {
      response: [
        "root = Root([b])",
        "$rows = []",
        'q = Query("fetch", {})',
        'm = Mutation("save", {n: 1})',
        'b = Button("Go", Action([@Set($rows, q), @Run(m)]))',
      ].join("\n"),
      isStreaming: false,
      generation: 1,
      provider,
      onError: (e) => errors.push(e.map(({ code }) => ({ code }))),
    };
    const view = await mount(base);
    live()[0]!.resolve(rows);
    await settle();
    await view.click("Go");
    expect(calls.at(-1)).toMatchObject({ tool: "save", args: { n: 1 } });
    calls.at(-1)!.resolve({ ok: true });
    await settle();

    await view.render({
      ...base,
      generation: 2,
      response: [
        'root = Root(@Each($rows, "r", Text("" + r)))',
        "$rows = []",
        'm = Mutation("delete", {n: 2})',
        'b = Button("Go", Action([@Run(m)]))',
      ].join("\n"),
    });
    expect(view.el.querySelectorAll("button")).toHaveLength(0);
    expect(errors.at(-1)).toEqual([{ code: "budget-exceeded" }]);
    expect(calls.filter((c) => c.tool === "delete")).toHaveLength(0);
  });
});
