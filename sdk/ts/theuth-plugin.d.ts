// theuth-plugin.d.ts 0.1.0: editor types for theuth-plugin.js (design 216). Node 18+ (with the template's package.json), Bun, Deno 2.

export const PROTOCOL: 1;

/** A JSON-RPC error. Throw it from a handler to answer with this error; request()/call() reject with it. */
export class RpcError extends Error {
  constructor(code: number, message: string, data?: unknown);
  code: number;
  data: unknown;
  /** `data.retry_after_ms` when the host sent one. */
  retry_after_ms: number | undefined;
  toJSON(): { code: number; message: string; data?: unknown };
}

/** A JSON object as the host sent it. */
export type Obj = Record<string, unknown>;

/** An object persisted to `<data dir>/state.json`. Every set/add writes it atomically (tmp + rename). */
export class State {
  /** The value at `key`, or `dflt` when the key is missing. */
  get<T = unknown>(key: string, dflt?: T): T;
  /** The integer at `key`; 0 when missing or not an integer. */
  int(key: string): number;
  /** Set `key` and save. */
  set(key: string, value: unknown): void;
  /** Add `n` (default 1) to the integer at `key`, save, return the new value. */
  add(key: string, n?: number): number;
  /** The whole state object (mutate it, then save()). */
  all(): Obj;
  save(): void;
}

/** `hello.params.grants` and the `granted` notification's grants. */
export interface Grants {
  subscribe?: string[];
  emits?: string[];
  placements: string[];
  tools?: string[];
  capabilities?: Obj;
}

/** A host request handed to action/query/command/tool/render handlers. */
export interface Request {
  placement?: string;
  /** Action, query, command or tool name. */
  name?: string;
  /** Request args; tools/call: `params.arguments`. */
  args?: unknown;
  /** tools/call: `params._meta.theuth.session`. */
  session?: string | null;
  client?: string;
  /** `null` for OpenUI actions from the desktop: form field values arrive in `args` (through `$state`). */
  form?: unknown;
  activation?: string;
  id: string | number;
  /** The whole JSON-RPC message. */
  raw: Obj;
}

/** A host `event` notification. */
export interface Event {
  topic: string;
  version: number;
  type: string;
  session?: string | null;
  at_ms: number;
  /** The JSON object the host sent, unchanged. */
  data: Obj;
  raw: Obj;
}

/** [label, action] or [label, action, style]; style defaults to "secondary". */
export type Button = [string, string] | [string, string, string];
export type Dot = "ok" | "busy" | "warn" | "error" | "off" | "none";
export type Maybe<T> = T | Promise<T>;

export interface StatusOptions { dot: Dot; badge?: number | string; label?: string; tooltip?: string; placement?: string }
export interface UiOptions { text: string; buttons?: Button[]; source?: string; format?: string; placement?: string }
export interface NotifyOptions { title: string; body?: string; level?: string; open?: Obj }
export interface LogOptions { level?: string; fields?: Obj }

/** The OpenUI source `ui(text, buttons)` sends. */
export function openui_card(text: string, buttons?: Button[]): string;

export class Plugin {
  state: State;
  /** `hello` params, and the parts of it the SDK keeps current. */
  hello: Obj;
  grants: Grants;
  data_dir: string | null;
  clients: Obj;
  /** Ids of host requests the host cancelled (`notifications/cancelled`). */
  cancelled: Set<string | number>;

  // registration: each returns the plugin
  /** Event handler. `name`: `agent.events#TurnFinished`, `agent.events`, `turns.limit_reached`, or `*`. */
  on(name: string, fn: (ev: Event) => Maybe<void>): this;
  /** `action` request. Return a JSON value (nothing -> {ok: true}); throw RpcError for an error. */
  action(name: string, fn: (req: Request) => Maybe<unknown>): this;
  /** OpenUI `Query(name, …)`. The return value is the query's value. */
  query(name: string, fn: (req: Request) => Maybe<unknown>): this;
  /** `/plugin:name` command. Return a string (-> {text}), an object (as-is) or nothing (-> {}). */
  command(name: string, fn: (req: Request) => Maybe<unknown>): this;
  /** `tools/call`. Return a string, a CallToolResult (`content`/`isError`), or any JSON value. */
  tool(name: string, fn: (req: Request) => Maybe<unknown>): this;
  /** Raw hatch: answer a host request method the helpers do not wrap. */
  on_request(method: string, fn: (params: unknown) => Maybe<unknown>): this;
  onRequest(method: string, fn: (params: unknown) => Maybe<unknown>): this;
  /** Raw hatch: handle a host notification method. */
  on_notify(method: string, fn: (params: unknown) => Maybe<void>): this;
  onNotify(method: string, fn: (params: unknown) => Maybe<void>): this;

  // hooks: several may be registered; they run in order
  /** After `hello`. Paint here. Also re-run on `render` unless on_render is set. */
  on_start(fn: () => Maybe<void>): this;
  onStart(fn: () => Maybe<void>): this;
  /** On `shutdown` or stdin EOF, before exit 0. */
  on_shutdown(fn: () => Maybe<void>): this;
  onShutdown(fn: () => Maybe<void>): this;
  /** Grants changed; `plugin.grants` is already replaced. */
  on_granted(fn: (grants: Grants) => Maybe<void>): this;
  onGranted(fn: (grants: Grants) => Maybe<void>): this;
  /** Events were dropped. Default: a warn log. */
  on_lagged(fn: (params: { dropped: number; since_ms: number }) => Maybe<void>): this;
  onLagged(fn: (params: { dropped: number; since_ms: number }) => Maybe<void>): this;
  /** `notifications/cancelled`; the id is already in `plugin.cancelled`. */
  on_cancel(fn: (params: { requestId: string | number; reason?: string }) => Maybe<void>): this;
  onCancel(fn: (params: { requestId: string | number; reason?: string }) => Maybe<void>): this;
  /** `render` request. Send a `ui`; the SDK replies {}. */
  on_render(fn: (req: Request) => Maybe<void>): this;
  onRender(fn: (req: Request) => Maybe<void>): this;

  // plugin -> host notifications
  /** Raw notification. */
  send(method: string, params?: Obj): void;
  /** Placement defaults to the single granted placement (otherwise throws). */
  status(dot: Dot, badge?: number | string, label?: string, tooltip?: string, placement?: string): void;
  status(options: StatusOptions): void;
  /** Without `source`, format openui (default) builds an OpenUI card from text + buttons. */
  ui(text: string, buttons?: Button[], source?: string, format?: string, placement?: string): void;
  ui(options: UiOptions): void;
  emit(event: string, data: Obj, session?: string): void;
  notify(title: string, body?: string, level?: string, open?: Obj): void;
  notify(options: NotifyOptions): void;
  invalidate(placement?: string): void;
  log(message: unknown, level?: string, fields?: Obj): void;
  log(message: unknown, options: LogOptions): void;

  // plugin -> host requests: resolve to `result`, reject with RpcError (-32006 once stdin closed)
  /** Raw request. */
  request<T = unknown>(method: string, params?: Obj): Promise<T>;
  /** Host-brokered tool call; resolves to the ToolOutput `{content, is_error, structured?}`. */
  call<T = Obj>(tool: string, args?: Obj): Promise<T>;
  input(text: string, session?: string, delivery?: string): Promise<{ session: string }>;
  catalog<T = unknown>(topic?: string): Promise<T>;
  ping(): Promise<{ now_ms: number }>;

  /** Serve stdin until `shutdown` or EOF, run on_shutdown hooks, exit 0. Never settles. `--theuth-manifest` prints the registered names and exits. */
  run(): Promise<never>;
  /** Test harness: run host messages without stdin; resolves to what the plugin sent. */
  drive(lines: Iterable<string | Obj>, responder?: (method: string, params: Obj) => Maybe<unknown>): Promise<Obj[]>;
}

/** The plugin. One per process. */
export const plugin: Plugin;
