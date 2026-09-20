/**
 * Host-facing wrapper over the patched upstream `Renderer` (design 46 §6.2, §7, §8).
 *
 * The upstream store/renderer is used as-is; this file only adapts the host contract:
 * snake_case provenance, per-generation query reset, pause, action scope callbacks and
 * deferring program updates while an action is running.
 */
import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { Renderer } from "@openuidev/react-lang";
import type {
  Library,
  OpenUIError,
  OpenUIRuntime,
  RendererProps,
} from "@openuidev/react-lang";
import type {
  ActionEndEvent,
  ActionStartEvent,
  QueryStatus as MetaQueryStatus,
  ScopedActionEvent,
  ToolCallMeta,
} from "@openuidev/react-lang/theuth-runtime";
import type {
  ActionResult,
  ActionScope,
  AppActionEvent,
  AppRuntime,
  AppToolProvider,
  ProviderContext,
  QueryStatus,
} from "./runtime-types.ts";

export interface AppRendererProps {
  /** Committed source, or the progressive preview while streaming. */
  response: string | null;
  library: Library;
  /** Preview phase: upstream keeps queries/mutations inert and controls disabled until commit. */
  isStreaming: boolean;
  /** Committed version. A change reloads query data and ignores late results of the old version; `$state` survives. */
  generation: number;
  provider: AppToolProvider;
  /** Hidden/background/disconnected view: no new fetches are started; in-flight calls finish. */
  paused?: boolean;
  /** Hold `response`/`generation`/`isStreaming` changes until the running action ends. Default true. */
  deferUpdatesDuringAction?: boolean;
  onAction?: (event: AppActionEvent) => void;
  onActionStart?: (scope: ActionScope) => void;
  onActionEnd?: (result: ActionResult) => void;
  runtimeRef?: { current: AppRuntime | null };
  onStateUpdate?: RendererProps["onStateUpdate"];
  initialState?: RendererProps["initialState"];
  onError?: (errors: OpenUIError[]) => void;
  queryLoader?: RendererProps["queryLoader"];
}

export function toProviderContext(meta: ToolCallMeta): ProviderContext {
  const context: ProviderContext = {
    statement_id: meta.statementId,
    trigger: meta.trigger,
  };
  if (meta.actionId !== undefined) context.action_id = meta.actionId;
  if (meta.signal) context.signal = meta.signal;
  return context;
}

function toQueryStatus(status: MetaQueryStatus | null): QueryStatus | null {
  if (!status) return null;
  const { hasData, ...rest } = status;
  return { ...rest, has_data: hasData };
}

function toAppRuntime(runtime: OpenUIRuntime): AppRuntime {
  return {
    pause: runtime.pause,
    resume: runtime.resume,
    is_paused: runtime.isPaused,
    query_status: (id) => toQueryStatus(runtime.getQueryStatus(id)),
    mutation_cause: runtime.getMutationCause,
    subscribe: runtime.subscribe,
    refresh: runtime.refresh,
    press: runtime.press,
  };
}

/**
 * What the upstream Renderer sees, deferred as one unit: while an action holds back an update, a
 * click on the still-visible frame reaches the provider and callbacks passed alongside that frame.
 */
interface Program {
  response: string | null;
  generation: number;
  isStreaming: boolean;
  provider: AppToolProvider;
  onAction?: (event: AppActionEvent) => void;
  onActionStart?: (scope: ActionScope) => void;
  onActionEnd?: (result: ActionResult) => void;
  onError?: (errors: OpenUIError[]) => void;
}

export function AppRenderer({
  response,
  library,
  isStreaming,
  generation,
  provider,
  paused = false,
  deferUpdatesDuringAction = true,
  onAction,
  onActionStart,
  onActionEnd,
  runtimeRef,
  onStateUpdate,
  initialState,
  onError,
  queryLoader,
}: AppRendererProps) {
  // Defer program changes while an action runs; the committed frame, its provider and callbacks stay until it ends.
  const incoming: Program = {
    response,
    generation,
    isStreaming,
    provider,
    onAction,
    onActionStart,
    onActionEnd,
    onError,
  };
  const latestRef = useRef(incoming);
  latestRef.current = incoming;
  const appliedRef = useRef(incoming);
  const activeActions = useRef(0);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  if (!deferUpdatesDuringAction || activeActions.current === 0)
    appliedRef.current = incoming;
  const applied = appliedRef.current;

  // Stable identity: the patched Renderer creates its query manager per `generation`, so results of
  // the previous version are discarded rather than shown. Calls go to the applied program's provider.
  const toolProvider = useMemo(
    () => (tool: string, args: Record<string, unknown>, meta: ToolCallMeta) =>
      appliedRef.current.provider(tool, args, toProviderContext(meta)),
    [],
  );

  const handleActionStart = useCallback((event: ActionStartEvent) => {
    activeActions.current += 1;
    appliedRef.current.onActionStart?.({
      action_id: event.actionId,
      steps: event.steps,
    });
  }, []);
  const handleActionEnd = useCallback((event: ActionEndEvent) => {
    activeActions.current -= 1;
    const result: ActionResult = { action_id: event.actionId, ok: event.ok };
    if (event.error !== undefined) result.error = event.error;
    appliedRef.current.onActionEnd?.(result);
    if (activeActions.current === 0 && latestRef.current !== appliedRef.current)
      rerender();
  }, []);
  const handleAction = useCallback((event: ScopedActionEvent) => {
    const { actionId, ...rest } = event;
    const mapped: AppActionEvent = rest;
    if (actionId !== undefined) mapped.action_id = actionId;
    appliedRef.current.onAction?.(mapped);
  }, []);

  const upstreamRuntimeRef = useRef<OpenUIRuntime | null>(null);
  useEffect(() => {
    if (!runtimeRef) return;
    const runtime = upstreamRuntimeRef.current;
    runtimeRef.current = runtime ? toAppRuntime(runtime) : null;
    return () => {
      runtimeRef.current = null;
    };
    // The upstream handle is (re)assigned in an effect of the same commit; re-run per generation/mount.
  }, [runtimeRef, applied.generation]);

  return (
    <Renderer
      response={applied.response}
      library={library}
      isStreaming={applied.isStreaming}
      generation={applied.generation}
      paused={paused}
      toolProvider={toolProvider}
      onAction={handleAction}
      onActionStart={handleActionStart}
      onActionEnd={handleActionEnd}
      runtimeRef={upstreamRuntimeRef}
      onStateUpdate={onStateUpdate}
      initialState={initialState}
      onError={applied.onError}
      queryLoader={queryLoader}
    />
  );
}
