import { type StoreApi, createStore } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { Injected } from '@/lib/injected';
import { codeFromHistory } from '@/lib/codemap/slice';
import type { CodemapSlice } from '@/lib/codemap/types';
import type { CompactStyle } from "@/rpc/bindings/CompactStyle";
import type { ContextEntry } from "@/rpc/bindings/ContextEntry";
import type { JobExit } from "@/rpc/bindings/JobExit";
import type { PeerOrigin } from "@/rpc/bindings/PeerOrigin";
import type { QueuedInput } from "@/rpc/bindings/QueuedInput";
import type { TodoSnapshot } from "@/rpc/bindings/TodoSnapshot";
import type { Usage } from "@/rpc/bindings/Usage";
import type { WireEvent } from "@/rpc/events";
import type {
  AgentInfo,
  AppCallChanged,
  Artifact,
  LoadedPage,
  VisionNote,
  VisionSource,
} from "@/rpc/extra";
import {
  adopt,
  empty,
  fromHistory,
  nextId,
  recordCall,
  reconcileTodos,
  route,
} from "./reduce";
import type { SecretRequested } from "./approvals";
import { type Stores, active, scoped } from "./engine-scope";

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

/** A direct `app` call whose source is still arriving or awaiting commit (46 §8.1), keyed like an artifact by
 * `call_id`; dropped on `ArtifactShown`. Failed/cancelled drafts stay readable as source with their error. */
export type AppDraft = {
  name?: string;
  title?: string;
  /** Source decoded so far from the partial argument envelope; the complete source once the call started. */
  source: string;
  /** `edit: true`: a patch against `base` (the receipt's `base_call_id`), not a whole program. */
  edit: boolean;
  base?: string;
  bytes: number;
  /** `streaming`: arguments arriving; `validating`: started, commit pending; `failed`/`cancelled`: no artifact follows. */
  status: "streaming" | "validating" | "failed" | "cancelled";
  error?: string;
};

/** One transcript row. `id` is the React key; ids are process-unique. `context` is the row's stored
 * identity in the model's context (48 §3.3): its removal target, membership and attachment occurrences.
 * Only history rows carry it; live cards get theirs on the settled reload after the turn. */
export type Entry = { id: string; context?: ContextEntry } & (
  | {
      kind: "user";
      text: string;
      /** A steer whose card moved from the queue into the transcript. */
      injected?: boolean;
      /** `/<skill>` injection: chip name; `text` is the user's words. */
      skill?: string;
      /** Attachment paths sent with the text: optimistic, our own sends only; `context.attachments` supersedes it once stored. */
      files?: string[];
      /** Seq of this row when it opens a turn (29 §5.1); steers and engine notes have none. */
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
      /** `Date.now()` at `ToolCallStarted` / `ToolCallFinished`; live cards only (history rows carry no times). */
      startedAt?: number;
      finishedAt?: number;
    }
  /** `⟵ from` card: a peer message, a subagent result or a job result. `origin` is the engine-authenticated
   * provenance (50 §9) of a peer exchange, from `PeerDelivered` live or `HistoryEntry::User.peer` on replay; a card
   * parsed from a legacy `<message>` wrapper has none and renders a label only, never inspect/answer controls.
   * `seq` is the stored row the live delivery became. */
  | {
      kind: "peer";
      from: string;
      text: string;
      origin?: PeerOrigin;
      seq?: number;
    }
  | Extract<Injected, { kind: 'engine' | 'desk' | 'wake' }>
  /** A vision consultation note (52 §6): a stored user-role row, never a `you` bubble. `text` is the row body
   * (`<vision_observation>…`), empty for a live card until the settled reload; `note` is the typed metadata,
   * absent live. `failed` marks a live `VisionProgress` failure whose per-source statuses are not known yet. */
  | {
      kind: "vision";
      model: string;
      sources: VisionSource[];
      note?: VisionNote;
      text: string;
      seq?: number;
      failed?: boolean;
    }
  | { kind: "info"; text: string }
  | { kind: "error"; text: string }
  | { kind: "compacted"; epoch: number; style: CompactStyle; summary?: string }
);

/** What the status dot says. `harvesting` is never stored in `activity`: a
 * detached harvest (10 §5.2 as built) sets `harvesting` and shows only while
 * the session is otherwise idle — see `shownActivity`. */
export type Activity =
  | "idle"
  | "thinking"
  | "streaming"
  | "reasoning"
  | "tool"
  | "compacting"
  | "harvesting"
  | "vision";

/** The activity to show: the turn's own, or `harvesting` while a detached harvest runs on an otherwise idle session. */
export function shownActivity(activity: Activity | undefined, harvesting: boolean | undefined): Activity {
  const a = activity ?? "idle";
  return a === "idle" && harvesting ? "harvesting" : a;
}

/** Everything the transcript and sidebar need for one session. */
export type SessionSlice = {
  id: string;
  entries: Entry[];
  running: boolean;
  /** Turn started, no delta yet. */
  waiting: boolean;
  activity: Activity;
  /** A detached harvest is in flight (`Harvesting` … `Harvested`); independent of `running`/`activity`. */
  harvesting: boolean;
  /** Status line while `activity === 'vision'` (52 §6): which model is asking or inspecting how many images. */
  visionStatus?: string;
  model?: string;
  effort?: string | null;
  usage?: Usage;
  /** Tokens the window holds after the last model call. */
  context?: number;
  /** `context` is the actor's estimate after a context edit/rewind or a fresh load, not provider usage (48 §3.2). */
  contextEstimated: boolean;
  /** Store revision of the loaded history (48 §3.1); `remove_context`'s `expected_revision`. Unset before the first load. */
  contextRevision?: string;
  /** The actor accepts context edits (idle native session, not stale). */
  contextEditable: boolean;
  /** Why not editable, from the server (`busy`, `external harness`, …). */
  contextReason?: string | null;
  /** A `ContextChanged` arrived (or a settled reload was refused) and the display predates it: controls disable until the reload lands. */
  contextStale: boolean;
  /** Bumped by every invalidation (context edit, reconnect, engine switch, snapshot install); a page or snapshot requested under an older value is discarded. */
  historyGeneration: number;
  /** Bumped by turn/edit activity (deltas, tool calls, interjections, context changes); a settled snapshot requested under an older value is discarded. */
  live: number;
  /** `defer` (62 §3.1): lands at the next turn, not after this tool batch; unknown after a reload (`list_queued` lacks it). */
  queued: (QueuedInput & { defer?: boolean })[];
  skills: string[];
  name?: string | null;
  parent?: string | null;
  agent?: string | null;
  /** Model the external engine reports for its session (06 §8.2); the header shows `agent · harnessModel`. */
  harnessModel?: string | null;
  undone: number;
  /** `load_session` has run at least once. */
  loaded: boolean;
  /** Older history exists before `firstSeq`. */
  more: boolean;
  firstSeq: number | null;
  /** Subtree rows for the agent tree, keyed by session id. */
  children: Record<string, AgentInfo>;
  title?: string;
  /** Keywords the title model assigned with the title (live from `TitleChanged`); the session list carries the stored set. */
  tags?: string[];
  /** Turn whose `InputInjected` already moved a steer into the transcript; its `TurnStarted` echo is skipped. */
  lastInjected?: string;
  /** Opener seq of the running (or last) turn, from `TurnStarted`. */
  turnSeq?: number;
  /** A `browser_*` tool ran in this session: the header shows the live-view globe. */
  browser: boolean;
  /** The live browser dialog is open (globe click or `BrowserShowLive`). */
  browserLive: boolean;
  /** A PTY job started in this session (`JobStarted.pty`): the header shows the terminals icon. */
  terminal: boolean;
  /** PTY jobs whose screen is still changing: added on `JobStarted.pty`/`JobScreen`, dropped on `JobIdle`/`JobFinished`. The sidebar's live-terminal glyph. */
  ptys: Set<string>;
  /** Non-PTY background jobs still running (`JobStarted` until `JobFinished`). The sidebar's background-job glyph. */
  jobs: Set<string>;
  /** Artifacts by `call_id` of the `show` call that made them (35 §7.3); fed by `ArtifactShown` and `list_artifacts`. */
  artifacts: Map<string, Artifact>;
  /** Tool calls whose arguments are still streaming (`ToolCallProgress`), by `call_id`; cleared on `ToolCallStarted`.
   * `writing` = a `show` call carrying `content` (writing a file) rather than a `path` (showing one). */
  progress: Map<string, { name?: string; writing?: boolean; bytes: number }>;
  /** Live crawl/scrape URLs by `call_id`; dropped on finish, the card then parses the result. */
  scrape: Map<string, { inflight: string[]; done: string[]; limit: number }>;
  /** Pending/failed direct `app` calls by `call_id` (46 §8.1); a panel opened on one is promoted in place by `ArtifactShown`. */
  appDrafts: Map<string, AppDraft>;
  /** Latest `AppCallChanged` per statement of each app record (46 §6.4), so a reopened view still shows what its
   * presses did; bounded to the newest `APP_CALL_RECORDS` records. */
  appCalls: Map<string, Map<string, AppCallChanged>>;
  /** `call_id` of the artifact open in the side panel; `null` when closed. */
  artifactPanel: string | null;
  /** 73 §4: roots touched by `code_*` tools, the last call, the activity trail and this session's edits. */
  codemap: CodemapSlice;
  /** Root shown in the docked CodeMap pane; `null` when closed. */
  codemapPanel: string | null;
  /** The plan (47 §5): the newest snapshot seen from any source; `null` before one arrives (unavailable, not empty). */
  todos: TodoSnapshot | null;
  /** A reconnect, `Lagged` or engine switch may have dropped a `TodoChanged`: the list is labelled stale until
   * an authoritative snapshot (event, `get_todos`, a reload read after the gap) confirms or replaces it. */
  todosStale: boolean;
  /** The todo card is expanded (47 §6.1); survives snapshots and session switches within this client. */
  todosExpanded: boolean;
};

/** What a page or settled snapshot must still match when it lands (48 §5): `historyGeneration` and `live` at request time. */
export type Freshness = { gen: number; live: number };

/** Snapshot the freshness of `s` before an await. */
export const freshness = (s: SessionSlice): Freshness => ({
  gen: s.historyGeneration,
  live: s.live,
});

type SessionsStore = {
  sessions: Record<string, SessionSlice>;
  /** Texts we sent ourselves, so the `TurnStarted` echo is skipped once. */
  sent: Set<string>;
  /** Events the server dropped; the UI reloads and resets it. */
  lagged: number;
  /** Fold a frame's events into the touched slices with one `set`. */
  applyEvents(events: WireEvent[]): void;
  /**
   * Install a `load_session` reply. `prepend` adds an older page (discarded when `fresh.gen` moved: a
   * pre-edit page would reintroduce removed payload). `replace` installs the newest page unconditionally,
   * left marked stale when `fresh.gen` moved. `settled` is the mutation refresh (48 §5): installed only
   * when the reply is `settled`, nothing raced the read (`fresh` still matches) and no turn runs; a
   * refusal keeps the display, its stale flag and every event already applied. Returns whether installed.
   */
  loadHistory(
    id: string,
    loaded: LoadedPage,
    mode: "replace" | "prepend" | "settled",
    fresh?: Freshness,
  ): boolean;
  /** Show our own input at once and skip its echo. */
  markSent(id: string, text: string, files?: string[]): void;
  /** The send failed: drop the row `markSent` showed and its echo skip. */
  unmarkSent(id: string, text: string): void;
  /** Drop the turn opened at `turn` and everything after it (before `regenerate`/`edit_turn`); no-op when absent. */
  truncate(id: string, turn: number): void;
  ensure(id: string): void;
  /** Replace the subtree rows from `list_agents`. */
  setChildren(id: string, agents: AgentInfo[]): void;
  /** Open or close the live browser dialog. */
  setBrowserLive(id: string, open: boolean): void;
  /** Replace the artifacts from `list_artifacts` (session load). */
  setArtifacts(id: string, artifacts: Artifact[]): void;
  /** Open the artifact panel on `callId`, or close it with `null`. */
  setArtifactPanel(id: string, callId: string | null): void;
  /** Open the CodeMap pane on `root`, or close it with `null`. */
  setCodemapPanel(id: string, root: string | null): void;
  /** Record a press outcome the view learned directly (46 §6.4), as if its `AppCallChanged` had arrived. */
  recordAppCall(id: string, e: AppCallChanged): void;
  /** Install a `get_todos` reply (47 §5.4): a higher version replaces, an equal one repairs a stale mark, an older or `null` one is ignored. */
  setTodos(id: string, snapshot: TodoSnapshot | null): void;
  /** Expand or collapse the todo card. */
  setTodosExpanded(id: string, expanded: boolean): void;
  /** A gap may hide a `TodoChanged`: label the list stale (one session, or every session) until a snapshot confirms it. */
  markTodosStale(id?: string): void;
  resetLagged(): void;
  /** Forget that any transcript was loaded: the chat route refetches on its next mount. */
  unload(): void;
};

export type SessionsApi = StoreApi<SessionsStore>;

/** One engine's sessions (31 §7.4); `useSessions` reads the enclosing engine's. */
export const createSessions = (): SessionsApi =>
  createStore<SessionsStore>()(
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
          set({
            sessions: changed ? draft : sessions,
            lagged: lagged + dropped,
          });
      },
      loadHistory(id, loaded, mode, fresh) {
        const before = get().sessions[id] ?? empty(id);
        const moved =
          fresh !== undefined && before.historyGeneration !== fresh.gen;
        // The plan rides on every page and reconciles by version (47 §5.4): a refused or older page never rolls
        // it back, and only a read issued after the last invalidation clears a stale mark.
        const cur = reconcileTodos(before, loaded.todos, !moved);
        const refused =
          (mode === "prepend" && moved) ||
          // A newer snapshot already landed (or a reconnect will bring one): a superseded full page has nothing to add.
          (mode === "replace" && moved && cur.loaded) ||
          (mode === "settled" &&
            (!fresh ||
              moved ||
              cur.live !== fresh.live ||
              cur.running ||
              !loaded.settled ||
              loaded.running));
        if (refused) {
          if (cur !== before)
            set((st) => ({ sessions: { ...st.sessions, [id]: cur } }));
          return false;
        }
        const entries = loaded.history.flatMap(fromHistory);
        const browser =
          cur.browser ||
          loaded.history.some(
            (h) => h.role === "tool" && h.name.startsWith("browser_"),
          );
        const codemap = codeFromHistory(
          cur.codemap,
          loaded.history.flatMap((h) => (h.role === "tool" ? [h.name] : [])),
        );
        let next: SessionSlice;
        if (mode === "prepend") {
          next = {
            ...cur,
            browser,
            codemap,
            entries: [...entries, ...cur.entries],
            firstSeq: loaded.first_seq ?? cur.firstSeq,
            more: loaded.more,
          };
        } else {
          const stale = mode === "replace" && moved;
          // A real usage figure from `TurnFinished` outlives a settled reconcile; an estimate (or nothing) yields to the server's.
          const estimate = loaded.context_estimate ?? undefined;
          const useEstimate =
            estimate !== undefined &&
            (cur.contextEstimated || cur.context === undefined);
          next = {
            ...cur,
            browser,
            codemap,
            entries: adopt(cur.entries, entries),
            running: loaded.running,
            waiting: false,
            activity: loaded.running ? "thinking" : "idle",
            model: loaded.model,
            effort: loaded.effort,
            queued: loaded.queued,
            undone: loaded.undone,
            agent: loaded.agent,
            harnessModel: loaded.harness_model ?? cur.harnessModel,
            name: loaded.name,
            parent: loaded.parent,
            skills: loaded.skills,
            loaded: true,
            firstSeq: loaded.first_seq,
            more: loaded.more,
            context: useEstimate ? estimate : cur.context,
            contextEstimated: useEstimate || cur.contextEstimated,
            contextRevision: loaded.context_revision,
            contextEditable: loaded.context_editable && !stale,
            contextReason: loaded.context_reason,
            contextStale: stale,
            // An older page still in flight was read before this snapshot: it must not land on top of it.
            historyGeneration: cur.historyGeneration + 1,
          };
        }
        set((st) => ({ sessions: { ...st.sessions, [id]: next } }));
        return true;
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
      unmarkSent(id, text) {
        get().sent.delete(text);
        const cur = get().sessions[id];
        const at = cur?.entries.findLastIndex(
          (e) => e.kind === "user" && e.turn === undefined && e.text === text,
        );
        if (!cur || at === undefined || at < 0) return;
        set((st) => ({
          sessions: {
            ...st.sessions,
            [id]: { ...cur, entries: cur.entries.toSpliced(at, 1) },
          },
        }));
      },
      truncate(id, turn) {
        const cur = get().sessions[id];
        if (!cur) return;
        const at = cur.entries.findIndex(
          (e) =>
            (e.kind === "user" || e.kind === "assistant") && e.turn === turn,
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
      setArtifacts(id, artifacts) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return {
            sessions: {
              ...st.sessions,
              [id]: {
                ...cur,
                artifacts: new Map(artifacts.map((a) => [a.call_id, a])),
              },
            },
          };
        });
      },
      setArtifactPanel(id, callId) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return cur.artifactPanel === callId
            ? st
            : {
                sessions: {
                  ...st.sessions,
                  [id]: { ...cur, artifactPanel: callId },
                },
              };
        });
      },
      setCodemapPanel(id, root) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return cur.codemapPanel === root
            ? st
            : { sessions: { ...st.sessions, [id]: { ...cur, codemapPanel: root } } };
        });
      },
      recordAppCall(id, e) {
        set((st) => ({
          sessions: {
            ...st.sessions,
            [id]: recordCall(st.sessions[id] ?? empty(id), e),
          },
        }));
      },
      setTodos(id, snapshot) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          const next = reconcileTodos(cur, snapshot, true);
          return next === cur
            ? st
            : { sessions: { ...st.sessions, [id]: next } };
        });
      },
      setTodosExpanded(id, expanded) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return cur.todosExpanded === expanded
            ? st
            : {
                sessions: {
                  ...st.sessions,
                  [id]: { ...cur, todosExpanded: expanded },
                },
              };
        });
      },
      markTodosStale(id) {
        set((st) => ({
          sessions: Object.fromEntries(
            Object.entries(st.sessions).map(([k, s]) => [
              k,
              id === undefined || k === id ? { ...s, todosStale: true } : s,
            ]),
          ),
        }));
      },
      resetLagged() {
        set({ lagged: 0 });
      },
      unload() {
        set((st) => ({
          sessions: Object.fromEntries(
            Object.entries(st.sessions).map(([id, s]) => [
              id,
              {
                ...s,
                loaded: false,
                contextStale: s.loaded || s.contextStale,
                todosStale: true,
                historyGeneration: s.historyGeneration + 1,
              },
            ]),
          ),
        }));
      },
    })),
  );

export const useSessions = scoped((st) => st.sessions);

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

/** Route one frame's events into `st` (the engine they came from): session slices, approvals, asks, secret requests, then `on` listeners. */
export function dispatch(events: WireEvent[], st: Stores = active()): void {
  st.sessions.getState().applyEvents(events);
  const approvals = st.approvals.getState();
  const asks = st.asks.getState();
  const secrets = st.secretRequests.getState();
  const live = st === active();
  for (const e of events) {
    if (e.type === "PendingPrompts") {
      approvals.replace(e.approvals);
      asks.replace(e.asks);
      secrets.replace(
        e.secrets.filter((s): s is SecretRequested => s.type === "SecretRequested"),
      );
    } else if (e.type === "ApprovalRequested") approvals.add(e);
    else if (e.type === "ApprovalResolved") approvals.resolve(e.id);
    else if (e.type === "AskRequested") asks.add(e);
    else if (e.type === "AskResolved") asks.resolve(e.id);
    else if (e.type === "SecretRequested") secrets.add(e);
    else if (e.type === "SecretResolved") secrets.resolve(e.id);
    if (live) listeners.get(e.type)?.forEach((cb) => cb(e));
  }
}
