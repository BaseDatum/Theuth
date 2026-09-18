/**
 * Public contract between the desktop host and the OpenUI renderer wrapper (design 46 §6.2, §7).
 * snake_case is the wire shape the host forwards to `app_call_tool`; the patched upstream
 * runtime speaks camelCase internally and `renderer.tsx` maps between them once.
 */
import type { ActionEvent, OpenUIError } from "@openuidev/react-lang";

/** `load`: first fetch; `refresh`: timer or automatic refetch; `state_change`: arguments changed; `press`: real activation. */
export type ProviderTrigger = "load" | "refresh" | "state_change" | "press";

export interface ProviderContext {
  statement_id: string;
  trigger: ProviderTrigger;
  /** Present only for calls made by one user activation (mutation, pressed `@Run(query)`, host retry). */
  action_id?: string;
  /** Aborted when the query's arguments change or the view is disposed; mutations carry none. */
  signal?: AbortSignal;
}

/** Host tool bridge. Resolve with the tool's structured result; reject to mark the statement failed. */
export type AppToolProvider = (
  tool: string,
  args: unknown,
  context: ProviderContext,
) => Promise<unknown>;

export interface ActionScope {
  action_id: string;
  /** Number of plan steps (0 for host-triggered `press`). */
  steps: number;
}

export interface ActionResult {
  action_id: string;
  /** False when a mutation failed (remaining steps skipped) or evaluation threw. */
  ok: boolean;
  error?: unknown;
}

/** Action emitted by a control; `action_id` is undefined only for events not raised by an activation. */
export interface AppActionEvent extends ActionEvent {
  action_id?: string;
}

export interface QueryStatus {
  /** `refetching`: loading with earlier data still shown. */
  state: "idle" | "loading" | "refetching" | "error";
  has_data: boolean;
  /** Visible data is not the settled result for the current arguments; render it marked stale. */
  stale: boolean;
  trigger?: ProviderTrigger;
  error?: OpenUIError;
  /** Raw provider rejection, so the host can recognise its own errors (needs approval, busy, unknown outcome). */
  cause?: unknown;
}

/** Handle published through `AppRendererProps.runtimeRef` while a renderer is mounted. */
export interface AppRuntime {
  pause(): void;
  resume(): void;
  is_paused(): boolean;
  query_status(statement_id: string): QueryStatus | null;
  /** Raw provider rejection of the last mutation attempt, if any. */
  mutation_cause(statement_id: string): unknown;
  /** Fires on any query/mutation status change. */
  subscribe(listener: () => void): () => void;
  /** Automatic refetch (`refresh`), e.g. once after reconnect. Omit ids for all queries. */
  refresh(statement_ids?: string[]): void;
  /** User-activated refetch (`press`) for Load/Refresh/Retry affordances; returns the new action id. */
  press(statement_ids: string[]): string;
}
