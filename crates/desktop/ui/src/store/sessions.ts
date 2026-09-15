import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { CompactStyle } from "@/rpc/bindings/CompactStyle";
import type { JobExit } from "@/rpc/bindings/JobExit";
import type { QueuedInput } from "@/rpc/bindings/QueuedInput";
import type { Usage } from "@/rpc/bindings/Usage";
import type { WireEvent } from "@/rpc/events";
import type { AgentInfo, LoadedPage } from "@/rpc/extra";
import { empty, fromHistory, nextId, route } from "./reduce";
import { useApprovals } from "./approvals";

/** Live tail of a `shell` card's background job. */
export type JobTail = {
  id: string;
  command: string;
  /** Output rows as `lib/jobTail` keeps them: the last element is the line in progress. */
  lines: string[];
  /** Latest PTY screen (22 §5); replaces `lines` when present. */
  screen?: { rows: string[]; cursor: [number, number] };
  /** Set once the job ended and its result was not folded into the tool output. */
  exit?: { exit: JobExit; secs: number };
};

/** `(output, is_error)` of a finished tool call plus files it produced. */
export type ToolResult = { output: string; isError: boolean; files: string[] };

/** One transcript row. `id` is the React key; ids are process-unique. */
export type Entry = { id: string } & (
  | {
      kind: "user";
      text: string;
      /** A steer whose card moved from the queue into the transcript. */
      injected?: boolean;
      /** `/<skill>` injection: chip name; `text` is the user's words. */
      skill?: string;
      /** Attachment paths sent with the text (only known for our own sends). */
      files?: string[];
      /** Seq of this row when it opens a turn (29 §5.1); steers and harness notes have none. */
      turn?: number;
    }
  | {
      kind: "assistant";
      text: string;
      open: boolean;
      /** Opener seq of the turn this reply belongs to. */
      turn?: number;
    }
  | { kind: "reasoning"; text: string; done: boolean }
  | {
      kind: "tool";
      callId: string;
      name: string;
      /** Raw arguments; `undefined` while `pending`. */
      args: unknown;
      /** `ToolCallPending` seen, `ToolCallStarted` not yet. */
      pending: boolean;
      /** Called from inside a `python` cell (call id contains `.`); rendered `↳ name`. */
      child: boolean;
      result?: ToolResult;
      job?: JobTail;
    }
  /** `⟵ from` card: a peer message, a subagent result or a job result. */
  | { kind: "peer"; from: string; text: string }
  | { kind: "info"; text: string }
  | { kind: "error"; text: string }
  | { kind: "compacted"; epoch: number; style: CompactStyle; summary?: string }
);

/** What the status dot says. */
export type Activity =
  | "idle"
  | "thinking"
  | "streaming"
  | "reasoning"
  | "tool"
  | "compacting"
  | "harvesting";

/** Everything the transcript and sidebar need for one session. */
export type SessionSlice = {
  id: string;
  entries: Entry[];
  running: boolean;
  /** Turn started, no delta yet. */
  waiting: boolean;
  activity: Activity;
  model?: string;
  effort?: string | null;
  usage?: Usage;
  /** Tokens the window holds after the last model call. */
  context?: number;
  queued: QueuedInput[];
  skills: string[];
  name?: string | null;
  parent?: string | null;
  agent?: string | null;
  undone: number;
  /** `load_session` has run at least once. */
  loaded: boolean;
  /** Older history exists before `firstSeq`. */
  more: boolean;
  firstSeq: number | null;
  /** Subtree rows for the agent tree, keyed by session id. */
  children: Record<string, AgentInfo>;
  title?: string;
  /** Turn whose `InputInjected` already moved a steer into the transcript; its `TurnStarted` echo is skipped. */
  lastInjected?: string;
  /** Opener seq of the running (or last) turn, from `TurnStarted`. */
  turnSeq?: number;
  /** A `browser_*` tool ran in this session: the header shows the live-view globe. */
  browser: boolean;
  /** The live browser dialog is open (globe click or `BrowserShowLive`). */
  browserLive: boolean;
};

type SessionsStore = {
  sessions: Record<string, SessionSlice>;
  /** Texts we sent ourselves, so the `TurnStarted` echo is skipped once. */
  sent: Set<string>;
  /** Events the server dropped; the UI reloads and resets it. */
  lagged: number;
  /** Fold a frame's events into the touched slices with one `set`. */
  applyEvents(events: WireEvent[]): void;
  loadHistory(
    id: string,
    loaded: LoadedPage,
    mode: "replace" | "prepend",
  ): void;
  /** Show our own input at once and skip its echo. */
  markSent(id: string, text: string, files?: string[]): void;
  /** Drop the turn opened at `turn` and everything after it (before `regenerate`/`edit_turn`); no-op when absent. */
  truncate(id: string, turn: number): void;
  ensure(id: string): void;
  /** Replace the subtree rows from `list_agents`. */
  setChildren(id: string, agents: AgentInfo[]): void;
  /** Open or close the live browser dialog. */
  setBrowserLive(id: string, open: boolean): void;
  resetLagged(): void;
};

export const useSessions = create<SessionsStore>()(
  subscribeWithSelector((set, get) => ({
    sessions: {},
    sent: new Set(),
    lagged: 0,
    applyEvents(events) {
      const { sessions, sent, lagged } = get();
      const draft = { ...sessions };
      let changed = false;
      let dropped = 0;
      for (const e of events) {
        if (e.type === "Lagged") dropped += e.count;
        else changed = route(draft, e, { sent }) || changed;
      }
      if (changed || dropped)
        set({ sessions: changed ? draft : sessions, lagged: lagged + dropped });
    },
    loadHistory(id, loaded, mode) {
      set((st) => {
        const cur = st.sessions[id] ?? empty(id);
        const fresh = loaded.history.flatMap(fromHistory);
        const browser =
          cur.browser ||
          loaded.history.some(
            (h) => h.role === "tool" && h.name.startsWith("browser_"),
          );
        const next: SessionSlice =
          mode === "prepend"
            ? {
                ...cur,
                browser,
                entries: [...fresh, ...cur.entries],
                firstSeq: loaded.first_seq ?? cur.firstSeq,
                more: loaded.more,
              }
            : {
                ...cur,
                browser,
                entries: fresh,
                running: loaded.running,
                waiting: false,
                activity: loaded.running ? "thinking" : "idle",
                model: loaded.model,
                effort: loaded.effort,
                queued: loaded.queued,
                undone: loaded.undone,
                agent: loaded.agent,
                name: loaded.name,
                parent: loaded.parent,
                skills: loaded.skills,
                loaded: true,
                firstSeq: loaded.first_seq,
                more: loaded.more,
              };
        return { sessions: { ...st.sessions, [id]: next } };
      });
    },
    markSent(id, text, files) {
      get().sent.add(text);
      set((st) => {
        const cur = st.sessions[id] ?? empty(id);
        const entry: Entry = {
          id: nextId(),
          kind: "user",
          text,
          ...(files?.length ? { files } : {}),
        };
        return {
          sessions: {
            ...st.sessions,
            [id]: { ...cur, entries: [...cur.entries, entry] },
          },
        };
      });
    },
    truncate(id, turn) {
      const cur = get().sessions[id];
      if (!cur) return;
      const at = cur.entries.findIndex(
        (e) => (e.kind === "user" || e.kind === "assistant") && e.turn === turn,
      );
      if (at < 0) return;
      set((st) => ({
        sessions: {
          ...st.sessions,
          [id]: { ...cur, entries: cur.entries.slice(0, at) },
        },
      }));
    },
    ensure(id) {
      if (!get().sessions[id])
        set((st) => ({ sessions: { ...st.sessions, [id]: empty(id) } }));
    },
    setChildren(id, agents) {
      set((st) => {
        const cur = st.sessions[id] ?? empty(id);
        const children = Object.fromEntries(agents.map((a) => [a.id, a]));
        return { sessions: { ...st.sessions, [id]: { ...cur, children } } };
      });
    },
    setBrowserLive(id, open) {
      set((st) => {
        const cur = st.sessions[id] ?? empty(id);
        return {
          sessions: { ...st.sessions, [id]: { ...cur, browserLive: open } },
        };
      });
    },
    resetLagged() {
      set({ lagged: 0 });
    },
  })),
);

const NO_ENTRIES: Entry[] = [];

/** The whole slice; re-renders on any change to this session only. */
export const useSession = (id: string) => useSessions((s) => s.sessions[id]);

/** The transcript rows (a stable empty array before the session exists). */
export const useEntries = (id: string) =>
  useSessions((s) => s.sessions[id]?.entries ?? NO_ENTRIES);

/** One field of the slice; re-renders only when that field changes. */
export function useSessionField<K extends keyof SessionSlice>(
  id: string,
  key: K,
): SessionSlice[K] | undefined {
  return useSessions((s) => s.sessions[id]?.[key]);
}

type Listener<T extends WireEvent["type"]> = (
  e: Extract<WireEvent, { type: T }>,
) => void;

const listeners = new Map<string, Set<(e: WireEvent) => void>>();

/** Subscribe to one event type outside the stores (`McpProgress`, `LoginFinished`, `KnowledgeChanged`, …). Returns the unsubscribe. */
export function on<T extends WireEvent["type"]>(
  type: T,
  cb: Listener<T>,
): () => void {
  let set = listeners.get(type);
  if (!set) listeners.set(type, (set = new Set()));
  // The map is keyed by `type`, so every event reaching `cb` has type `T`; the compiler cannot see the key.
  const l = (e: WireEvent) => cb(e as Extract<WireEvent, { type: T }>);
  set.add(l);
  return () => {
    set.delete(l);
  };
}

/** Route one frame's events: session slices, approvals, then `on` listeners. */
export function dispatch(events: WireEvent[]): void {
  useSessions.getState().applyEvents(events);
  const approvals = useApprovals.getState();
  for (const e of events) {
    if (e.type === "ApprovalRequested") approvals.add(e);
    else if (e.type === "ApprovalResolved") approvals.resolve(e.id);
    listeners.get(e.type)?.forEach((cb) => cb(e));
  }
}
