/**
 * Minimal jsonrpsee WebSocket client for the acceptance drivers in this directory (Bun).
 *
 *   import { connect, recorder, readToken, parseArgs } from "./rpc.ts";
 *   const rpc = await connect("ws://127.0.0.1:7777", readToken(dataDir));
 *   const info = await rpc.call<Info>("info");
 *   const rec = await recorder(rpc);      // subscribe_events(null) into rec.events
 *   await rec.waitFor((e) => e.type === "TurnFinished", 30_000, "turn");
 *
 * Params are jsonrpsee positional arrays (`rpc.call("set_model", [session, model])`).
 * Auth is `Authorization: Bearer <data_dir>/token` on the upgrade, as `rpc::serve` requires.
 * No dependencies beyond Bun's WebSocket.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** One `subscribe_events` item: `{"type": ..., ...}`. */
export type WireEvent = { type: string } & Record<string, unknown>;

export class RpcError extends Error {
  constructor(
    message: string,
    public code: number,
    public data?: unknown,
  ) {
    super(message);
  }
}

export interface Rpc {
  call<T = unknown>(method: string, params?: unknown[] | Record<string, unknown>): Promise<T>;
  /** `subscribe_events(session, descendants)`; returns an unsubscribe function. */
  subscribe(
    session: string | null,
    onEvent: (ev: WireEvent) => void,
    descendants?: boolean,
  ): Promise<() => Promise<void>>;
  close(): void;
  /** Resolves once the socket is closed (by either side). */
  closed: Promise<void>;
}

export interface Args {
  flags: Record<string, string | true | undefined>;
  positional: string[];
}

export function readToken(dataDir: string): string {
  return readFileSync(join(dataDir, "token"), "utf8").trim();
}

/** `--k=v`, `--k v`, `--flag` → `flags: {k: "v", flag: true}`; the rest under `positional`. */
export function parseArgs(argv: string[]): Args {
  const out: Args = { flags: {}, positional: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!;
    if (!a.startsWith("--")) {
      out.positional.push(a);
      continue;
    }
    const eq = a.indexOf("=");
    if (eq > 0) {
      out.flags[a.slice(2, eq)] = a.slice(eq + 1);
    } else if (i + 1 < argv.length && !argv[i + 1]!.startsWith("--")) {
      out.flags[a.slice(2)] = argv[++i]!;
    } else {
      out.flags[a.slice(2)] = true;
    }
  }
  return out;
}

interface Reply {
  id?: number;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
  method?: string;
  params?: { subscription?: string | number; result?: WireEvent };
}

export async function connect(url: string, token: string): Promise<Rpc> {
  // Bun's WebSocket takes `{headers}` as a second argument; the DOM typing does not know it.
  const ws = new WebSocket(
    url,
    { headers: { Authorization: `Bearer ${token}` } } as unknown as string[],
  );
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  const subs = new Map<string | number, (ev: WireEvent) => void>();
  let nextId = 1;
  const done = Promise.withResolvers<void>();

  ws.onmessage = (m) => {
    let msg: Reply;
    try {
      msg = JSON.parse(String(m.data)) as Reply;
    } catch {
      return;
    }
    if (msg.id !== undefined && pending.has(msg.id)) {
      const p = pending.get(msg.id)!;
      pending.delete(msg.id);
      if (msg.error) {
        p.reject(new RpcError(`${msg.error.message} (code ${msg.error.code})`, msg.error.code, msg.error.data));
      } else p.resolve(msg.result);
      return;
    }
    if (msg.method && msg.params?.subscription !== undefined && msg.params.result) {
      subs.get(msg.params.subscription)?.(msg.params.result);
    }
  };
  ws.onclose = () => {
    for (const p of pending.values()) p.reject(new Error("socket closed"));
    pending.clear();
    done.resolve();
  };

  const opened = Promise.withResolvers<void>();
  ws.onopen = () => opened.resolve();
  ws.onerror = () => opened.reject(new Error(`ws connect ${url} failed`));
  await opened.promise;

  const call = <T,>(method: string, params: unknown[] | Record<string, unknown> = []) => {
    const { promise, resolve, reject } = Promise.withResolvers<T>();
    const id = nextId++;
    pending.set(id, { resolve: (v) => resolve(v as T), reject });
    ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    return promise;
  };

  return {
    call,
    closed: done.promise,
    async subscribe(session, onEvent, descendants = false) {
      const id = await call<string | number>("subscribe_events", [session, descendants]);
      subs.set(id, onEvent);
      return async () => {
        subs.delete(id);
        await call("unsubscribe_events", [id]).catch(() => {});
      };
    },
    close() {
      ws.close();
    },
  };
}

export interface Recorder {
  events: WireEvent[];
  /** First event at index >= `from` (already recorded or future) satisfying `pred`; rejects after `ms`. */
  waitFor(pred: (ev: WireEvent) => boolean, ms: number, label: string, from?: number): Promise<WireEvent>;
  stop(): Promise<void>;
}

/** Records every event of `session` (`null` = all) from `subscribe_events`. */
export async function recorder(rpc: Rpc, session: string | null = null): Promise<Recorder> {
  const events: WireEvent[] = [];
  const waiters: { pred: (ev: WireEvent) => boolean; resolve: (ev: WireEvent) => void }[] = [];
  const unsub = await rpc.subscribe(session, (ev) => {
    events.push(ev);
    for (let i = waiters.length - 1; i >= 0; i--) {
      if (waiters[i]!.pred(ev)) waiters.splice(i, 1)[0]!.resolve(ev);
    }
  });
  return {
    events,
    waitFor(pred, ms, label, from = 0) {
      const hit = events.slice(from).find(pred);
      if (hit) return Promise.resolve(hit);
      const { promise, resolve, reject } = Promise.withResolvers<WireEvent>();
      const w = {
        pred,
        resolve: (ev: WireEvent) => {
          clearTimeout(t);
          resolve(ev);
        },
      };
      const t = setTimeout(() => {
        const i = waiters.indexOf(w);
        if (i >= 0) waiters.splice(i, 1);
        const tail = events.slice(-5).map((e) => e.type);
        reject(new Error(`timeout ${ms}ms waiting for ${label}; last events: ${tail.join(", ")}`));
      }, ms);
      waiters.push(w);
      return promise;
    },
    stop: unsub,
  };
}
