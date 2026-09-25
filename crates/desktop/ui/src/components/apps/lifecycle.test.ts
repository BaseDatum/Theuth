import { expect, test, vi } from "vitest";
import type { Api } from "@/rpc/api";
import { RpcError } from "@/rpc/client";
import type { AppToolResult } from "@/rpc/extra";
import {
  AppCallError,
  PendingPresses,
  type Target,
  assistantInput,
  parseAppInput,
  createProvider,
  previewPatch,
  resultValue,
  splitPatch,
} from "./lifecycle";

const ok = (over: Partial<AppToolResult> = {}): AppToolResult => ({
  request_id: "r",
  content: '{"status":"Healthy"}',
  structured: null,
  is_error: false,
  status: "succeeded",
  ...over,
});

const committed: Target = {
  record_call_id: "rec1",
  instance_id: "inst",
  generation: 2,
  committed: true,
};
const ctx = (
  trigger: "load" | "refresh" | "state_change" | "press",
  extra: { action_id?: string; signal?: AbortSignal } = {},
) => ({
  statement_id: "health",
  trigger,
  ...extra,
});

function mockApi() {
  const api = { app_call_tool: vi.fn(), app_call_status: vi.fn() };
  const as = () => api as unknown as Api;
  // A huge poll interval: tests drive `settle` explicitly unless they say otherwise.
  return { api, as, pending: () => new PendingPresses(as, 60_000) };
}

test("splitPatch cuts at the last newline that closes every bracket and string; the arriving statement is the tail", () => {
  expect(splitPatch('a = TextContent("x")\nb = Stack([a,')).toEqual({
    complete: 'a = TextContent("x")\n',
    tail: "b = Stack([a,",
  });
  expect(splitPatch("a = 1\nb = Stack([\n  a,")).toEqual({
    complete: "a = 1\n",
    tail: "b = Stack([\n  a,",
  });
  expect(splitPatch('a = TextContent("multi\nline')).toEqual({
    complete: "",
    tail: 'a = TextContent("multi\nline',
  });
  expect(splitPatch("root = Stack([a])")).toEqual({
    complete: "",
    tail: "root = Stack([a])",
  });
  expect(splitPatch("root = Stack([a])\n")).toEqual({
    complete: "root = Stack([a])\n",
    tail: "",
  });
});

test("previewPatch merges only complete statements: a partial root replacement never collects the old children", () => {
  const base =
    'root = Stack([a, b])\na = TextContent("A")\nb = TextContent("B")';
  const p = previewPatch(base, 'a = TextContent("A2")\nroot = Stack([a');
  expect(p.source).toContain('TextContent("A2")');
  expect(p.source).toContain('b = TextContent("B")');
  expect(p.source).toContain("root = Stack([a, b])");
  expect(p.tail).toBe("root = Stack([a");
  expect(p.error).toBeUndefined();
  // Once the root replacement completes, `b` is unreachable and goes; the tail is empty.
  const done = previewPatch(base, 'a = TextContent("A2")\nroot = Stack([a])\n');
  expect(done.source).not.toContain('TextContent("B")');
  expect(done.tail).toBe("");
  // Nothing complete yet: the base is shown unchanged.
  expect(previewPatch(base, "a = TextCon")).toEqual({
    source: base,
    tail: "a = TextCon",
  });
});

test("resultValue prefers structured data, parses JSON content, and throws a classified error otherwise", () => {
  expect(resultValue(ok({ structured: { synced: true } }))).toEqual({
    synced: true,
  });
  expect(resultValue(ok())).toEqual({ status: "Healthy" });
  expect(resultValue(ok({ content: "plain" }))).toBe("plain");
  expect(() => resultValue(ok({ is_error: true, content: "boom" }))).toThrow(
    expect.objectContaining({ status: "failed", message: "boom" }),
  );
  expect(() =>
    resultValue(ok({ status: "needs_approval", content: "" })),
  ).toThrow(expect.objectContaining({ status: "needs_approval" }));
  expect(() => resultValue(ok({ status: "busy" }))).toThrow(AppCallError);
});

test("a preview target runs no tool: the provider rejects before any RPC", async () => {
  const { api, as, pending } = mockApi();
  const provider = createProvider(
    as,
    { ...committed, generation: 0, committed: false },
    pending(),
  );
  await expect(
    provider("argo_app_health", { name: "x" }, ctx("load")),
  ).rejects.toMatchObject({ status: "preview" });
  expect(api.app_call_tool).not.toHaveBeenCalled();
});

test("one call = one request id with the agreed CallArgs; the runtime tool name is not sent", async () => {
  const { api, as, pending } = mockApi();
  api.app_call_tool.mockResolvedValue(ok({ structured: { synced: false } }));
  const provider = createProvider(as, committed, pending());
  const v = await provider(
    "argo_app_health",
    { name: "portal-web" },
    ctx("press", { action_id: "act1" }),
  );
  expect(v).toEqual({ synced: false });
  const call = api.app_call_tool.mock.calls[0]?.[0];
  expect(call).toMatchObject({
    record_call_id: "rec1",
    instance_id: "inst",
    generation: 2,
    statement_id: "health",
    args: { name: "portal-web" },
    trigger: "press",
    action_id: "act1",
  });
  expect(call.request_id).toMatch(/^[0-9a-f-]{36}$/);
  expect(Object.keys(call).sort()).toEqual([
    "action_id",
    "args",
    "generation",
    "instance_id",
    "record_call_id",
    "request_id",
    "statement_id",
    "trigger",
  ]);
  // An automatic load carries no action.
  api.app_call_tool.mockResolvedValue(ok());
  await provider("argo_app_health", {}, ctx("load"));
  expect(api.app_call_tool.mock.calls[1]?.[0]).toMatchObject({
    action_id: null,
    trigger: "load",
  });
  expect(api.app_call_tool.mock.calls[1]?.[0].request_id).not.toBe(
    call.request_id,
  );
});

test("a running reply is polled by status under the same request id until it settles", async () => {
  const { api, as, pending } = mockApi();
  api.app_call_tool.mockResolvedValue(ok({ status: "running" }));
  api.app_call_status
    .mockResolvedValueOnce(ok({ status: "running" }))
    .mockResolvedValueOnce(ok({ structured: { message: "synced" } }));
  const provider = createProvider(as, committed, pending(), { pollMs: 1 });
  await expect(
    provider("argo_sync", {}, ctx("press", { action_id: "a" })),
  ).resolves.toEqual({ message: "synced" });
  const id = api.app_call_tool.mock.calls[0]?.[0].request_id;
  expect(api.app_call_status.mock.calls.map((c) => c[0])).toEqual([id, id]);
  expect(api.app_call_tool).toHaveBeenCalledTimes(1);
});

test("a press whose reply is lost is parked, not repeated; reconnect settles it from status", async () => {
  const { api, as, pending: make } = mockApi();
  api.app_call_tool.mockRejectedValue(
    new RpcError(-32300, "connection closed"),
  );
  const pending = make();
  const provider = createProvider(as, committed, pending);
  const press = provider("argo_sync", {}, ctx("press", { action_id: "a" }));
  await Promise.resolve();
  await Promise.resolve();
  expect(pending.size).toBe(1);
  const id = api.app_call_tool.mock.calls[0]?.[0].request_id;
  // Still running on the first settle: stays parked.
  api.app_call_status.mockResolvedValueOnce(
    ok({ status: "running", request_id: id }),
  );
  await pending.settle();
  expect(pending.size).toBe(1);
  api.app_call_status.mockResolvedValueOnce(
    ok({ structured: { message: "done" }, request_id: id }),
  );
  await pending.settle();
  expect(pending.size).toBe(0);
  await expect(press).resolves.toEqual({ message: "done" });
  expect(api.app_call_status.mock.calls.every((c) => c[0] === id)).toBe(true);
  expect(api.app_call_tool).toHaveBeenCalledTimes(1);
  pending.abandon();
});

test("a lost read is not parked: it fails as disconnected for the runtime to refresh after reconnect", async () => {
  const { api, as, pending: make } = mockApi();
  api.app_call_tool.mockRejectedValue(
    new RpcError(-32300, "connection closed"),
  );
  const pending = make();
  const provider = createProvider(as, committed, pending);
  await expect(
    provider("argo_app_health", {}, ctx("refresh")),
  ).rejects.toMatchObject({ status: "disconnected" });
  expect(pending.size).toBe(0);
});

test("abandoning parked presses reports unknown; a disabled engine reads as unavailable", async () => {
  const { api, as, pending: make } = mockApi();
  const pending = make();
  const parked = pending.park("r1");
  pending.abandon();
  await expect(parked).rejects.toMatchObject({ status: "unknown" });
  expect(pending.size).toBe(0);

  api.app_call_tool.mockRejectedValue(new RpcError(-32001, "app is disabled"));
  await expect(
    createProvider(as, committed, pending)("t", {}, ctx("load")),
  ).rejects.toMatchObject({ status: "unavailable" });
});

test("a parked press polls only while connected, settles at once on resume, and leaves no timer after abandon", async () => {
  vi.useFakeTimers();
  try {
    const { api, as } = mockApi();
    const pending = new PendingPresses(as, 1000);
    api.app_call_status
      .mockResolvedValueOnce(ok({ status: "running" }))
      .mockResolvedValueOnce(ok({ structured: { message: "landed" } }));
    const p = pending.park("r7");
    expect(api.app_call_status).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1000);
    expect(api.app_call_status.mock.calls.map((c) => c[0])).toEqual(["r7"]);
    await vi.advanceTimersByTimeAsync(1000);
    await expect(p).resolves.toMatchObject({
      structured: { message: "landed" },
    });
    expect(api.app_call_status).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);

    // Disconnected: nothing is scheduled, however long it lasts.
    pending.pause();
    const p2 = pending.park("r8");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(api.app_call_status).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);

    // Reconnected: read at once, then the bounded timer resumes while it is still running.
    api.app_call_status.mockResolvedValue(ok({ status: "running" }));
    pending.resume();
    await vi.advanceTimersByTimeAsync(0);
    expect(api.app_call_status).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(1000);
    expect(api.app_call_status).toHaveBeenCalledTimes(4);
    expect(pending.size).toBe(1);

    // Closing the view: unknown outcome, timer gone, no further reads.
    pending.abandon();
    await expect(p2).rejects.toMatchObject({ status: "unknown" });
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(api.app_call_status).toHaveBeenCalledTimes(4);
  } finally {
    vi.useRealTimers();
  }
});

test("an aborted query rejects locally and never issues a second request", async () => {
  const { api, as, pending } = mockApi();
  const reply = Promise.withResolvers<AppToolResult>();
  api.app_call_tool.mockReturnValue(reply.promise);
  const ac = new AbortController();
  const provider = createProvider(as, committed, pending());
  const p = provider(
    "argo_app_health",
    {},
    ctx("state_change", { signal: ac.signal }),
  );
  ac.abort();
  await expect(p).rejects.toMatchObject({ status: "aborted" });
  reply.resolve(ok());
  await Promise.resolve();
  expect(api.app_call_tool).toHaveBeenCalledTimes(1);
  expect(api.app_call_status).not.toHaveBeenCalled();
});

test("assistantInput leads with provenance and carries message and context as data", () => {
  const app = { name: "argo", version: 3, record_call_id: "rec1" };
  expect(
    assistantInput(app, "act9", "Explain this application's health", "Healthy"),
  ).toBe(
    "[app argo v3 · rec1 · action act9]\nExplain this application's health\n\ncontext:\nHealthy",
  );
  expect(assistantInput(app, undefined, "  ", undefined)).toBe(
    "[app argo v3 · rec1]\n(button pressed)",
  );
  expect(assistantInput(app, "a", "Look", { synced: false })).toContain(
    'context:\n{\n  "synced": false\n}',
  );
  expect(
    assistantInput(app, "a", "Send", undefined, {
      name: "ticket",
      values: { title: "Down", severity: 2 },
    }),
  ).toBe(
    '[app argo v3 · rec1 · action a]\nSend\n\nform ticket:\n{\n  "title": "Down",\n  "severity": 2\n}',
  );
  expect(assistantInput(app, "a", "Send", undefined, { values: {} })).toBe(
    "[app argo v3 · rec1 · action a]\nSend",
  );
});

test("parseAppInput reads assistantInput back, section by section", () => {
  const app = { name: "playground.openui", version: 1, record_call_id: "call-1_0" };
  expect(parseAppInput(assistantInput(app, "6cf3", "User is in the OpenUI playground.\nclicks: 4", undefined, { values: { $count: 4, $name: "Sky" } }))).toEqual({
    name: "playground.openui",
    version: "1",
    record: "call-1_0",
    action: "6cf3",
    message: "User is in the OpenUI playground.\nclicks: 4",
    form: { values: '{\n  "$count": 4,\n  "$name": "Sky"\n}' },
  });
  expect(parseAppInput(assistantInput(app, undefined, "", "Healthy", { name: "ticket", values: { a: 1 } }))).toEqual({
    name: "playground.openui",
    version: "1",
    record: "call-1_0",
    message: "(button pressed)",
    context: "Healthy",
    form: { name: "ticket", values: '{\n  "a": 1\n}' },
  });
  expect(parseAppInput("[app x v1 · r]\nhi")).toEqual({ name: "x", version: "1", record: "r", message: "hi" });
  expect(parseAppInput("[app x v1 · call-1\nfc_2 · action a]\nhi")).toEqual({ name: "x", version: "1", record: "call-1\nfc_2", action: "a", message: "hi" });
  expect(parseAppInput("[app x v1 · r] hi")).toBeNull();
  expect(parseAppInput("[application] hi")).toBeNull();
});
