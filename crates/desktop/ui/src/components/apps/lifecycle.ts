/**
 * Pure pieces of the app host (46 §6, §7, §8): the tool bridge from the renderer's provider contract to
 * `app_call_tool`, result/error classification, pressed calls parked across a transport loss, and the
 * complete-statement split of a streaming patch. No React, no DOM; `AppHost` wires these to a view.
 */
import { autoClose, mergeStatements } from "@openuidev/lang-core";
import type { AppRendererProps } from "@wth/apps/renderer";
import type { Api } from "@/rpc/api";
import { RpcError } from "@/rpc/client";
import type { AppToolResult, CallArgs, CallStatus } from "@/rpc/extra";

export type Provider = AppRendererProps["provider"];
export type ProviderContext = Parameters<Provider>[2];
export type Runtime = NonNullable<
  NonNullable<AppRendererProps["runtimeRef"]>["current"]
>;

/** `RpcError.code` of a transport failure (`rpc/client.ts` `TRANSPORT`). */
const TRANSPORT = -32300;
/** `RpcError.code` the harness answers with while the app plugin is off. */
export const APP_DISABLED = -32001;
/** `RpcError.code` of JSON-RPC method-not-found: a harness older than apps. */
const METHOD_NOT_FOUND = -32601;

/** Poll interval for a call the harness reports `running`. */
const POLL_MS = 500;

/** Why a provider call failed, beyond the wire statuses: host-side outcomes the status strip must name. */
export type FailStatus =
  | Exclude<CallStatus, "succeeded" | "running">
  | "disconnected"
  | "preview"
  | "aborted";

/** A provider rejection the host can classify; the runtime keeps it as the statement's `cause`. */
export class AppCallError extends Error {
  constructor(
    readonly status: FailStatus,
    message: string,
    readonly request_id?: string,
  ) {
    super(message);
    this.name = "AppCallError";
  }
}

/** The status behind a runtime `cause`, or `null` for anything that is not ours. */
export const causeStatus = (cause: unknown): FailStatus | null =>
  cause instanceof AppCallError ? cause.status : null;

/** The value a statement receives for `r`: structured data, else the content parsed as JSON, else the text. Throws
 * `AppCallError` for every non-success status; a tool error under `succeeded` is `failed`. */
export function resultValue(r: AppToolResult): unknown {
  if (r.status === "succeeded" && !r.is_error) {
    if (r.structured !== null) return r.structured;
    try {
      return JSON.parse(r.content);
    } catch {
      return r.content;
    }
  }
  const status: FailStatus =
    r.status === "succeeded" || r.status === "running" ? "failed" : r.status;
  throw new AppCallError(
    status,
    r.content || status.replaceAll("_", " "),
    r.request_id,
  );
}

/** An RPC failure as an `AppCallError`; `AppCallError`s pass through. */
function wrap(e: unknown): AppCallError {
  if (e instanceof AppCallError) return e;
  if (e instanceof RpcError) {
    if (e.code === TRANSPORT)
      return new AppCallError("disconnected", "connection lost");
    if (e.code === APP_DISABLED)
      return new AppCallError(
        "unavailable",
        "apps are disabled on this harness",
      );
    if (e.code === METHOD_NOT_FOUND)
      return new AppCallError(
        "unavailable",
        "this harness does not support apps",
      );
    return new AppCallError("failed", e.message);
  }
  return new AppCallError("failed", e instanceof Error ? e.message : String(e));
}

/** `p`, or an `aborted` rejection as soon as `signal` fires. Aborting never cancels the harness's work (46 §7). */
function abortable<T>(
  p: Promise<T>,
  signal: AbortSignal | undefined,
): Promise<T> {
  if (!signal) return p;
  const { promise, resolve, reject } = Promise.withResolvers<T>();
  const abort = () => reject(new AppCallError("aborted", "superseded"));
  if (signal.aborted) {
    abort();
    // The superseded call still settles on its own; nobody listens, and the harness's work is not cancelled.
    p.catch(() => undefined);
  } else {
    signal.addEventListener("abort", abort, { once: true });
    p.then(resolve, reject).finally(() =>
      signal.removeEventListener("abort", abort),
    );
  }
  return promise;
}

/** How often parked presses re-read their status while any remain (46 §6.4). */
const PARKED_POLL_MS = 2_000;

/**
 * Pressed calls whose reply was lost to a transport failure (46 §6.4). The press already happened; the harness
 * keeps its outcome under the request id, so the promise the renderer awaits is settled from `app_call_status`
 * instead of by repeating the mutation. While any call is parked and the transport is open, one timer re-reads
 * status; while disconnected nothing is scheduled (46 §7), and `resume` settles at once before polling again.
 * An `AppCallChanged` for a parked id settles it at once through `settle`. A call the harness keeps reporting
 * `running` stays parked as long as the view lives; `abandon` clears the timer. Reads are not parked: the runtime
 * refreshes them once after reconnect.
 */
export class PendingPresses {
  readonly #calls = new Map<
    string,
    { resolve(r: AppToolResult): void; reject(e: AppCallError): void }
  >();
  readonly #api: () => Api;
  readonly #pollMs: number;
  #connected = true;
  #stopped = false;
  #timer: NodeJS.Timeout | undefined;
  #settling: Promise<void> | null = null;

  constructor(api: () => Api, pollMs = PARKED_POLL_MS) {
    this.#api = api;
    this.#pollMs = pollMs;
  }

  get size(): number {
    return this.#calls.size;
  }

  has(request_id: string): boolean {
    return this.#calls.has(request_id);
  }

  park(request_id: string): Promise<AppToolResult> {
    const { promise, resolve, reject } = Promise.withResolvers<AppToolResult>();
    this.#calls.set(request_id, { resolve, reject });
    this.#schedule();
    return promise;
  }

  /** Transport lost: stop scheduling; parked calls wait for `resume`. */
  pause(): void {
    this.#connected = false;
    clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  /** Transport back: read every parked status now, then keep polling while any remains. */
  resume(): void {
    if (this.#connected) return;
    this.#connected = true;
    void this.settle();
  }

  #schedule(): void {
    if (
      this.#timer !== undefined ||
      this.#stopped ||
      !this.#connected ||
      this.#calls.size === 0
    )
      return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      void this.settle();
    }, this.#pollMs);
  }

  /** Read every parked request's status once; a reply still `running` (or unreachable) stays parked and, while
   * connected, is read again after `pollMs`. One settle runs at a time. */
  settle(): Promise<void> {
    if (this.#settling) return this.#settling;
    const api = this.#api();
    this.#settling = Promise.all(
      [...this.#calls].map(async ([id, p]) => {
        try {
          const r = await api.app_call_status(id);
          if (r.status === "running" || !this.#calls.has(id)) return;
          this.#calls.delete(id);
          p.resolve(r);
        } catch (e) {
          const err = wrap(e);
          if (err.status === "disconnected" || !this.#calls.has(id)) return;
          this.#calls.delete(id);
          p.reject(err);
        }
      }),
    ).then(() => {
      this.#settling = null;
      this.#schedule();
    });
    return this.#settling;
  }

  /** The view is going away: stop polling; nothing will read these, their outcome stays discoverable on the harness. */
  abandon(): void {
    this.#stopped = true;
    clearTimeout(this.#timer);
    this.#timer = undefined;
    for (const p of this.#calls.values())
      p.reject(
        new AppCallError("unknown", "view closed before the harness answered"),
      );
    this.#calls.clear();
  }
}

/** What one provider executes against: the view's instance and the committed record/generation it was built for.
 * The host builds a new provider per generation, so a provider the runtime pinned for a deferred action keeps naming
 * the generation the user activated (46 §3.3). */
export type Target = {
  record_call_id: string;
  instance_id: string;
  generation: number;
  committed: boolean;
};

/** A press the harness answered (directly, by polling, or after a reconnect): what the view records for it. */
export type SettledPress = {
  statement_id: string;
  request_id: string;
  result: AppToolResult;
  ms: number;
};

export type ProviderOptions = {
  pollMs?: number;
  /** Every press outcome, including one recovered from status after a lost reply, so a missed `AppCallChanged` never hides it. */
  onPress?: (settled: SettledPress) => void;
};

/**
 * The renderer's provider over `app_call_tool` (46 §6.3). One invocation = one request id; a transport retry never
 * mints another. The tool name the runtime passes is ignored: the harness resolves it from the pinned statement.
 * `running` replies are polled through `app_call_status`; a lost reply to a press is parked (see `PendingPresses`).
 */
export function createProvider(
  api: () => Api,
  t: Target,
  pending: PendingPresses,
  { pollMs = POLL_MS, onPress }: ProviderOptions = {},
): Provider {
  return async (_tool, args, ctx) => {
    if (!t.committed)
      throw new AppCallError(
        "preview",
        "a preview runs no tools until the app is committed",
      );
    const request_id = crypto.randomUUID();
    const call: CallArgs = {
      record_call_id: t.record_call_id,
      instance_id: t.instance_id,
      generation: t.generation,
      statement_id: ctx.statement_id,
      request_id,
      args: args as CallArgs["args"],
      trigger: ctx.trigger,
      action_id: ctx.action_id ?? null,
    };
    const fetch = async (
      p: () => Promise<AppToolResult>,
    ): Promise<AppToolResult> => {
      try {
        return await abortable(p(), ctx.signal);
      } catch (e) {
        const err = wrap(e);
        if (err.status === "disconnected" && ctx.trigger === "press")
          return pending.park(request_id);
        throw err;
      }
    };
    const started = Date.now();
    let r = await fetch(() => api().app_call_tool(call));
    while (r.status === "running") {
      const tick = Promise.withResolvers<void>();
      setTimeout(tick.resolve, pollMs);
      await abortable(tick.promise, ctx.signal);
      r = await fetch(() => api().app_call_status(request_id));
    }
    if (ctx.trigger === "press")
      onPress?.({
        statement_id: ctx.statement_id,
        request_id,
        result: r,
        ms: Date.now() - started,
      });
    return resultValue(r);
  };
}

/**
 * A streaming patch split at the last newline that closes every bracket and string (upstream `autoClose`): the
 * prefix holds only complete statements and may enter `mergeStatements`; the tail is the statement still arriving
 * and is shown as source (46 §8.1). Never passes a partial parent into merge's deletion/reachability collection.
 */
export function splitPatch(patch: string): { complete: string; tail: string } {
  let cut = patch.lastIndexOf("\n");
  while (cut >= 0) {
    const head = patch.slice(0, cut + 1);
    if (!autoClose(head).wasIncomplete)
      return { complete: head, tail: patch.slice(cut + 1) };
    cut = cut === 0 ? -1 : patch.lastIndexOf("\n", cut - 1);
  }
  return { complete: "", tail: patch };
}

/** The provisional program for an `edit` draft: the committed base with the complete patch statements merged in.
 * `error` names a merge failure, in which case the base is shown unchanged. */
export function previewPatch(
  base: string,
  patch: string,
): { source: string; tail: string; error?: string } {
  const { complete, tail } = splitPatch(patch);
  if (!complete.trim()) return { source: base, tail };
  try {
    return { source: mergeStatements(base, complete, "root"), tail };
  } catch (e) {
    return {
      source: base,
      tail,
      error: e instanceof Error ? e.message : String(e),
    };
  }
}

/** The app-origin input a `@ToAssistant` action sends through `send_input` (46 §6.3): provenance first, then the
 * chosen message, context and the form's field values as ordinary app data. Never a system instruction. */
export function assistantInput(
  app: { name: string; version: number; record_call_id: string },
  action_id: string | undefined,
  message: string,
  context: unknown,
  form?: { name?: string; values: Record<string, unknown> },
): string {
  const head = `[app ${app.name} v${app.version} · ${app.record_call_id}${action_id ? ` · action ${action_id}` : ""}]`;
  let out = `${head}\n${message.trim() || "(button pressed)"}`;
  if (context !== undefined && context !== null && context !== "")
    out += `\n\ncontext:\n${typeof context === "string" ? context : JSON.stringify(context, null, 2)}`;
  if (form && Object.keys(form.values).length > 0)
    out += `\n\nform${form.name ? ` ${form.name}` : ""}:\n${JSON.stringify(form.values, null, 2)}`;
  return out;
}
