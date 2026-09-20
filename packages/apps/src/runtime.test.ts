/**
 * Runtime contract of the patched react-lang query manager / action runner (design 46 §6.2, §7).
 * Requires the react-lang patch to be applied (`bun install` with patchedDependencies).
 */
import { afterEach, beforeEach, describe, expect, test, vi } from "bun:test";
import { createStore } from "@openuidev/lang-core";
import type {
  EvaluationContext,
  ParseResult,
  Store,
} from "@openuidev/lang-core";
import {
  STATE_CHANGE_DEBOUNCE_MS,
  createQueryManager,
  runActionPlan,
} from "@openuidev/react-lang/theuth-runtime";
import type {
  QueryManager,
  ToolCallMeta,
} from "@openuidev/react-lang/theuth-runtime";
import { toProviderContext } from "./renderer.tsx";

interface Call {
  tool: string;
  args: Record<string, unknown>;
  meta: ToolCallMeta;
  resolve: (value: unknown) => void;
  reject: (err: unknown) => void;
}

/** Provider that records every call and lets the test settle each one explicitly. */
function recordingProvider() {
  const calls: Call[] = [];
  return {
    calls,
    provider: {
      callTool(
        tool: string,
        args: Record<string, unknown>,
        meta: ToolCallMeta,
      ) {
        const { promise, resolve, reject } = Promise.withResolvers<unknown>();
        calls.push({ tool, args, meta, resolve, reject });
        return promise;
      },
    },
  };
}

/** Drain the microtask chain behind a settled provider promise (no timers involved). */
async function settle() {
  for (let i = 0; i < 16; i++) await Promise.resolve();
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function queryNode(
  statementId: string,
  args: Record<string, unknown>,
  refreshInterval?: number,
) {
  return {
    statementId,
    toolName: "fetch",
    args,
    defaults: null,
    deps: undefined,
    refreshInterval,
    complete: true,
  };
}

function contextFor(store: Store, qm: QueryManager): EvaluationContext {
  return {
    getState: (name) => store.get(name),
    resolveRef: (name) => qm.getMutationResult(name) ?? qm.getResult(name),
  };
}

describe("query provenance", () => {
  test("load, pressed refetch and later refresh carry distinct triggers; refresh never inherits the press", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", { a: 1 })]);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.meta).toMatchObject({ statementId: "q", trigger: "load" });
    expect(calls[0]!.meta.actionId).toBeUndefined();
    calls[0]!.resolve({ v: 1 });
    await settle();

    qm.invalidate(["q"], "act-1");
    expect(calls[1]!.meta).toMatchObject({
      trigger: "press",
      actionId: "act-1",
    });
    calls[1]!.resolve({ v: 2 });
    await settle();

    qm.invalidate(["q"]);
    expect(calls[2]!.meta.trigger).toBe("refresh");
    expect(calls[2]!.meta.actionId).toBeUndefined();
    qm.dispose();
  });

  test("a press queued behind an in-flight fetch keeps its action id; only one runs at a time", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", {})]);
    qm.invalidate(["q"], "act-2");
    qm.invalidate(["q"], "act-3");
    expect(calls).toHaveLength(1);
    calls[0]!.resolve(1);
    await settle();
    expect(calls).toHaveLength(2);
    expect(calls[1]!.meta).toMatchObject({
      trigger: "press",
      actionId: "act-3",
    });
    qm.dispose();
  });

  test("changed arguments coalesce for the debounce window and fire one state_change fetch", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", { n: 1 })]);
    calls[0]!.resolve("one");
    await settle();
    qm.evaluateQueries([queryNode("q", { n: 2 })]);
    qm.evaluateQueries([queryNode("q", { n: 3 })]);
    expect(calls).toHaveLength(1);
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    expect(calls).toHaveLength(2);
    expect(calls[1]!.args).toEqual({ n: 3 });
    expect(calls[1]!.meta.trigger).toBe("state_change");
    qm.dispose();
  });

  test("refresh interval is floored to 5 s", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", {}, 1)]);
    calls[0]!.resolve(1);
    await settle();
    vi.advanceTimersByTime(4999);
    expect(calls).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(calls).toHaveLength(2);
    expect(calls[1]!.meta).toMatchObject({ trigger: "refresh" });
    expect(calls[1]!.meta.actionId).toBeUndefined();
    qm.dispose();
  });
});

describe("stale and obsolete results", () => {
  test("a result for superseded arguments is discarded; previous data is shown as stale meanwhile", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", { page: 1 })]);
    calls[0]!.resolve("page-1");
    await settle();
    expect(qm.getResult("q")).toBe("page-1");

    qm.evaluateQueries([queryNode("q", { page: 2 })]);
    expect(qm.getQueryStatus("q")).toMatchObject({
      state: "idle",
      hasData: true,
      stale: true,
    });
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    expect(qm.getQueryStatus("q")).toMatchObject({
      state: "refetching",
      stale: true,
    });

    qm.evaluateQueries([queryNode("q", { page: 3 })]);
    expect(calls[1]!.meta.signal?.aborted).toBe(true);
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    calls[1]!.resolve("page-2");
    await settle();
    expect(qm.getResult("q")).toBe("page-1");
    calls[2]!.resolve("page-3");
    await settle();
    expect(qm.getResult("q")).toBe("page-3");
    expect(qm.getQueryStatus("q")).toMatchObject({
      state: "idle",
      stale: false,
    });
    qm.dispose();
  });

  test("a failed refetch keeps the last data, marks it stale and exposes the raw cause for retry controls", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", {})]);
    calls[0]!.resolve("good");
    await settle();
    const cause = Object.assign(new Error("approval required"), {
      code: "needs_approval",
    });
    qm.invalidate(["q"]);
    calls[1]!.reject(cause);
    await settle();
    expect(qm.getResult("q")).toBe("good");
    const status = qm.getQueryStatus("q")!;
    expect(status).toMatchObject({
      state: "error",
      hasData: true,
      stale: true,
    });
    expect(status.cause).toBe(cause);
    expect(qm.getSnapshot().__openui_errors[0]).toMatchObject({
      statementId: "q",
      code: "tool-error",
    });
    qm.dispose();
  });

  test("pause defers the latest desired fetch; resume runs it once", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", {})]);
    calls[0]!.resolve(1);
    await settle();
    qm.pause();
    qm.invalidate(["q"]);
    qm.invalidate(["q"], "act-4");
    expect(calls).toHaveLength(1);
    qm.resume();
    expect(calls).toHaveLength(2);
    expect(calls[1]!.meta).toMatchObject({
      trigger: "press",
      actionId: "act-4",
    });
    qm.dispose();
  });

  test("results arriving after dispose are ignored", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    let notified = 0;
    qm.subscribe(() => notified++);
    qm.evaluateQueries([queryNode("q", {})]);
    const before = notified;
    qm.dispose();
    expect(calls[0]!.meta.signal?.aborted).toBe(true);
    calls[0]!.resolve("late");
    await settle();
    expect(notified).toBe(before);
    expect(qm.getResult("q")).toBeNull();
  });

  test("returning to cached arguments while the newer fetch is in flight ends loading; the aborted call is inert", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", { page: 1 })]);
    calls[0]!.resolve("page-1");
    await settle();
    qm.evaluateQueries([queryNode("q", { page: 2 })]);
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    expect(qm.getQueryStatus("q")!.state).toBe("refetching");

    qm.evaluateQueries([queryNode("q", { page: 1 })]);
    expect(calls[1]!.meta.signal?.aborted).toBe(true);
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    expect(calls).toHaveLength(2);
    expect(qm.getResult("q")).toBe("page-1");
    expect(qm.getQueryStatus("q")).toMatchObject({
      state: "idle",
      stale: false,
    });
    expect(qm.getSnapshot().__openui_loading).toEqual([]);
    // the superseded promise never settled: page 2 must still be fetchable
    qm.evaluateQueries([queryNode("q", { page: 2 })]);
    vi.advanceTimersByTime(STATE_CHANGE_DEBOUNCE_MS);
    expect(calls).toHaveLength(3);
    expect(calls[2]!.args).toEqual({ page: 2 });
    qm.dispose();
  });

  test("reactivation after dispose restarts a load that was aborted before settling", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    qm.evaluateQueries([queryNode("q", {})]);
    qm.dispose();
    expect(calls[0]!.meta.signal?.aborted).toBe(true);
    qm.activate();
    qm.evaluateQueries([queryNode("q", {})]);
    expect(calls).toHaveLength(2);
    expect(calls[1]!.meta).toMatchObject({ trigger: "load" });
    expect(calls[1]!.meta.signal?.aborted).toBe(false);
    calls[1]!.resolve("data");
    await settle();
    expect(qm.getResult("q")).toBe("data");
    expect(qm.getQueryStatus("q")).toMatchObject({
      state: "idle",
      hasData: true,
    });
    // settled data is not reloaded by a further cleanup/setup cycle
    qm.dispose();
    qm.activate();
    expect(calls).toHaveLength(2);
    qm.dispose();
  });

  test("a manager born paused defers its first load until resume", () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider, { paused: true });
    qm.evaluateQueries([queryNode("q", {})]);
    expect(calls).toHaveLength(0);
    expect(qm.isPaused()).toBe(true);
    qm.resume();
    expect(calls).toHaveLength(1);
    expect(calls[0]!.meta.trigger).toBe("load");
    qm.dispose();
  });
});

describe("action scope", () => {
  test("one activation threads one action id through sequential @Run steps and captures inputs at activation", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    store.initialize({ $note: "first" }, {});
    qm.registerMutations([
      { statementId: "m1", toolName: "save" },
      { statementId: "m2", toolName: "save" },
    ]);
    qm.evaluateQueries([queryNode("q", {})]);
    calls[0]!.resolve("data");
    await settle();

    const parse = {
      mutationStatements: [
        {
          statementId: "m1",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [["note", { k: "StateRef", n: "$note" }]],
          },
        },
        {
          statementId: "m2",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [["note", { k: "StateRef", n: "$note" }]],
          },
        },
      ],
      stateDeclarations: {},
    } as unknown as ParseResult;
    const starts: string[] = [];
    const ends: { actionId: string; ok: boolean }[] = [];
    const done = runActionPlan({
      plan: {
        steps: [
          { type: "run", refType: "mutation", statementId: "m1" },
          { type: "run", refType: "mutation", statementId: "m2" },
          { type: "run", refType: "query", statementId: "q" },
        ],
      },
      userMessage: "Save",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => parse,
      onActionStart: (e) => starts.push(e.actionId),
      onActionEnd: (e) => ends.push({ actionId: e.actionId, ok: e.ok }),
    });
    await settle();
    expect(starts).toHaveLength(1);
    const actionId = starts[0]!;
    expect(calls[1]!.meta).toMatchObject({
      statementId: "m1",
      trigger: "press",
      actionId,
    });
    store.set("$note", "changed-after-press");
    calls[1]!.resolve({ ok: true });
    await settle();
    expect(calls[2]!.meta).toMatchObject({
      statementId: "m2",
      trigger: "press",
      actionId,
    });
    expect(calls[2]!.args).toEqual({ note: "first" });
    calls[2]!.resolve({ ok: true });
    await done;
    expect(calls[3]!.meta).toMatchObject({
      statementId: "q",
      trigger: "press",
      actionId,
    });
    expect(ends).toEqual([{ actionId, ok: true }]);

    calls[3]!.resolve("data");
    await settle();
    qm.invalidate(["q"]);
    expect(calls[4]!.meta.trigger).toBe("refresh");
    expect(calls[4]!.meta.actionId).toBeUndefined();
    qm.dispose();
  });

  test("a failed mutation stops the plan, is not retried, and reports ok=false", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    qm.registerMutations([
      { statementId: "m1", toolName: "save" },
      { statementId: "m2", toolName: "save" },
    ]);
    const ends: boolean[] = [];
    const done = runActionPlan({
      plan: {
        steps: [
          { type: "run", refType: "mutation", statementId: "m1" },
          { type: "run", refType: "mutation", statementId: "m2" },
        ],
      },
      userMessage: "Save",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => null,
      onActionEnd: (e) => ends.push(e.ok),
    });
    await settle();
    calls[0]!.reject(new Error("remote refused"));
    await done;
    expect(calls).toHaveLength(1);
    expect(ends).toEqual([false]);
    expect(qm.getMutationResult("m1")).toMatchObject({
      status: "error",
      error: "remote refused",
    });
    expect(qm.getMutationResult("m2")).toMatchObject({ status: "idle" });
    qm.dispose();
  });

  test("two activations never share an action id", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    qm.evaluateQueries([queryNode("q", {})]);
    calls[0]!.resolve(1);
    await settle();
    const plan = {
      steps: [
        { type: "run" as const, refType: "query" as const, statementId: "q" },
      ],
    };
    const common = {
      plan,
      userMessage: "",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => null,
    };
    await runActionPlan(common);
    calls[1]!.resolve(2);
    await settle();
    await runActionPlan(common);
    expect(calls[1]!.meta.actionId).toBeDefined();
    expect(calls[2]!.meta.actionId).toBeDefined();
    expect(calls[1]!.meta.actionId).not.toBe(calls[2]!.meta.actionId);
    qm.dispose();
  });

  test("a @Set before @Run is visible to that mutation, a repeated @Run around a @Set sends step-specific arguments, and outside edits after activation are not", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    store.initialize({ $id: "old" }, {});
    qm.registerMutations([{ statementId: "m", toolName: "save" }]);
    const parse = {
      mutationStatements: [
        {
          statementId: "m",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [["id", { k: "StateRef", n: "$id" }]],
          },
        },
      ],
      stateDeclarations: { $id: "old" },
    } as unknown as ParseResult;
    const done = runActionPlan({
      plan: {
        steps: [
          { type: "run", refType: "mutation", statementId: "m" },
          { type: "set", target: "$id", valueAST: { k: "Str", v: "new" } },
          { type: "run", refType: "mutation", statementId: "m" },
          { type: "reset", targets: ["$id"] },
          { type: "run", refType: "mutation", statementId: "m" },
        ],
      } as never,
      userMessage: "Save",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => parse,
    });
    await settle();
    expect(calls[0]!.args).toEqual({ id: "old" });
    expect(store.get("$id")).toBe("old");
    store.set("$id", "typed-meanwhile");
    calls[0]!.resolve({ ok: true });
    await settle();
    expect(store.get("$id")).toBe("new");
    expect(calls[1]!.args).toEqual({ id: "new" });
    calls[1]!.resolve({ ok: true });
    await settle();
    expect(store.get("$id")).toBe("old");
    expect(calls[2]!.args).toEqual({ id: "old" });
    calls[2]!.resolve({ ok: true });
    await done;
    expect(calls).toHaveLength(3);
    qm.dispose();
  });

  test("row selection: @Set then @Run sends the selected id", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    store.initialize({ $id: "old" }, {});
    qm.registerMutations([{ statementId: "m", toolName: "save" }]);
    const parse = {
      mutationStatements: [
        {
          statementId: "m",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [["id", { k: "StateRef", n: "$id" }]],
          },
        },
      ],
      stateDeclarations: {},
    } as unknown as ParseResult;
    const done = runActionPlan({
      plan: {
        steps: [
          { type: "set", target: "$id", valueAST: { k: "Str", v: "row-7" } },
          { type: "run", refType: "mutation", statementId: "m" },
        ],
      } as never,
      userMessage: "Select",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => parse,
    });
    await settle();
    expect(calls[0]!.args).toEqual({ id: "row-7" });
    expect(store.get("$id")).toBe("row-7");
    calls[0]!.resolve({ ok: true });
    await done;
    qm.dispose();
  });

  test("steps after @Run see that mutation's returned data while $state stays frozen at activation", async () => {
    const { calls, provider } = recordingProvider();
    const qm = createQueryManager(provider);
    const store = createStore();
    store.initialize({ $id: "", $name: "draft" }, {});
    qm.registerMutations([
      { statementId: "create", toolName: "create" },
      { statementId: "tag", toolName: "tag" },
    ]);
    const member = (obj: unknown, field: string) => ({
      k: "Member",
      obj,
      field,
    });
    const parse = {
      mutationStatements: [
        {
          statementId: "create",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [["name", { k: "StateRef", n: "$name" }]],
          },
        },
        {
          statementId: "tag",
          toolAST: null,
          argsAST: {
            k: "Obj",
            entries: [
              [
                "id",
                member(member({ k: "RuntimeRef", n: "create" }, "data"), "id"),
              ],
              ["name", { k: "StateRef", n: "$name" }],
            ],
          },
        },
      ],
      stateDeclarations: {},
    } as unknown as ParseResult;
    const done = runActionPlan({
      plan: {
        steps: [
          { type: "run", refType: "mutation", statementId: "create" },
          {
            type: "set",
            target: "$id",
            valueAST: member(
              member({ k: "RuntimeRef", n: "create" }, "data"),
              "id",
            ),
          },
          { type: "run", refType: "mutation", statementId: "tag" },
        ],
      } as never,
      userMessage: "Create",
      formPayload: {},
      queryManager: qm,
      evaluationContext: contextFor(store, qm),
      store,
      getResult: () => parse,
    });
    await settle();
    expect(calls[0]!.args).toEqual({ name: "draft" });
    store.set("$name", "typed-meanwhile");
    calls[0]!.resolve({ id: "srv-42" });
    await settle();
    expect(store.get("$id")).toBe("srv-42");
    expect(calls[1]!.args).toEqual({ id: "srv-42", name: "draft" });
    calls[1]!.resolve({ ok: true });
    await done;
    qm.dispose();
  });
});

describe("host contract mapping", () => {
  test("provider context is snake_case and omits absent action_id", () => {
    const signal = new AbortController().signal;
    expect(
      toProviderContext({ statementId: "s", trigger: "load", signal }),
    ).toEqual({ statement_id: "s", trigger: "load", signal });
    expect(
      toProviderContext({ statementId: "s", trigger: "press", actionId: "a" }),
    ).toEqual({ statement_id: "s", trigger: "press", action_id: "a" });
  });
});
