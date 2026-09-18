/**
 * The live app view (46 §6–§8): the patched OpenUI renderer over the app RPC bridge. One mounted host is one
 * `instance_id`; each committed record it shows is a `generation`. A draft previews with the runtime inert and
 * no tool RPC; `ArtifactShown` turns the same mounted renderer into the committed, interactive generation.
 */
import "@openuidev/react-ui/layered/styles/index.css";
import { ThemeProvider } from "@openuidev/react-ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { library, libraryHash } from "@wth/apps";
import { AppRenderer, type AppRendererProps } from "@wth/apps/renderer";
import { Loader2 } from "lucide-react";
import {
  type ReactNode,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { toast } from "sonner";
import { useText } from "@/components/chat/ArtifactCard";
import { Button } from "@/components/ui/button";
import { useApi } from "@/lib/rpc";
import { cn } from "@/lib/utils";
import { RpcError } from "@/rpc/client";
import type { AppBinding, AppCallChanged, AppDoc } from "@/rpc/extra";
import { useConnection } from "@/store/connection";
import { on, useSessions } from "@/store/sessions";
import type { AppViewProps } from "./AppView";
import {
  APP_DISABLED,
  PendingPresses,
  type Runtime,
  assistantInput,
  causeStatus,
  createProvider,
  previewPatch,
} from "./lifecycle";

type ActionEvent = Parameters<NonNullable<AppRendererProps["onAction"]>>[0];
type RenderError = Parameters<
  NonNullable<AppRendererProps["onError"]>
>[0][number];

const fail = (e: unknown) =>
  toast.error(e instanceof Error ? e.message : String(e));

const short = (hash: string) => hash.slice(0, 12);

const NO_CALLS: ReadonlyMap<string, AppCallChanged> = new Map();

const RISK: Record<AppBinding["risk"], string> = {
  read_only: "bg-layer-2 text-fg-secondary",
  mutating: "bg-warning-bg text-warning",
  destructive: "bg-danger-bg text-danger",
};

/** `true` while the document is shown; hidden views stop scheduling (46 §7). */
function useVisible(): boolean {
  return useSyncExternalStore(
    (cb) => {
      document.addEventListener("visibilitychange", cb);
      return () => document.removeEventListener("visibilitychange", cb);
    },
    () => document.visibilityState === "visible",
  );
}

/** Follows the `dark` class `lib/theme.ts` toggles on `<html>`. */
function useDark(): boolean {
  return useSyncExternalStore(
    (cb) => {
      const o = new MutationObserver(cb);
      o.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class"],
      });
      return () => o.disconnect();
    },
    () => document.documentElement.classList.contains("dark"),
  );
}

/** `get_app` for one record. Source/bindings are immutable once committed, but `recent_calls` is not: every mount
 * (reopen) refetches, and a reconnect invalidates (46 §6.4); cached data stays on screen meanwhile. */
function useAppDoc(callId: string | undefined, enabled: boolean) {
  const api = useApi();
  return useQuery({
    queryKey: ["app", callId],
    queryFn: () => api.get_app(callId as string),
    enabled: enabled && callId !== undefined,
    staleTime: 0,
    refetchOnMount: "always",
    refetchOnWindowFocus: false,
    retry: false,
  });
}

/** Why a record cannot run here, from its `get_app` failure. */
function reasonOf(e: unknown): string {
  if (e instanceof RpcError && e.code === APP_DISABLED)
    return "Apps are disabled on this harness; the source stays readable.";
  if (e instanceof RpcError && e.code === -32601)
    return "This harness predates apps: update it to run this app. The source stays readable.";
  return `Could not load the app: ${e instanceof Error ? e.message : String(e)}`;
}

type Program =
  /** A committed, compatible record: interactive. */
  | { kind: "run"; doc: AppDoc; response: string }
  /** A draft: progressive render, runtime inert, no tool RPC. `doc` is the base of a patch. */
  | {
      kind: "preview";
      response: string;
      tail: string;
      doc?: AppDoc;
      mergeError?: string;
    }
  /** Readable source only: disabled, incompatible, unsupported, failed or cancelled (46 §8.3). `null` source = still reading. */
  | { kind: "source"; source: string | null; reason: string }
  | { kind: "loading" };

export default function AppHost({ session, artifact, draft }: AppViewProps) {
  const api = useApi();
  const qc = useQueryClient();
  const apiRef = useRef(api);
  apiRef.current = api;
  const enabled = useConnection((s) => s.info?.app_enabled === true);
  const conn = useConnection((s) => s.state);
  const visible = useVisible();
  const dark = useDark();

  const docQ = useAppDoc(artifact?.call_id, enabled);
  const baseQ = useAppDoc(draft?.edit ? draft.base : undefined, enabled);
  // A patch still arriving against the shown lineage previews over the committed view (46 §8.1): the runtime goes inert
  // (the wrapper defers the swap while an action runs) and the committed frame returns if the edit fails.
  const attached =
    artifact !== undefined &&
    draft?.edit === true &&
    (draft.status === "streaming" || draft.status === "validating");
  // Without a usable `get_app` the bytes themselves are the fallback.
  const needFile = artifact !== undefined && (!enabled || docQ.isError);
  const fileText = useText(needFile ? artifact.path : null);

  let program: Program;
  let banner: { tone: "error" | "info"; text: string } | undefined;
  let patch: string | undefined;
  if (artifact) {
    if (!enabled)
      program = {
        kind: "source",
        source: fileText,
        reason: "Apps are disabled on this harness; the source stays readable.",
      };
    else if (docQ.isPending) program = { kind: "loading" };
    else if (docQ.isError)
      program = {
        kind: "source",
        source: fileText,
        reason: reasonOf(docQ.error),
      };
    else if (docQ.data.library_hash !== libraryHash)
      program = {
        kind: "source",
        source: docQ.data.source,
        reason: `This app was published for app library ${short(docQ.data.library_hash)}; this desktop runs ${short(libraryHash)}. Update wth and the desktop to matching versions, or ask for the app to be republished.`,
      };
    else if (
      attached &&
      baseQ.data &&
      baseQ.data.library_hash === libraryHash
    ) {
      const p = previewPatch(baseQ.data.source, draft.source);
      program = {
        kind: "preview",
        response: p.source,
        tail: p.tail,
        doc: baseQ.data,
        ...(p.error ? { mergeError: p.error } : {}),
      };
    } else
      program = { kind: "run", doc: docQ.data, response: docQ.data.source };
  } else if (draft) {
    const settled = draft.status === "failed" || draft.status === "cancelled";
    if (settled)
      banner =
        draft.status === "failed"
          ? { tone: "error", text: draft.error ?? "publishing failed" }
          : { tone: "info", text: "Cancelled before publishing." };
    if (draft.edit) {
      patch = draft.source;
      if (!draft.base)
        program = {
          kind: "source",
          source: draft.source,
          reason: "This patch names no base app.",
        };
      else if (!enabled)
        program = {
          kind: "source",
          source: draft.source,
          reason:
            "Apps are disabled on this harness; the patch stays readable.",
        };
      else if (baseQ.isPending) program = { kind: "loading" };
      else if (baseQ.isError)
        program = {
          kind: "source",
          source: draft.source,
          reason: reasonOf(baseQ.error),
        };
      else if (settled) {
        // The edit did not land: the committed base stays the app the user has (46 §8.1).
        const base = baseQ.data;
        program =
          base.library_hash === libraryHash
            ? { kind: "run", doc: base, response: base.source }
            : {
                kind: "source",
                source: base.source,
                reason: `The base app was published for app library ${short(base.library_hash)}; this desktop runs ${short(libraryHash)}.`,
              };
      } else {
        const p = previewPatch(baseQ.data.source, draft.source);
        program = {
          kind: "preview",
          response: p.source,
          tail: p.tail,
          doc: baseQ.data,
          ...(p.error ? { mergeError: p.error } : {}),
        };
      }
    } else if (settled)
      program = {
        kind: "source",
        source: draft.source,
        reason:
          draft.status === "failed"
            ? "Publishing failed; the drafted source stays readable."
            : "Cancelled before publishing.",
      };
    else program = { kind: "preview", response: draft.source, tail: "" };
  } else program = { kind: "loading" };

  // The renderer mounts for a run or a preview and stays mounted across the promotion between them. While the next
  // record's document is still loading (a draft just committed, a newer version arrived), the last live program
  // stays rendered: an old generation keeps its controls and a running action, a preview keeps its frame; source,
  // generation and provider then switch together in one render once the document is ready (46 §3.3, §8.1).
  const lastView = useRef<Extract<Program, { kind: "run" | "preview" }> | null>(
    null,
  );
  const fetching = program.kind === "loading" && lastView.current !== null;
  const view =
    program.kind === "run" || program.kind === "preview"
      ? program
      : fetching
        ? lastView.current
        : null;
  lastView.current = view;
  const live = view !== null;

  // One instance per mounted view (the harness binds it to a record lineage; an adopted version is generation+1 on the
  // same instance, 46 §3.3, §6.3). A reconnect is a new server peer: the old instance died with the old one, so the
  // view continues under a fresh id; parked presses are found by request id regardless.
  const [instance, setInstance] = useState(() => crypto.randomUUID());
  const pending = useRef<PendingPresses | null>(null);
  pending.current ??= new PendingPresses(() => apiRef.current);
  const presses = pending.current;
  const recordId = view?.kind === "run" ? view.doc.artifact.call_id : undefined;
  const genRef = useRef<{ id: string | undefined; n: number }>({
    id: undefined,
    n: 0,
  });
  if (recordId !== genRef.current.id)
    genRef.current = {
      id: recordId,
      n: recordId === undefined ? genRef.current.n : genRef.current.n + 1,
    };
  const generation = genRef.current.n;
  const committed = view?.kind === "run";
  // A provider per target: one the runtime pinned for a deferred action keeps naming the generation the user pressed in.
  // Every press outcome the view itself learns is recorded like its `AppCallChanged` would be, so a lost event
  // never hides what a button did (46 §6.4).
  const bindings = view?.doc?.bindings;
  const provider = useMemo(
    () =>
      createProvider(
        () => apiRef.current,
        {
          record_call_id: recordId ?? "",
          instance_id: instance,
          generation,
          committed,
        },
        presses,
        {
          onPress: ({ statement_id, request_id, result, ms }) => {
            const b = bindings?.find((x) => x.statement_id === statement_id);
            if (!b || !recordId) return;
            useSessions
              .getState()
              .recordAppCall(session, {
                type: "AppCallChanged",
                session,
                record_call_id: recordId,
                request_id,
                statement_id,
                tool: b.tool,
                risk: b.risk,
                trigger: "press",
                status: result.status,
                ms,
              });
          },
        },
      ),
    [recordId, instance, generation, committed, presses, bindings, session],
  );

  // Closing cleans the instance; the harness keeps any accepted mutation's outcome under its request id (46 §6.4).
  // The close is deferred one tick and cancelled by a re-run setup, so StrictMode's replayed mount does not close
  // the instance the live view keeps using.
  const instanceRef = useRef(instance);
  instanceRef.current = instance;
  const closing = useRef<number | undefined>(undefined);
  useEffect(() => {
    clearTimeout(closing.current);
    closing.current = undefined;
    return () => {
      closing.current = window.setTimeout(() => {
        presses.abandon();
        apiRef.current.app_close(instanceRef.current).catch(() => undefined);
      }, 0);
    };
  }, [presses]);

  // The wrapper publishes the runtime handle when its (possibly deferred) generation actually mounts; status
  // subscription follows that handle, not the generation this render asked for.
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const [runtime, setRuntime] = useState<Runtime | null>(null);
  const runtimeRef = useMemo(() => {
    let cur: Runtime | null = null;
    let off: (() => void) | undefined;
    return {
      get current() {
        return cur;
      },
      set current(r: Runtime | null) {
        off?.();
        off = r?.subscribe(bump);
        cur = r;
        setRuntime(r);
        bump();
      },
    };
  }, []);

  // Transport loss: stop scheduling (renderer paused, parked presses paused), then on reconnect continue under a new
  // instance (new server peer), read parked presses' status at once and refresh reads once; never replay a mutation (46 §7).
  // `ready` keeps the renderer paused across the first render after `conn` opens, until this effect has rotated the
  // instance: otherwise the runtime would resume and bind the old instance id to the new peer.
  const [ready, setReady] = useState(true);
  const offline = useRef(false);
  useEffect(() => {
    if (conn !== "open") {
      offline.current = true;
      setReady(false);
      presses.pause();
      return;
    }
    if (!offline.current) return;
    offline.current = false;
    setInstance(crypto.randomUUID());
    setReady(true);
    presses.resume();
    void qc.invalidateQueries({ queryKey: ["app"] });
    void presses.settle().finally(() => runtimeRef.current?.refresh());
  }, [conn, presses, qc]);

  // Durable status of pressed invocations against this record (46 §6.4) lives in the session store, so a reopened
  // view still shows them; a parked press whose outcome is announced settles at once instead of waiting for its poll.
  const calls = useSessions((s) =>
    recordId ? s.sessions[session]?.appCalls.get(recordId) : undefined,
  );
  useEffect(
    () =>
      on("AppCallChanged", (e) => {
        if (e.session === session && presses.has(e.request_id))
          void presses.settle();
      }),
    [session, presses],
  );

  const [active, setActive] = useState<{
    action_id: string;
    steps: number;
    record: string | undefined;
  } | null>(null);
  // The wrapper holds the program it applied while any action runs; `applied` mirrors that: it follows this render's
  // record only while no action is active, so every activation and its events name the version actually on screen.
  type Provenance = { name: string; version: number; record_call_id: string };
  const doc = view?.doc;
  const incoming: Provenance = {
    name: doc?.artifact.name ?? draft?.name ?? "app",
    version: doc?.artifact.version ?? 0,
    record_call_id: recordId ?? "",
  };
  const running = useRef(new Set<string>());
  const applied = useRef(incoming);
  if (running.current.size === 0) applied.current = incoming;
  const activations = useRef(new Map<string, Provenance>());
  const [errors, setErrors] = useState<RenderError[]>([]);
  const response = view?.response ?? "";
  useEffect(() => setErrors([]), [response]);

  // Unknown root/parents yield no nodes; the host never shows a blank sheet (46 §8.3).
  const frame = useRef<HTMLDivElement>(null);
  const [empty, setEmpty] = useState(false);
  useEffect(() => {
    const el = frame.current;
    if (!el) return;
    const check = () => setEmpty(el.childElementCount === 0);
    check();
    const o = new MutationObserver(check);
    o.observe(el, { childList: true });
    return () => o.disconnect();
  }, [live]);

  const onAction = (ev: ActionEvent) => {
    if (ev.type === "open_url") {
      const url = ev.params.url;
      // Ordinary links open at once (46 §8.2): `window.open` is the system opener inside Tauri (`main.tsx`) and a new
      // tab in a browser; other schemes are not executable here.
      if (typeof url === "string" && /^https?:\/\//i.test(url))
        window.open(url, "_blank", "noopener");
      else toast.error("Only http(s) links open from an app");
      return;
    }
    if (ev.type === "continue_conversation") {
      const text = assistantInput(
        (ev.action_id !== undefined
          ? activations.current.get(ev.action_id)
          : undefined) ?? applied.current,
        ev.action_id,
        ev.humanFriendlyMessage,
        ev.params.context,
        ev.formState && Object.keys(ev.formState).length > 0
          ? { name: ev.formName, values: ev.formState }
          : undefined,
      );
      // The same paths the composer takes: a steer while the turn runs, else a new input (46 §6.3).
      const st = useSessions.getState();
      if (st.sessions[session]?.running) {
        const id = crypto.randomUUID();
        st.applyEvents([{ type: "InputQueued", session, id, text }]);
        api.steer(session, text, id, []).catch(fail);
      } else {
        st.markSent(session, text);
        api.send_input(session, text, []).catch(fail);
      }
    }
  };

  const paused = !visible || conn !== "open" || !ready;
  return (
    <div className="flex h-full min-h-0 flex-col">
      {banner && (
        <Notice tone={banner.tone}>
          <pre className="font-mono text-xs whitespace-pre-wrap wrap-anywhere">
            {banner.text}
          </pre>
        </Notice>
      )}
      {program.kind === "loading" && !fetching && (
        <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
      )}
      {program.kind === "source" && (
        <>
          <Notice tone="info">{program.reason}</Notice>
          <Source text={program.source} open />
        </>
      )}
      {view && (
        <>
          {view.kind === "preview" && (
            <div className="flex items-center gap-2 px-4 pt-3 text-xs text-fg-tertiary">
              <Loader2 className="size-3.5 animate-spin" />
              {fetching
                ? "published, loading…"
                : draft?.status === "validating"
                  ? "validating…"
                  : draft?.edit
                    ? "applying patch…"
                    : "drafting…"}{" "}
              · controls enable once published
            </div>
          )}
          {view.kind === "preview" && view.mergeError && (
            <Notice tone="error">
              Patch does not merge yet: {view.mergeError}
            </Notice>
          )}
          {active && recordId !== active.record && (
            <Notice tone="info">
              Finishing the current action before switching to the new version…
            </Notice>
          )}
          {fetching && view.kind === "run" && (
            <Notice tone="info">Loading the new version…</Notice>
          )}
          {paused && view.kind === "run" && (
            <Notice tone="info">
              {conn !== "open"
                ? "Offline: reads pause and refresh once reconnected."
                : "Hidden: reads paused."}
            </Notice>
          )}
          <div ref={frame} className="min-h-0 flex-1 px-4 py-3 [&_a]:underline">
            <ThemeProvider mode={dark ? "dark" : "light"}>
              <AppRenderer
                response={view.response}
                library={library}
                isStreaming={view.kind === "preview"}
                generation={generation}
                provider={provider}
                paused={paused}
                onAction={onAction}
                onActionStart={(scope) => {
                  running.current.add(scope.action_id);
                  activations.current.set(scope.action_id, applied.current);
                  setActive({
                    ...scope,
                    record: applied.current.record_call_id,
                  });
                }}
                onActionEnd={(r) => {
                  running.current.delete(r.action_id);
                  activations.current.delete(r.action_id);
                  if (running.current.size === 0) setActive(null);
                  if (!r.ok && r.error !== undefined)
                    toast.error(
                      r.error instanceof Error
                        ? r.error.message
                        : String(r.error),
                    );
                }}
                runtimeRef={runtimeRef}
                onError={setErrors}
              />
            </ThemeProvider>
          </div>
          {empty && view.response.trim() !== "" && (
            <Notice tone={view.kind === "run" ? "error" : "info"}>
              {view.kind === "run"
                ? "Nothing renders: the root is missing or uses a component this desktop does not know."
                : "Waiting for a renderable root…"}
            </Notice>
          )}
          {errors.length > 0 && (
            <Notice tone="error">
              {errors.map((e, i) => (
                <div key={i} className="font-mono text-xs">
                  {e.message}
                  {e.hint ? ` — ${e.hint}` : ""}
                </div>
              ))}
            </Notice>
          )}
          {view.kind === "run" && view.doc.diagnostics.length > 0 && (
            <Notice tone="info">
              {view.doc.diagnostics.map((d, i) => (
                <div key={i} className="font-mono text-xs">
                  {d.at ? `${d.at}: ` : ""}
                  {d.message}
                </div>
              ))}
            </Notice>
          )}
          {view.kind === "preview" && view.tail.trim() !== "" && (
            <div className="border-t border-border-subtle px-4 py-2">
              <div className="text-xs text-fg-tertiary">
                statement in progress
              </div>
              <pre className="font-mono text-xs whitespace-pre-wrap wrap-anywhere text-fg-secondary">
                {view.tail}
              </pre>
            </div>
          )}
          {view.doc && (
            <Strip
              bindings={view.doc.bindings}
              runtime={runtime}
              calls={calls ?? NO_CALLS}
              recent={view.doc.recent_calls}
              streaming={view.kind === "preview"}
              active={active}
            />
          )}
          <Source text={view.response} />
        </>
      )}
      {patch !== undefined && program.kind !== "preview" && (
        <Source text={patch} label="Patch" />
      )}
    </div>
  );
}

function Notice({
  tone,
  children,
}: {
  tone: "error" | "info";
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "mx-4 mt-3 rounded-lg px-3 py-2 text-xs",
        tone === "error" ? "bg-danger-bg text-danger" : "bg-info-bg text-info",
      )}
      role={tone === "error" ? "alert" : "status"}
    >
      {children}
    </div>
  );
}

/** The program text behind a native disclosure; `null` while the file is still being read. */
function Source({
  text,
  label = "Source",
  open,
}: {
  text: string | null;
  label?: string;
  open?: boolean;
}) {
  return (
    <details
      className="border-t border-border-subtle px-4 py-2 text-xs"
      open={open}
    >
      <summary className="cursor-pointer text-fg-tertiary">{label}</summary>
      {text === null ? (
        <div className="mt-2 h-24 animate-pulse rounded-lg bg-layer-2" />
      ) : (
        <pre className="mt-2 max-h-96 overflow-auto rounded-lg bg-layer-2 p-3 font-mono leading-5 whitespace-pre-wrap wrap-anywhere">
          {text}
        </pre>
      )}
    </details>
  );
}

/**
 * Per-statement status under the app (46 §6.1, §8.3): every mutation with its tool and risk, and every query that is
 * not quietly healthy: loading, stale, offline, failed, busy, unknown, or waiting for the user's go-ahead. That
 * Load/Refresh press is the approval and runs the read once; mutations are never retried from here.
 */
function Strip({
  bindings,
  runtime,
  calls,
  recent,
  streaming,
  active,
}: {
  bindings: AppBinding[];
  runtime: Runtime | null;
  calls: ReadonlyMap<string, AppCallChanged>;
  /** The harness's stored pressed invocations for this record (`get_app`), for a view opened after the events passed. */
  recent: AppDoc["recent_calls"];
  streaming: boolean;
  active: { action_id: string; steps: number } | null;
}) {
  const rows = bindings.flatMap((b) => {
    const risk = (
      <span className={cn("rounded px-1.5 py-0.5 text-[11px]", RISK[b.risk])}>
        {b.risk.replaceAll("_", " ")}
      </span>
    );
    const head = (
      <>
        <span className="font-medium">{b.statement_id}</span>
        <span className="text-fg-tertiary">{b.tool}</span>
        {risk}
      </>
    );
    if (b.kind === "mutation") {
      const cause = runtime?.mutation_cause(b.statement_id);
      const status = causeStatus(cause);
      const last = calls.get(b.statement_id);
      const stored = recent.find((c) => c.statement_id === b.statement_id);
      const text = streaming
        ? "runs on press once published"
        : active
          ? "running…"
          : status === "busy"
            ? "harness busy — press again later"
            : status === "unknown"
              ? "outcome unknown — check before repeating"
              : status !== null
                ? `${status.replaceAll("_", " ")}: ${cause instanceof Error ? cause.message : ""}`
                : cause !== undefined && cause !== null
                  ? `failed: ${cause instanceof Error ? cause.message : String(cause)}`
                  : last
                    ? `${last.status.replaceAll("_", " ")} · ${last.ms} ms`
                    : stored
                      ? `last run ${stored.status.replaceAll("_", " ")}${stored.is_error && stored.content ? `: ${stored.content}` : ""}`
                      : "runs on press";
      return [
        <Row key={b.statement_id} head={head}>
          {text}
        </Row>,
      ];
    }
    if (b.kind !== "query") return [];
    if (streaming)
      return [
        <Row key={b.statement_id} head={head}>
          inert until published
        </Row>,
      ];
    const st = runtime?.query_status(b.statement_id);
    if (!st) return [];
    const press = (label: string) => (
      <Button
        size="sm"
        variant="outline"
        className="h-6 px-2 text-xs"
        onClick={() => runtime?.press([b.statement_id])}
      >
        {label}
      </Button>
    );
    let text: ReactNode = null;
    let action: ReactNode = null;
    if (st.state === "loading" || st.state === "refetching")
      text = st.trigger === "press" ? "loading (pressed)…" : "loading…";
    else if (st.state === "error") {
      const status = causeStatus(st.cause);
      const msg =
        st.cause instanceof Error
          ? st.cause.message
          : (st.error?.message ?? "error");
      switch (status) {
        case "needs_approval":
          text = st.has_data
            ? "needs your go-ahead to refresh"
            : "needs your go-ahead to load";
          action = press(st.has_data ? "Refresh" : "Load");
          break;
        case "busy":
          text = "harness busy";
          action = press("Retry");
          break;
        case "unavailable":
          text = `unavailable: ${msg}`;
          break;
        case "disconnected":
          text = "offline — refreshes when reconnected";
          break;
        case "unknown":
          text = "outcome unknown";
          action = press("Retry");
          break;
        // Superseded by newer arguments or a newer generation: the runtime already dropped the result.
        case "aborted":
        case "cancelled":
          return [];
        default:
          text = `error: ${msg}`;
          action = press("Retry");
      }
      if (st.has_data) text = <>{text} · showing stale data</>;
    } else if (st.stale) text = "stale — showing earlier data";
    else return [];
    return [
      <Row key={b.statement_id} head={head} action={action}>
        {text}
      </Row>,
    ];
  });
  if (rows.length === 0) return null;
  return (
    <div className="border-t border-border-subtle px-4 py-2 text-xs">
      {rows}
    </div>
  );
}

function Row({
  head,
  action,
  children,
}: {
  head: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex min-h-7 flex-wrap items-center gap-2 py-0.5">
      {head}
      <span className="text-fg-secondary">{children}</span>
      {action && <span className="ml-auto">{action}</span>}
    </div>
  );
}
