// Shared pieces of the two real-engine smokes (scripts/durability-mesh/smoke.ts,
// scripts/restart-clarity/smoke.ts): JSON types, logging, polling, and the engine's
// JSON-RPC over HTTP POST plus a `subscribe_events` websocket. Bun only.
export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
export type Obj = { [k: string]: Json };
export type Ev = { at: number; ev: Obj };
export type RpcError = { code: number; message: string; data?: Json };

export const sleep = (ms: number) => {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
};
export const now = () => Date.now();
const T0 = now();
const ts = () => `${((now() - T0) / 1000).toFixed(1).padStart(6)}s`;
export function log(msg: string) {
  console.log(`[${ts()}] ${msg}`);
}

export function isObj(v: unknown): v is Obj {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

/// A `waitFor` whose default timeout / poll interval are the caller's (they differ per smoke).
export function makeWaitFor(defaultTimeoutMs: number, defaultEveryMs: number) {
  return async function waitFor<T>(
    what: string,
    f: () => T | Promise<T>,
    timeoutMs = defaultTimeoutMs,
    every = defaultEveryMs,
  ): Promise<NonNullable<T>> {
    const end = now() + timeoutMs;
    for (;;) {
      const v = await f();
      if (v) return v as NonNullable<T>;
      if (now() > end) throw new Error(`timeout waiting for ${what}`);
      await sleep(every);
    }
  };
}

/// One JSON-RPC call over HTTP POST with the engine token. Never throws: failures come back as `error`.
export async function rawRpc(port: number, token: string, method: string, params: Json[]): Promise<{ result?: Json; error?: RpcError }> {
  try {
    const res = await fetch(`http://127.0.0.1:${port}/`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      signal: AbortSignal.timeout(60_000),
    });
    const j: unknown = await res.json();
    if (!isObj(j)) return { error: { code: -1, message: `bad reply ${JSON.stringify(j)}` } };
    if (isObj(j.error)) return { error: j.error as unknown as RpcError };
    return { result: j.result as Json };
  } catch (e) {
    return { error: { code: -2, message: String(e) } };
  }
}

/// Unwrap a `rawRpc` reply: the result (`null` when absent) or an Error `"<prefix><method>: <code> <message>"`.
export function rpcResult(r: { result?: Json; error?: RpcError }, method: string, prefix = ""): Json {
  if (r.error) throw new Error(`${prefix}${method}: ${r.error.code} ${r.error.message}`);
  return r.result ?? null;
}

/// Open the engine websocket, send `subscribe_events(null, true)` (every session, descendants included) and
/// call `onEvent` with each pushed event object. Resolves once the socket is open; `wsLabel` prefixes errors.
export async function subscribeEvents(port: number, token: string, wsLabel: string, onEvent: (ev: Obj) => void): Promise<WebSocket> {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/`, { headers: { authorization: `Bearer ${token}` } });
  const { promise, resolve, reject } = Promise.withResolvers<void>();
  ws.onopen = () => {
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "subscribe_events", params: [null, true] }));
    resolve();
  };
  ws.onerror = (e) => reject(new Error(`${wsLabel}: ${String(e)}`));
  await promise;
  ws.onmessage = (m) => {
    try {
      const j: unknown = JSON.parse(String(m.data));
      if (isObj(j) && isObj(j.params) && isObj(j.params.result) && typeof j.params.result.type === "string") {
        onEvent(j.params.result);
      }
    } catch {
      /* snapshot or non-JSON frame */
    }
  };
  return ws;
}
