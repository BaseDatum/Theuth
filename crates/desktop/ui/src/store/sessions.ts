import type { TaggedTopicPayloads, TopicEvent, TopicKey, TopicPayloads } from '@/rpc/bindings/client';
import { type StoreApi, createStore } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { CellResultInput, ContactNote, FileNote, Injected, ParentViewLine, RestartNote } from '@/lib/injected';
import { codeFromHistory } from '@/lib/codemap/slice';
import type { CodemapSlice } from '@/lib/codemap/types';
import { useRemoteStart } from '@/lib/remote-start';
import type { AdvisoryNote } from "@/rpc/bindings/method.agent.output"
import type { CompactStyle } from "@/rpc/bindings/method.agent.output"
import type { ChannelOrigin } from "@/rpc/bindings/method.agent.output"
import type { ContextEntry } from "@/rpc/bindings/method.agent.output"
import type { HarnessCommand } from "@/rpc/bindings/method.agent.output"
import type { JobExit } from "@/rpc/bindings/method.agent.output"
import type { LiveCountsRow } from "@/rpc/bindings/method.agent.output"
import type { ContactFile, PeerOrigin } from "@/rpc/bindings/method.agent.output"
import type { ObservedMeta } from "@/rpc/bindings/method.agent.output"
import type { QueuedInput } from "@/rpc/bindings/method.agent.output"
import type { SessionKind } from "@/rpc/bindings/method.agent.output"
import type { ResumeCause } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { TodoSnapshot } from "@/rpc/bindings/method.agent.output"
import type { PendingReminderView } from "@/rpc/bindings/method.agent.output"
import type { Usage } from "@/rpc/bindings/event.agent_2eevents.output"
import type { UiEvent as WireEvent } from '@/store/ui-events'
import type { AgentInfo } from '@/store/ui-events'
import type { AppCallChanged } from "@/rpc/bindings/event.app_2ecalls.output"
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import type { Loaded as LoadedPage, VisionNote, VisionSource } from "@/rpc/bindings/method.agent.output"
import {
  adopt,
  empty,
  fromHistoryPage,
  lastOpenedBy,
  interruptDangling,
  nextId,
  openTail,
  recordCall,
  reconcileTodos,
  reseedRunning,
  resumeFromLoaded,
  route,
  settleCompacting,
  shorterTail,
  stampStored,
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
 * Only history rows carry it; live cards get theirs on the settled reload after the turn. `at` is the row's
 * wall-clock time in ms (74 §5): `context.at` on load, `Date.now()` for a live card; absent for rows stored
 * before the column existed. */
export type Entry = { id: string; context?: ContextEntry; at?: number } & (
  | {
      kind: "user";
      text: string;
      /** A steer whose card moved from the queue into the transcript. */
      injected?: boolean;
      /** `/<skill>` injection: the skill names joined by `, ` (one card each); `text` is the user's words. */
      skill?: string;
      /** Attachment paths sent with the text: optimistic, our own sends only; `context.attachments` supersedes it once stored. */
      files?: string[];
      /** Drawn from another client's `TurnStarted` echo, which carries no attachments: the stored row's `context` (its
       * `attachments`) is read mid-turn and copied on (`stampStored`), so its images show before the turn ends. */
      echo?: true;
      /** Seq of this row when it opens a turn (29 §5.1); steers and engine notes have none. */
      turn?: number;
      /** 172 D3: the channel that delivered it (Slack, Signal, Telegram); absent for desktop input and rows stored before 172. */
      channel?: ChannelOrigin;
      /** 171F B4: the parked cell results this message carried into its turn (live `splitLeading`, reload `foldCells`). */
      cells?: CellResultInput[];
    }
  | {
      kind: "assistant";
      text: string;
      open: boolean;
      /** Opener seq of the turn this reply belongs to. */
      turn?: number;
    }
  | {
      kind: "reasoning";
      text: string;
      done: boolean;
      /** 91: ms the model thought; shown as "Thought 9s" once `done`. */
      ms?: number;
      /** Client clock of the first delta: the fallback `ms` when the engine sends no `ReasoningDone`. */
      startedAt?: number;
    }
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
      /** 146 D7: a dead ACP child's trailing call, loaded with `running === false`: greyed, no spinner. */
      interrupted?: true;
      /** 183 §12.4: the `<engine_restart/>` note the result opened with (the first call of a resumed batch). */
      restart?: RestartNote & { text: string };
      /** 186: the full-collaboration contact (its `peer:` address) whose message opened this call's turn: the typed
       * opener (`PeerOrigin.contact_mode`), stamped live from `turnBy` and on reload from the page's opener rows. */
      by?: string;
    }
  /** `⟵ from` card: a peer message, a subagent result or a job result. `origin` is the engine-authenticated
   * provenance (50 §9) of a peer exchange, from `PeerDelivered` live or `HistoryEntry::User.peer` on replay; a card
   * parsed from a legacy `<message>` wrapper has none and renders a label only, never inspect/answer controls.
   * `seq` is the stored row the live delivery became. A stored row a contact wrapper opened (183 §11.5/§11.6) carries
   * its inner words as `text` and the wrapper's facts and harness note as `contact`; the tag itself is never kept. */
  | {
      kind: "peer";
      from: string;
      text: string;
      origin?: PeerOrigin;
      seq?: number;
      contact?: ContactNote & { note: string };
      /** 199: the files the message carries — the row's `state="receiving"` placeholders on replay, `PeerDelivered.files`
       * live, as the same notes; the card's current state comes from `SessionSlice.files` and the notice rows. */
      files?: FileNote[];
    }
  | Extract<Injected, { kind: 'engine' | 'desk' | 'wake' | 'resume' | 'lost' | 'child' | 'autoresearch' | 'moved' | 'engine_restart' | 'contact_inbox' | 'contact_message' | 'file' }>
  /** 171 §9.1: a cell result the agent read; `turn` when its stored row opened the turn (`foldCells` hands it on). */
  | (CellResultInput & { turn?: number })
  /** 152 D3: the view an endless parent handed a native child before its brief (137 R1-D1); folded, the lines render
   * only while open. Reload/reseed only: a live `TurnStarted` carries the brief without the view. */
  | { kind: "parentView"; lines: ParentViewLine[]; bytes: number }
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
  /** 125 §8.3: one Advisory card — every note the advisor raised at one step boundary (or one steer). From the
   * `advisory` history row or `AdvisoryAdded` live (`seq` is the stored row, the idempotence key); parsed from the
   * `<advisory>` wrapper text when a row arrives as plain user input (no `seq` then). Never a `you` bubble.
   * R1: a consultation card carries its `question`; `notes` is then the one `advice` answer. */
  | { kind: "advisory"; notes: AdvisoryNote[]; seq?: number; question?: string }
  /** `label`: 201 R3-2 — a harness note's collapsed line (`Ledger updated`); `text` shows when expanded. */
  | { kind: "info"; text: string; label?: string }
  /** `child`: the subagent whose error a parent relays (`↳ name error: …`); the card attributes it. */
  | { kind: "error"; text: string; child?: string }
  | { kind: "compacted"; epoch: number; style: CompactStyle; summary?: string }
  /** 109 §8.2: the partial an interrupted model call streamed; greyed, removed by the regenerated reply's first delta. */
  | { kind: "interrupted"; text: string }
);

/** 109 §8.2: the session banner's state — a turn interrupted by a restart (or suspended by a planned stop) and
 * what its resume did. From `Loaded.resume` on load, `TurnResumed` / `ResumeReport` / `TurnSuspended` live.
 * 139 §9.3: `owner`/`here` say who runs it (a replica displays, never "resuming…"); `cause`/`from` the transfer. */
export type ResumeBanner = {
  turn: string;
  kind: "interrupted" | "suspended";
  /** When the turn was interrupted (ms since epoch). */
  at: number;
  /** `TurnResumed` seen: the engine is replaying. Meaningful only when `here`. */
  resumed: boolean;
  /** `ResumeReport` counts — live, or `Loaded.resume.report` persisted by the engine; absent until one arrives. */
  report?: { replayed: number; skipped: string[]; lostJobs: number };
  /** 139 D2: the engine running the turn when it is not the one we talk to (`sessions.owner`). */
  owner?: string;
  /** 139 D2: the session is owned by the engine that answered; `true` from pre-139 engines. */
  here: boolean;
  /** 139 §6.3: why the turn was / will be resumed. */
  cause?: ResumeCause;
  /** 139 §6.3: the previous owner for `transfer` / `takeover` (live events only; a reload loses the name). */
  from?: string;
};

/** 181 D4: one deliberate move of the session, as this client saw it (ms since epoch; lost on reload). */
export type Moved = { to?: string; from?: string; at: number };

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

/** 212 §12: the agent's status line outranks the activity word and the vision line, but not `compacting…` (as in the
 * TUI). `undefined` = show the activity word. */
export function statusLine(activity: Activity, statusText: string | undefined, visionStatus?: string): string | undefined {
  if (activity === "compacting") return undefined;
  return statusText || (activity === "vision" ? visionStatus : undefined) || undefined;
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
  /** 212 §12: the agent's own one-line status (`StatusChanged`); replaces the activity word until the turn ends. */
  statusText?: string;
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
  /** `defer` (62 §3.1): lands at the next turn, not after this tool batch; unknown after a reload (`list_queued` lacks it).
   * `QueuedInput.channel` (172 D3) and `files` (171F4: the attachments, shown on the chip) survive a reload and move
   * to the transcript row on `InputInjected`; our own steer has its `files` from the composer at once. */
  queued: (QueuedInput & { defer?: boolean })[];
  skills: string[];
  name?: string | null;
  parent?: string | null;
  agent?: string | null;
  /** 141 §5.1: `observed` is a watch-only harness subagent — no composer, no model menu, an `ObservedBar` instead. */
  kind?: SessionKind;
  /** 141: the watch-only facts (source, state, cost rule, live detail) when `kind === 'observed'`. */
  observed?: ObservedMeta | null;
  /** Model the external engine reports for its session (06 §8.2); the header shows `agent · harnessModel`. */
  harnessModel?: string | null;
  /** The ACP child's command manifest (75 §3): `skill:<name>` entries are loaded skills, the rest commands. */
  harnessCommands: HarnessCommand[];
  /** The harness's context window when it reports one (75 §5); the catalog limit otherwise. */
  contextSize?: number | null;
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
  /** "What's next?" after the last turn (`Suggestions`): ephemeral, never stored; cleared by the next turn or a pick. */
  suggestions?: string[];
  /** Turn whose `InputInjected` already moved a steer into the transcript; its `TurnStarted` echo is skipped. */
  lastInjected?: string;
  /** Opener seq of the running (or last) turn, from `TurnStarted`. */
  turnSeq?: number;
  /** 186: the `peer:` address of the full-collaboration contact whose message opened the running turn: the peer row
   * whose seq is `turnSeq` (`PeerDelivered` or `TurnStarted`, whichever lands second, sets it). */
  turnBy?: string;
  /** 125 §3: the advisor model the last `AdvisorChanged` named (`null` = off); the reducer prints `advisor → …` when it moves. */
  advisorModel?: string | null;
  /** A `browser_*` tool ran in this session: the header shows the live-view globe. */
  browser: boolean;
  /** The live browser dialog is open (globe click or `BrowserShowLive`). */
  browserLive: boolean;
  /** A PTY job started in this session (`JobStarted.pty`): the header shows the terminals icon. */
  terminal: boolean;
  /** PTY jobs whose screen is still changing: added on `JobStarted.pty`/`JobScreen`, dropped on `JobIdle`/`JobFinished`. The sidebar's terminal glyph pulses while non-empty. */
  ptys: Set<string>;
  /** Open terminals, including quiet ones; unlike `ptys`, only exit removes them. Seeded for every session by `seedLive`;
   * a non-empty set keeps the row in the sidebar's Active section and shows its terminal glyph (static while quiet). */
  terminals: Set<string>;
  /** Non-PTY background jobs still running (`JobStarted` until `JobFinished`). The sidebar's background-job glyph. */
  jobs: Set<string>;
  /** Pending reminders (`ReminderSet` until `ReminderFired`/`ReminderCancelled`). The sidebar's reminder glyph. */
  reminders: Set<string>;
  /** Pending reminder details, hydrated after load/reconnect and updated by events. */
  reminderDetails: Map<string, PendingReminderView>;
  /** Activity counters restart the one-shot dot animation, even without a loaded tool row. */
  jobBeats: Record<string, number>;
  /** Artifacts by `call_id` of the `show` call that made them (35 §7.3); fed by `ArtifactShown` and `list_artifacts`. */
  artifacts: Map<string, Artifact>;
  /** Tool calls whose arguments are still streaming (`ToolCallProgress`), by `call_id`; cleared on `ToolCallStarted`.
   * `writing` = a `show` call carrying `content` (writing a file) rather than a `path` (showing one). */
  progress: Map<string, { name?: string; writing?: boolean; bytes: number }>;
  /** 199: contact files by `holder/id` as `ContactFileChanged` (and `PeerDelivered.files`) last reported them. */
  files: Map<string, ContactFile>;
  /** Live crawl/scrape URLs by `call_id`; dropped on finish, the card then parses the result. */
  scrape: Map<string, { inflight: string[]; done: string[]; limit: number }>;
  /** 79 §4.4: the latest `SandboxEvent.Progress` line of a `sandbox_*` tool call by `call_id`; dropped on finish. */
  sandbox: Map<string, { step: string; line: string; pct: number | null }>;
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
  subagentsExpanded: boolean;
  jobsExpanded: boolean;
  remindersExpanded: boolean;
  /** The whole background area is shown above the composer; the grabber and `/todos` toggle it. Default hidden. */
  todosShown: boolean;
  /** 109 §8.2: the interruption banner above the composer; cleared by the next turn that is not the resumed one. */
  resume?: ResumeBanner;
  /** 181 D4: a deliberate move — `to` on the origin (set by Move to ▸), `from` on the target (`SessionOwnerChanged.moved_from`);
   * dropped by the next ownership change that is not this move. */
  moved?: Moved;
  /** 183 §12.1 / R2: this engine's follow stream to the session's owner is up and speaks Watch (`FollowState`). Only while
   * it is up may a replicated commit be swallowed or a settled page wait on an open reply: with it down no `TurnFinished`
   * is coming. */
  following?: boolean;
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
  /** cells-midturn D2: a mid-turn `load_session` page's `context` copied onto the running turn's live replies
   * (`stampStored`); dropped when `fresh.gen` moved (a newer page landed or the context was edited meanwhile). */
  stampTail(id: string, loaded: LoadedPage, fresh: Freshness): void;
  /** Show our own input at once and skip its echo. */
  markSent(id: string, text: string, files?: string[]): void;
  /** The send failed: drop the row `markSent` showed and its echo skip. */
  unmarkSent(id: string, text: string): void;
  /** Drop the turn opened at `turn` and everything after it (before `regenerate`/`edit_turn`); no-op when absent. */
  truncate(id: string, turn: number): void;
  /** `api.compact` was refused: undo the optimistic `Compacting` unless another activity change already landed (87). */
  settleCompacting(id: string): void;
  ensure(id: string): void;
  /** Replace the subtree rows from `list_agents`. */
  setChildren(id: string, agents: AgentInfo[]): void;
  /** Open or close the live browser dialog. */
  setBrowserLive(id: string, open: boolean): void;
  /** Drop the "What's next?" chips (one was picked). */
  dismissSuggestions(id: string): void;
  /** 181: this client moved the session to `to` (`session_move` answered). */
  setMoved(id: string, to: string): void;
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
  /** Show or hide the todo block (the grabber's state); independent of `todosExpanded`. */
  setTodosShown(id: string, shown: boolean): void;
  setBackgroundExpanded(id: string, card: "subagents" | "jobs" | "reminders", expanded: boolean): void;
  setReminders(id: string, rows: PendingReminderView[]): void;
  setTerminals(id: string, ids: Iterable<string>): void;
  /** A gap may hide a `TodoChanged`: label the list stale (one session, or every session) until a snapshot confirms it. */
  markTodosStale(id?: string): void;
  resetLagged(): void;
  /** 129 D3: a gap (`Lagged`, reconnect) or a session leaving event focus may have dropped events: label the
   * transcript and plan stale (one session, or every session) so the mounted views re-settle; `loaded` stays, so
   * nothing on screen empties. */
  markStale(id?: string): void;
  /** Forget that any transcript was loaded: the chat route refetches on its next mount. */
  unload(): void;
  /** Install the `list_live_counts` snapshot: every row's session gets exactly its `jobs`/`terminals`/`reminders`, and
   * `ptys` keeps only the ids already known to be changing; every other slice with a non-empty set is emptied (the
   * snapshot is the truth). Touches nothing else. */
  seedLive(rows: LiveCountsRow[]): void;
  /** 142 F4: install `list_sessions`' `running` on every slice and subtree row (`reseedRunning`); a session the list
   * lacks keeps its marks. Touches nothing else. */
  seedRunning(rows: readonly SessionSummary[]): void;
};

export type SessionsApi = StoreApi<SessionsStore>;

/** 171F B4: parked cell results reload as their own rows ahead of the user's words (one row per input; the first row
 * opens the turn). A run of `cell_result` entries immediately followed by a turnless `user` entry folds into that
 * entry's `cells`, and the entry takes the run's turn: the card a live `TurnStarted` builds, so `adopt` pairs the two
 * and the message keeps Edit/Fork. A run followed by anything else (a review wake, a message opening its own turn)
 * stays as result cards. */
export function foldCells(entries: Entry[]): Entry[] {
  const out: Entry[] = [];
  let run: Extract<Entry, { kind: "cell_result" }>[] = [];
  for (const e of entries) {
    if (e.kind === "cell_result") {
      run.push(e);
      continue;
    }
    if (e.kind === "user" && e.turn === undefined && run.length) {
      const cells = run.map(({ id: _id, context: _context, at: _at, turn: _turn, ...c }) => c);
      out.push({ ...e, turn: run[0]?.turn, cells });
    } else out.push(...run, e);
    run = [];
  }
  return out.concat(run);
}

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
          else changed = route(draft, e, { sent, now: Date.now() }) || changed;
        }
        if (changed || dropped)
          set({
            sessions: changed ? draft : sessions,
            lagged: lagged + dropped,
          });
      },
      stampTail(id, loaded, fresh) {
        const s = get().sessions[id];
        if (!s || s.historyGeneration !== fresh.gen) return;
        const entries = stampStored(s.entries, loaded.history, s.turnSeq);
        if (entries !== s.entries) set((st) => ({ sessions: { ...st.sessions, [id]: { ...s, entries } } }));
      },
      loadHistory(id, loaded, mode, fresh) {
        const before = get().sessions[id] ?? empty(id);
        const moved =
          fresh !== undefined && before.historyGeneration !== fresh.gen;
        // The plan rides on every page and reconciles by version (47 §5.4): a refused or older page never rolls
        // it back, and only a read issued after the last invalidation clears a stale mark.
        const cur = reconcileTodos(before, loaded.todos, !moved);
        const raw = foldCells(fromHistoryPage(loaded.history));
        const refused =
          (mode === "prepend" && moved) ||
          // A newer snapshot already landed (or a reconnect will bring one): a superseded full page has nothing to add.
          (mode === "replace" && moved && cur.loaded) ||
          // 129 D3: `cur.running` is not a refusal — after a gap (`Lagged`, reconnect, out of focus) the lost
          // `TurnFinished` leaves it stale, and a settled page with `running: false` is what corrects it; a turn
          // that raced the read moved `live`, a turn still running answers `settled: false`.
          (mode === "settled" &&
            (!fresh ||
              moved ||
              cur.live !== fresh.live ||
              !loaded.settled ||
              loaded.running ||
              // 183 §5.1: a replica answers `settled` while its owner's turn is open (`resume.here: false`); the
              // followed stream is drawing a reply that page does not hold yet, so it waits for `TurnFinished`.
              // R2: only while the follow stream is up; once it dropped, nothing will close the tail.
              (loaded.resume?.here === false && cur.following === true && openTail(cur)) ||
              // 183 §5.6: nor does a lagging replica's page replace a reply the stream drew in full with less of it.
              (cur.following === true && shorterTail(cur.entries, raw))));
        if (refused) {
          if (cur !== before)
            set((st) => ({ sessions: { ...st.sessions, [id]: cur } }));
          return false;
        }
        // 146 D7: an ACP child that is not running cannot still be inside its last tool call.
        const entries = loaded.agent && !loaded.running ? interruptDangling(raw) : raw;
        const opened = lastOpenedBy(loaded.history);
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
            // A page may end inside a parked run whose user row opens the page already shown.
            entries: foldCells([...entries, ...cur.entries]),
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
            // 186: a page holding the running turn's opener names its full contact; one that starts later keeps ours.
            turnBy: !loaded.running ? undefined : opened === undefined ? cur.turnBy : (opened ?? undefined),
            waiting: false,
            activity: loaded.running ? "thinking" : "idle",
            model: loaded.model,
            effort: loaded.effort,
            queued: loaded.queued,
            undone: loaded.undone,
            agent: loaded.agent,
            kind: loaded.kind,
            observed: loaded.observed,
            harnessModel: loaded.harness_model ?? cur.harnessModel,
            harnessCommands: loaded.harness_commands ?? cur.harnessCommands,
            contextSize: loaded.context_size ?? cur.contextSize,
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
            // 109 §8.2: an open turn's banner; a live `TurnResumed`/`ResumeReport` already seen for it outranks the row.
            resume: resumeFromLoaded(cur.resume, loaded.resume),
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
            at: Date.now(),
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
      settleCompacting(id) {
        const cur = get().sessions[id];
        if (!cur) return;
        const next = settleCompacting(cur);
        if (next !== cur)
          set((st) => ({ sessions: { ...st.sessions, [id]: next } }));
      },
      ensure(id) {
        if (!get().sessions[id])
          set((st) => ({ sessions: { ...st.sessions, [id]: empty(id) } }));
      },
      setChildren(id, agents) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          // 125 §8.5: an advisor child is never a tree row, a strip dot or a live session; the engine hides it, this is the belt.
          // A row with no model yet takes the child's own `harnessModel` (a `HarnessModelChanged` newer than this snapshot).
          const children = Object.fromEntries(
            agents
              .filter((a) => a.kind !== "advisor")
              .map((a) => {
                const known = !a.model && st.sessions[a.id]?.harnessModel;
                return [a.id, known ? { ...a, model: known } : a];
              }),
          );
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
      dismissSuggestions(id) {
        set((st) => {
          const cur = st.sessions[id];
          if (!cur?.suggestions) return st;
          return { sessions: { ...st.sessions, [id]: { ...cur, suggestions: undefined } } };
        });
      },
      setMoved(id, to) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return { sessions: { ...st.sessions, [id]: { ...cur, moved: { to, at: Date.now() } } } };
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
      setTodosShown(id, shown) {
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return cur.todosShown === shown
            ? st
            : {
                sessions: {
                  ...st.sessions,
                  [id]: { ...cur, todosShown: shown },
                },
              };
        });
      },
      setBackgroundExpanded(id, card, expanded) {
        const key = `${card}Expanded` as const;
        set((st) => {
          const cur = st.sessions[id] ?? empty(id);
          return cur[key] === expanded ? st : { sessions: { ...st.sessions, [id]: { ...cur, [key]: expanded } } };
        });
      },
      setReminders(id, rows) {
        set((st) => ({
          sessions: {
            ...st.sessions,
            [id]: {
              ...(st.sessions[id] ?? empty(id)),
              reminders: new Set(rows.map((r) => r.id)),
              reminderDetails: new Map(rows.map((r) => [r.id, r])),
            },
          },
        }));
      },
      setTerminals(id, ids) {
        set((st) => ({
          sessions: {
            ...st.sessions,
            [id]: { ...(st.sessions[id] ?? empty(id)), terminals: new Set(ids) },
          },
        }));
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
      markStale(id) {
        // 142 F4: a gap over every session (reconnect, `Lagged`) also lost the `TurnFinished`s; drop the running
        // marks until `seedRunning` installs the engine's answer. One session leaving event focus keeps its events.
        const now = new Date().toISOString();
        set((st) => ({
          sessions: Object.fromEntries(
            Object.entries(st.sessions).map(([k, s]) => {
              const dropped = id === undefined ? reseedRunning(s, () => false, now) : s;
              return [
                k,
                (id === undefined || k === id) && s.loaded
                  ? {
                      ...dropped,
                      contextStale: true,
                      todosStale: true,
                      historyGeneration: s.historyGeneration + 1,
                    }
                  : dropped,
              ];
            }),
          ),
        }));
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
      seedRunning(rows) {
        const running = new Map(rows.map((r) => [r.id, r.running]));
        const now = new Date().toISOString();
        set((st) => {
          let changed = false;
          const sessions = { ...st.sessions };
          for (const [id, s] of Object.entries(sessions)) {
            const next = reseedRunning(s, (k) => running.get(k), now);
            if (next === s) continue;
            sessions[id] = next;
            changed = true;
          }
          return changed ? { sessions } : st;
        });
      },
      seedLive(rows) {
        set((st) => {
          const sessions = { ...st.sessions };
          const seen = new Set<string>();
          for (const r of rows) {
            seen.add(r.session);
            const prev = sessions[r.session] ?? empty(r.session);
            sessions[r.session] = {
              ...prev,
              jobs: new Set(r.jobs),
              // The snapshot lists every running PTY, quiet or not: all are open terminals. Only one this client already
              // saw change stays "changing"; the rest turn live again on their next `JobScreen`.
              terminals: new Set(r.ptys),
              ptys: new Set(r.ptys.filter((id) => prev.ptys.has(id))),
              reminders: new Set(r.reminders),
              reminderDetails: new Map([...(sessions[r.session]?.reminderDetails ?? [])].filter(([id]) => r.reminders.includes(id))),
              jobBeats: Object.fromEntries(Object.entries(sessions[r.session]?.jobBeats ?? {}).filter(([id]) => r.jobs.includes(id) || r.ptys.includes(id))),
            };
          }
          for (const [id, s] of Object.entries(sessions)) {
            if (seen.has(id) || !(s.jobs.size || s.ptys.size || s.terminals.size || s.reminders.size)) continue;
            sessions[id] = { ...s, jobs: new Set(), ptys: new Set(), terminals: new Set(), reminders: new Set(), reminderDetails: new Map(), jobBeats: {} };
          }
          return { sessions };
        });
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

/** A `browser_*` call is in flight: a tool card without its result during a running turn (a stale card after a crash does not count). */
export const browserBusy = (s: SessionSlice | undefined): boolean =>
  !!s?.running &&
  s.entries.some(
    (t) => t.kind === "tool" && t.name.startsWith("browser_") && !t.result,
  );

/** `browserBusy` of one session; the header's globe spins on it. */
export const useBrowserBusy = (id: string) =>
  useSessions((s) => browserBusy(s.sessions[id]));

type TaggedKey = keyof TaggedTopicPayloads;
type Listener<K extends TaggedKey, T extends TaggedTopicPayloads[K]['type']> = (event: Extract<TaggedTopicPayloads[K], {type: T}>) => void;
const listeners = new Map<TopicKey, Map<string, Set<(event: unknown) => void>>>();
const topicListeners = new Map<TopicKey, Set<(event: unknown) => void>>();

/** Topic identity is checked before the owner's inner variant. */
export function on<K extends TaggedKey, T extends TaggedTopicPayloads[K]['type']>(topic: K, type: T, cb: Listener<K, T>): () => void {
  let variants = listeners.get(topic);
  if (!variants) listeners.set(topic, variants = new Map());
  let callbacks = variants.get(type);
  if (!callbacks) variants.set(type, callbacks = new Set());
  const listener = (event: unknown) => cb(event as Extract<TaggedTopicPayloads[K], {type: T}>);
  callbacks.add(listener);
  return () => { callbacks.delete(listener); };
}

export function onTopic<K extends TopicKey>(topic: K, cb: (event: TopicPayloads[K]) => void): () => void {
  let callbacks = topicListeners.get(topic);
  if (!callbacks) topicListeners.set(topic, callbacks = new Set());
  const listener = (event: unknown) => cb(event as TopicPayloads[K]);
  callbacks.add(listener);
  return () => { callbacks.delete(listener); };
}

/** Keep the envelope until the owning reducer/listener has been selected. */
export function dispatch(events: TopicEvent[], st: Stores = active()): void {
  const core: WireEvent[] = [];
  for (const item of events) {
    switch (item.key) {
      case 'agent.events@1': case 'agent.peer-delivered@1': case 'tools.jobs@1':
      case 'tools.reminders@1': case 'browser.events@1': case 'codeindex.events@1':
      case 'artifact.shown@1': case 'app.calls@1': case 'web.scrape-progress@1':
      case 'sandbox.events@1': case 'sessionsync.follow-state@1': case 'rpc.stream@1':
        core.push(item.event);
    }
  }
  st.sessions.getState().applyEvents(core);
  const approvals = st.approvals.getState();
  const asks = st.asks.getState();
  const secrets = st.secretRequests.getState();
  const live = st === active();
  for (const item of events) {
    const e = item.event;
    switch (item.key) {
      case 'rpc.stream@1':
        if (item.event.type === 'PendingPrompts') {
          approvals.replace(item.event.approvals);
          asks.replace(item.event.asks);
          secrets.replace(item.event.secrets.filter((s): s is SecretRequested => s.type === 'SecretRequested'));
        }
        break;
      case 'tools.approval-requested@1': approvals.add(item.event); break;
      case 'tools.approval-resolved@1': approvals.resolve(item.event.id); break;
      case 'tools.ask-requested@1': asks.add(item.event); break;
      case 'tools.ask-resolved@1': asks.resolve(item.event.id); break;
      // 209 §4.10: into this engine's store, live or not, like the prompts.
      case 'browser.element-form-requested@1': st.elementForms.getState().add(item.event); break;
      case 'browser.element-form-resolved@1': st.elementForms.getState().resolve(item.event.id, item.event.outcome); break;

      case 'secrets.events@1':
        if (item.event.type === 'SecretRequested') secrets.add(item.event);
        else if (item.event.type === 'SecretResolved') secrets.resolve(item.event.id);
        break;
      case 'agent.events@1': {
        const event = item.event;
        const conn = st.connection.getState();
        if (conn.info) {
          if (event.type === 'DefaultModelChanged') conn.setInfo({...conn.info, default_model: event.model});
          else if (event.type === 'DefaultEffortChanged') conn.setInfo({...conn.info, default_effort: event.effort});
          else if (event.type === 'FavoriteModelsChanged') conn.setInfo({...conn.info, favorite_models: event.models});
        }
        break;
      }
    }
    if (!live) continue;
    if (item.key === 'sessionsync.follow-state@1') {
      if (item.event.live) useRemoteStart.getState().live(item.event.session);
    } else if ('session' in e && typeof e.session === 'string') useRemoteStart.getState().forget(e.session);
    topicListeners.get(item.key)?.forEach(cb => cb(e));
    if ('type' in e && typeof e.type === 'string') listeners.get(item.key)?.get(e.type)?.forEach(cb => cb(e));
  }
}

