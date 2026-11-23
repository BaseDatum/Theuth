import { receiveComponent } from '@/rpc/bindings/receive'
import { jsonrepair } from "jsonrepair";
import { resultLine } from "@/lib/describe";
import { codeEdited, codeToolRan, emptyCodemap } from "@/lib/codemap/slice";
import { classify, type CellResultInput, type ContactNote, type FileNote, HARNESS_NOTES, humanSecs, type Injected, movedFromRecord, splitLeading, splitParentView } from "@/lib/injected";
import { pushChunk } from "@/lib/jobTail";
import { splitRestart, splitTime } from "@/lib/time";
import { live, observedState } from "@/lib/session-tree";
import type { Advisory as AdvisoryRow } from "@/rpc/bindings/method.agent.output"
import type { AdvisoryNote } from "@/rpc/bindings/method.agent.output"
import type { ChannelOrigin } from "@/rpc/bindings/method.agent.output"
import type { ContextEntry } from "@/rpc/bindings/method.agent.output"
import type { ContactFile, PeerOrigin } from "@/rpc/bindings/method.agent.output"
import type { TodoSnapshot } from "@/rpc/bindings/method.agent.output"
import type { UiEvent as WireEvent } from '@/store/ui-events'
import type { AgentInfo } from '@/store/ui-events'
import type { AppCallChanged } from "@/rpc/bindings/event.app_2ecalls.output"
import type { HistoryEntry, Loaded as LoadedPage } from "@/rpc/bindings/method.agent.output"
import type { AppDraft, Entry, JobTail, ResumeBanner, SessionSlice } from "./sessions";

/** `sent`: texts we submitted (echo skipped); `now`: the batch's client clock (91: the thinking-time fallback). */
export type Ctx = { sent: Set<string>; now: number };

let ids = 0;

/** A fresh, process-unique entry id. */
export function nextId(): string {
  return `e${++ids}`;
}

/** 199: a contact file's key, `holder/id` (D11: every file-specific read names both). */
export const fileKey = (holder: string, id: string): string => `${holder}/${id}`;

/** 199: a `PeerDelivered.files` record as the placeholder note its stored row carries (`files::placeholders`), so the
 * live card and the reloaded one start from the same note. */
export const placeholderOf = (from: string, f: ContactFile): FileNote => ({
  from,
  message: f.exchange,
  name: f.name,
  type: f.media_type,
  state: "receiving",
  holder: f.holder,
  id: f.id,
  ...(f.size == null ? {} : { bytes: f.size }),
});

/** A session we know nothing about yet. */
export function empty(id: string): SessionSlice {
  return {
    id,
    entries: [],
    running: false,
    waiting: false,
    activity: "idle",
    harvesting: false,
    queued: [],
    skills: [],
    harnessCommands: [],
    undone: 0,
    loaded: false,
    more: false,
    firstSeq: null,
    children: {},
    browser: false,
    browserLive: false,
    terminal: false,
    ptys: new Set(),
    terminals: new Set(),
    jobs: new Set(),
    reminders: new Set(),
    reminderDetails: new Map(),
    jobBeats: {},
    artifacts: new Map(),
    progress: new Map(),
    files: new Map(),
    scrape: new Map(),
    sandbox: new Map(),
    appDrafts: new Map(),
    appCalls: new Map(),
    artifactPanel: null,
    codemap: emptyCodemap(),
    codemapPanel: null,
    contextEstimated: false,
    contextEditable: false,
    contextStale: false,
    historyGeneration: 0,
    live: 0,
    todos: null,
    todosStale: false,
    todosExpanded: false,
    subagentsExpanded: false,
    jobsExpanded: false,
    remindersExpanded: false,
    todosShown: false,
  };
}

/**
 * `snapshot` folded into the slice's plan (47 §5.4): a higher version replaces the list, an equal one is
 * idempotent, an older one is ignored. `null` (unavailable: a failed or absent read) never clears a known
 * list. `authoritative` (a live event, a read issued after the last invalidation) also repairs `todosStale`;
 * a read that predates it may carry a newer list, but not proof that nothing newer exists.
 */
export function reconcileTodos(
  s: SessionSlice,
  snapshot: TodoSnapshot | null,
  authoritative: boolean,
): SessionSlice {
  if (!snapshot) return s;
  const cur = s.todos;
  if (cur && snapshot.version < cur.version) return s;
  if (cur && snapshot.version === cur.version)
    return authoritative && s.todosStale ? { ...s, todosStale: false } : s;
  return {
    ...s,
    todos: snapshot,
    todosStale: authoritative ? false : s.todosStale,
  };
}

/**
 * The optimistic `Compacting` (87 COMPACT-NONBLOCK) when `api.compact` was refused: back to idle only if the
 * session is still `compacting`; a `Compacted`, `TurnStarted`, `Error`… that landed meanwhile wins. Same
 * resting state as `Compacted`: `thinking` while a turn runs (`turn in flight`), its next delta relabels it.
 */
export function settleCompacting(s: SessionSlice): SessionSlice {
  return s.activity === "compacting"
    ? { ...s, activity: s.running ? "thinking" : "idle" }
    : s;
}

/** What a streaming `show` call is doing, read off its partial JSON arguments: `name` (else the base name of
 * `path`) and whether a `content` key is present (writing a file vs showing an existing one). `undefined` until it parses. */
export function partialShow(
  args: string,
): { name?: string; writing: boolean } | undefined {
  try {
    const v: unknown = JSON.parse(jsonrepair(args));
    if (typeof v !== "object" || v === null) return undefined;
    const { name, path, content } = v as Record<string, unknown>;
    const file =
      typeof name === "string" && name
        ? name
        : typeof path === "string" && path
          ? path.slice(path.lastIndexOf("/") + 1) || undefined
          : undefined;
    return { name: file, writing: content !== undefined };
  } catch {}
  return undefined;
}

/** The `app` tool's arguments read off a partial (or complete) JSON envelope: only the envelope is repaired, never the
 * source inside it (46 §8.1). `undefined` until the envelope parses to an object; a field the chunk has not reached
 * yet is absent, so the caller keeps what an earlier chunk carried. */
export function partialApp(
  args: unknown,
):
  | Partial<Pick<AppDraft, "name" | "title" | "source" | "edit" | "base">>
  | undefined {
  let v: unknown = args;
  if (typeof args === "string") {
    try {
      v = JSON.parse(jsonrepair(args));
    } catch {
      return undefined;
    }
  }
  if (typeof v !== "object" || v === null) return undefined;
  const { name, title, source, edit, base_call_id } = v as Record<
    string,
    unknown
  >;
  return {
    ...(typeof name === "string" && name ? { name } : {}),
    ...(typeof title === "string" && title ? { title } : {}),
    ...(typeof source === "string" ? { source } : {}),
    ...(typeof edit === "boolean" ? { edit } : {}),
    ...(typeof base_call_id === "string" && base_call_id
      ? { base: base_call_id }
      : {}),
  };
}

/** The newest tool card for `callId`. */
function toolOf(s: SessionSlice, callId: string): Tool | undefined {
  return s.entries.findLast(
    (t): t is Tool => t.kind === "tool" && t.callId === callId,
  );
}

/** `s` with the draft for `callId` replaced by `f(previous)`; `undefined` drops it. Unchanged when `f` returns the same draft. */
function patchDraft(
  s: SessionSlice,
  callId: string,
  f: (d: AppDraft | undefined) => AppDraft | undefined,
): SessionSlice {
  const prev = s.appDrafts.get(callId);
  const next = f(prev);
  if (next === prev) return s;
  const appDrafts = new Map(s.appDrafts);
  if (next === undefined) appDrafts.delete(callId);
  else appDrafts.set(callId, next);
  return { ...s, appDrafts };
}

/** How many app records keep their pressed-invocation statuses per session. */
const APP_CALL_RECORDS = 32;

/** `s` with `e` as the newest status of its statement; the oldest record's statuses go once the bound is reached. */
export function recordCall(s: SessionSlice, e: AppCallChanged): SessionSlice {
  const appCalls = new Map(s.appCalls);
  const record = new Map(appCalls.get(e.record_call_id));
  record.set(e.statement_id, e);
  appCalls.delete(e.record_call_id);
  appCalls.set(e.record_call_id, record);
  while (appCalls.size > APP_CALL_RECORDS)
    appCalls.delete(appCalls.keys().next().value as string);
  return { ...s, appCalls };
}

/** `map` without `key`; `map` itself when absent. */
function without<V>(map: Map<string, V>, key: string): Map<string, V> {
  if (!map.has(key)) return map;
  const next = new Map(map);
  next.delete(key);
  return next;
}

function replaceAt(
  entries: readonly Entry[],
  at: number,
  entry: Entry,
): Entry[] {
  const out = entries.slice();
  out[at] = entry;
  return out;
}

/**
 * Add an entry that did not come from the model's stream (a peer message, a
 * spawn notice, a divider). It goes above a still-streaming tail so the text
 * keeps accumulating in one entry instead of splitting a markdown block.
 */
function interject(s: SessionSlice, entry: Entry): SessionSlice {
  const last = s.entries[s.entries.length - 1];
  const streaming =
    (last?.kind === "assistant" && last.open) ||
    (last?.kind === "reasoning" && !last.done);
  const at = s.entries.length - (streaming ? 1 : 0);
  return { ...s, entries: s.entries.toSpliced(at, 0, entry) };
}

/** `09:00` local of an RFC 3339 instant; the raw attribute when it does not parse. */
function hhmm(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? iso
    : `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** Transcript entries for one user input: the skill chip plus words, one card per engine wrapper, or `you`. `turn` stamps
 * the `you` row (29 §5.1); every card of one stored row shares its `context` (48 §3.2); `channel` (172 D3) rides on the `you` row. */
function userEntries(
  text: string,
  cls: Injected[] | null,
  injected?: true,
  turn?: number,
  context?: ContextEntry,
  channel?: ChannelOrigin | null,
): Entry[] {
  const flag = { ...(injected ? { injected } : {}), ...(channel ? { channel } : {}) };
  if (cls === null)
    return [{ id: nextId(), kind: "user", text, turn, context, ...flag }];
  // 125 §8.3: every `<advisory>` element of one row is one card; its seq is the row's when the row is stored.
  const notes = cls.filter((c): c is Extract<Injected, { kind: "advisory" }> => c.kind === "advisory");
  const card: Entry[] = notes.length
    ? [{ id: nextId(), kind: "advisory", notes: notes.map((n) => ({ severity: n.severity, note: n.text })), seq: seqOf(context), context }]
    : [];
  return cls.filter((c): c is Exclude<Injected, { kind: "advisory" }> => c.kind !== "advisory").map((c): Entry => {
    switch (c.kind) {
      case "skill":
        return {
          id: nextId(),
          kind: "user",
          text: c.words,
          skill: c.name,
          turn,
          context,
          ...flag,
        };
      case "peer":
        return {
          id: nextId(),
          kind: "peer",
          from: c.from,
          text: c.text,
          context,
        };
      case "subagent":
        return {
          id: nextId(),
          kind: "peer",
          from: c.name,
          text: c.text,
          context,
        };
      case "job":
        return {
          id: nextId(),
          kind: "peer",
          from: `${c.id} · ${c.command} · exit ${c.exit} · ${c.secs}s${c.pty ? " · pty" : ""}`,
          text: c.text,
          context,
        };
      // 61 §2.3: `j4 idle 63s · omp1`, the screen as the body; 155 §6.5: `· <why>` when the engine says why it fired.
      case "job_idle":
        return {
          id: nextId(),
          kind: "peer",
          from: `${c.id} idle ${c.quiet}s${c.name === undefined ? "" : ` · ${c.name}`}${c.why === undefined ? "" : ` · ${c.why}`}`,
          text: c.text,
          context,
        };
      case "schedule":
        return {
          id: nextId(),
          kind: "peer",
          from: `${c.name} · ${hhmm(c.at)}`,
          text: c.text,
          context,
        };
      // 14 as-built: `Reminder r3 · after 10 min` (MessagePeer draws the timer icon), the model's message as the body.
      case "reminder":
        return {
          id: nextId(),
          kind: "peer",
          from: `Reminder ${c.id} · after ${humanSecs(c.after)}${c.late ? " · late" : ""}`,
          text: c.text,
          context,
        };
      // 199: a file notice row (or an ended share's) is a file card.
      case "file":
      case "engine":
      case "desk":
      case "wake":
      case "resume":
      case "lost":
      case "child":
      case "autoresearch":
      case "moved":
      case "engine_restart":
      case "contact_inbox":
      case "contact_message":
        return { id: nextId(), ...c, context };
      // 171F B4: `turn` when this row opened the turn, so `foldCells` can hand it to the user's words that follow.
      case "cell_result":
        return { id: nextId(), ...c, turn, context };
      // 137G G10: the endless view was shrunk to fit — a muted info line, not a user bubble or a nudge card.
      case "memory":
        return { id: nextId(), kind: "info", text: c.text, context };
      // 201 R3-2: a `<ledger>` / `<recall>` / `<tools>` note — the same muted line, collapsed under its label.
      case "note":
        return { id: nextId(), kind: "info", text: c.text, label: HARNESS_NOTES[c.note], context };
    }
  }).concat(card);
}

/** 152 D3: a user text split into the parent's view card (an endless parent's handoff to a native child, 137 R1-D1,
 * reload only) and the rest as every user text is read today: `splitTime`, then the classifier. */
function userText(raw: string): { view?: Entry; text: string; at?: number } {
  const pv = splitParentView(raw);
  const { text, at } = splitTime(pv ? pv.rest.trimStart() : raw);
  return pv ? { view: { id: nextId(), kind: "parentView", lines: pv.lines, bytes: pv.bytes }, text, at } : { text, at };
}

/** The stored row a context entry points at, when it is a message. */
const seqOf = (context: ContextEntry | undefined): number | undefined =>
  context?.target.kind === "message" ? context.target.seq : undefined;

/** The notes of a card as one comparable string. */
const notesKey = (notes: readonly AdvisoryNote[]): string => notes.map((n) => `${n.severity}\n${n.note}`).join("\0");

type Advisory = Extract<Entry, { kind: "advisory" }>;

/** R1.7: the consultation's question (the flattened `kind` tag), `undefined` for a review. */
const questionOf = (a: AdvisoryRow): string | undefined => (a.kind === "consultation" ? a.question : undefined);

/**
 * 125 §8.3: show one Advisory card once. `AdvisoryAdded`, the `<advisory>` wrapper of a `TurnStarted`/`InputInjected`
 * and the settled reload all describe the same stored row: a card with the same `seq` is already there; a card parsed
 * from the wrapper text has no `seq` yet, so it is matched by its notes and stamped. New cards go above a streaming tail.
 */
function addAdvisory(s: SessionSlice, entry: Advisory): SessionSlice {
  const key = notesKey(entry.notes);
  const at = s.entries.findLastIndex(
    (x): x is Advisory =>
      x.kind === "advisory" &&
      (x.seq !== undefined && entry.seq !== undefined ? x.seq === entry.seq : notesKey(x.notes) === key),
  );
  if (at < 0) return interject({ ...s, live: s.live + 1 }, entry);
  const cur = s.entries[at] as Advisory;
  return cur.seq === undefined && entry.seq !== undefined
    ? { ...s, entries: replaceAt(s.entries, at, { ...cur, seq: entry.seq }) }
    : s;
}

/** The newest `you` row stamped with the turn it opened: an own send or an injected steer whose `TurnStarted` echo was
 * skipped. It carries the turn's parked cell results when there are any (171F B4). */
function stampUser(entries: Entry[], turn: number, cells: CellResultInput[] = []): Entry[] {
  const at = entries.findLastIndex((u) => u.kind === "user");
  const u = entries[at];
  return u?.kind === "user" ? replaceAt(entries, at, { ...u, turn, ...(cells.length ? { cells } : {}) }) : entries;
}

/** The body of a stored peer row (50 §9 stores the provider-visible form): the words inside the one `<message>`
 * wrapper, the one `<contact_message>` (183 §11.6) or an inbox's `<contact_inbox/>` notice plus its `<message>`
 * (§11.5); the raw text otherwise. A contact wrapper also gives the contact facts and the harness note the model read
 * (Details lists them, never the tag). 199: the `<message_file …/>` placeholders after the wrapper (and their hint
 * line) come back as `files`. Display only; identity comes from the typed `PeerOrigin`. */
function peerBody(text: string): { text: string; contact?: ContactNote & { note: string }; files?: FileNote[] } {
  const cls = classify(text) ?? []
  const notes = cls.flatMap((c) => (c.kind === 'file' ? [c.file] : []))
  const files = notes.length ? { files: notes } : {}
  const [head, next, more] = cls.filter((c) => c.kind !== 'file')
  if (more) return { text, ...files }
  if (head?.kind === 'peer' && !next) return { text: head.text, ...files }
  // The contact's words: the lone `<contact_message>`'s, or the `<message>` after an inbox's notice.
  const words = head?.kind === 'contact_message' && !next ? head.text : head?.kind === 'contact_inbox' && !head.message && next?.kind === 'peer' ? next.text : undefined
  if (words === undefined || (head?.kind !== 'contact_message' && head?.kind !== 'contact_inbox')) return { text, ...files }
  return { text: words, contact: { contact: head.contact, machine: head.machine, export: head.export, cap: head.cap, note: head.note }, ...files }
}

/** The card a peer exchange renders as, from its authenticated provenance; never from the wrapper's attributes. */
function peerEntry(origin: PeerOrigin, text: string, seq?: number, context?: ContextEntry): Extract<Entry, { kind: 'peer' }> {
  return { id: nextId(), kind: 'peer', from: origin.from, text, origin, seq, context }
}

/** Transcript entries for one history record; `context` (48 §3.3) rides along on every card the record becomes, and
 * the stored row time becomes `at` (74 §5). */
export function fromHistory(h: HistoryEntry): Entry[] {
  const out = historyEntries(h);
  const at = h.context?.at ? Date.parse(h.context.at) : NaN;
  return Number.isNaN(at) ? out : out.map((e) => ({ ...e, at }));
}

/** 186: the `peer:` address of a full-collaboration contact when `h` is the row that opened its turn, `null` when `h`
 * opened any other turn, `undefined` when it opens none (a steer, a reply, a tool row). Typed fields only. */
const openedBy = (h: HistoryEntry): string | null | undefined =>
  h.role !== 'user' || h.turn === null ? undefined : h.peer?.contact_mode === 'full' ? h.peer.from : null

/** A page of history as entries; 186: each tool call of a turn a full contact's message opened carries `by`. A page that
 * starts mid-turn leaves those first calls unstamped (no tag rather than a wrong one). */
export function fromHistoryPage(history: readonly HistoryEntry[]): Entry[] {
  let by: string | null = null
  return history.flatMap((h) => {
    const opened = openedBy(h)
    if (opened !== undefined) by = opened
    const out = fromHistory(h)
    return by ? out.map((e) => (e.kind === 'tool' ? { ...e, by: by as string } : e)) : out
  })
}

/** 186: the full contact whose message opened a loaded page's last turn; `null` when another input opened it,
 * `undefined` when the page holds no opener. */
export function lastOpenedBy(history: readonly HistoryEntry[]): string | null | undefined {
  for (let i = history.length - 1; i >= 0; i--) {
    const opened = openedBy(history[i] as HistoryEntry)
    if (opened !== undefined) return opened
  }
  return undefined
}

/** 146 D7: a session loaded with `running === false` whose last entry is a result-less tool call — an ACP child
 * killed mid-call by an engine restart — shows that card interrupted instead of spinning forever. */
export function interruptDangling(entries: Entry[]): Entry[] {
  const at = entries.length - 1;
  const last = entries[at];
  return last?.kind === "tool" && !last.result ? replaceAt(entries, at, { ...last, interrupted: true }) : entries;
}

function historyEntries(h: HistoryEntry): Entry[] {
  const context = h.context ?? undefined;
  switch (h.role) {
    case "user": {
      // 74 §3: the engine's `<time …/>` line is for the model; the row's `at` — or the stamp's instant for a row
      // stored before `messages.at` existed — is what the human sees.
      const { view, text, at } = userText(h.text);
      // 52 §6: the typed metadata, never the `<vision_observation>` prose, makes a row a consultation note.
      const vision = h.vision ?? context?.vision;
      if (vision)
        return [
          {
            id: nextId(),
            kind: "vision",
            model: vision.vision_model,
            sources: vision.observations.map((o) => o.source),
            note: vision,
            text,
            seq: context?.target.kind === "message" ? context.target.seq : undefined,
            context,
          },
        ];
      if (h.peer) {
        const { text: words, contact, files } = peerBody(text)
        return [{ ...peerEntry(h.peer, words, undefined, context), ...(contact ? { contact } : {}), ...(files ? { files } : {}) }];
      }
      const rows = userEntries(
        text,
        classify(text),
        undefined,
        h.turn ?? undefined,
        context,
        h.channel,
      );
      return (view ? [{ ...view, context }, ...rows] : rows).map((e) => (at === undefined ? e : { ...e, at }));
    }
    case "assistant":
      return h.text.trim()
        ? [
            {
              id: nextId(),
              kind: "assistant",
              text: h.text,
              open: false,
              turn: h.turn ?? undefined,
              context,
            },
          ]
        : [];
    case "reasoning":
      return [{ id: nextId(), kind: "reasoning", text: h.text, done: true, ms: h.thinking_ms ?? undefined }];
    case "tool": {
      // 183 §12.4: the restart note (and the `<time/>` stamp) leave the output; the note becomes its own card.
      const out = h.result ? splitRestart(h.result[0]) : undefined;
      return [
        {
          id: nextId(),
          kind: "tool",
          callId: h.call_id,
          name: h.name,
          args: h.args,
          pending: false,
          child: h.call_id.includes("."),
          context,
          ...(h.result && out
            ? {
                result: {
                  output: out.text,
                  isError: h.result[1],
                  files: h.files ?? [],
                },
                ...(out.restart ? { restart: out.restart } : {}),
              }
            : {}),
        },
      ];
    }
    case "compacted":
      return [
        {
          id: nextId(),
          kind: "compacted",
          epoch: h.epoch,
          style: h.style,
          summary: h.summary,
          context,
        },
      ];
    // Never model content: shown as it was live, nothing to remove from context.
    case "error":
      return [{ id: nextId(), kind: "error", text: h.text, context }];
    // 109 §8.2: the open turn's streamed partial; greyed until the regenerated reply replaces it.
    case "interrupted":
      return [{ id: nextId(), kind: "interrupted", text: h.text }];
    // 125 §8.3: the typed card; `seq` is the stored row (the `AdvisoryAdded` idempotence key).
    case "advisory":
      return [{ id: nextId(), kind: "advisory", notes: h.advisory.notes, seq: seqOf(context), context, at: h.advisory.at_ms, question: questionOf(h.advisory) }];
    // 183 §12.4: the typed record of a move or take; the card renders its fields, never the `<session_moved/>` text.
    case "moved":
      return [{ id: nextId(), ...movedFromRecord(h.moved), context }];
  }
}

/** 109 §8.2: the banner's counts from a `ResumeReport` — the live event or the persisted row in `Loaded.resume`. */
function reportCounts(r: { replayed: unknown[]; interrupted: [string, string][]; lost_jobs: unknown[] }): NonNullable<ResumeBanner["report"]> {
  return { replayed: r.replayed.length, skipped: r.interrupted.map(([, name]) => name), lostJobs: r.lost_jobs.length };
}

/** 109 §8.2: the banner from `Loaded.resume`. No row while we show a turn still "resuming…" → the turn is no longer
 * open, so it ran: flip `resumed` (the live `TurnResumed` was published before this client subscribed). Same turn →
 * keep what we have, take the row's `resumed` if newer and its persisted report when we hold none (a live report for
 * the turn wins); 139: the row's `owner`/`here` are the truth about who runs it now. Another turn → the row's state
 * and report. A pre-139 engine omits `here`: owned here, as before. */
export function resumeFromLoaded(
  cur: ResumeBanner | undefined,
  loaded: LoadedPage["resume"] | undefined,
): ResumeBanner | undefined {
  // 183 §12.1: a replica's "Running on <owner>" ends with the owner's turn (a remote chat streams every turn now).
  if (!loaded) return cur && !cur.here ? undefined : cur && !cur.resumed ? { ...cur, resumed: true } : cur;
  // 183 R2 (live): a replica's page names the owner's last resume report once its turn is over (`resumed` is the
  // owner's word only for a closed turn here); nothing runs there now, so no "Running on <owner>".
  if (loaded.here === false && loaded.resumed) return undefined;
  const report = loaded.report ? reportCounts(loaded.report) : undefined;
  const here = loaded.here ?? true;
  const owner = loaded.owner ?? undefined;
  if (cur?.turn === loaded.turn) {
    const next = cur.resumed || !loaded.resumed ? cur : { ...cur, resumed: true };
    const owned = next.here === here && next.owner === owner ? next : { ...next, here, owner };
    // 183 R1 (H3): the moved-from engine survives a reload (a live `TurnResumed.from` already set it).
    const sourced = owned.from || !loaded.from ? owned : { ...owned, from: loaded.from };
    return cur.report || !report ? sourced : { ...sourced, report };
  }
  const at = Date.parse(loaded.interrupted_at);
  return {
    turn: loaded.turn,
    kind: loaded.kind,
    at: Number.isNaN(at) ? Date.now() : at,
    resumed: loaded.resumed,
    report,
    owner,
    here,
    cause: loaded.cause ?? undefined,
    ...(loaded.from ? { from: loaded.from } : {}),
  };
}

/** 139 §9.3: who runs the session as the banner last learned it (`Loaded.resume`, `SessionOwnerChanged`). Owner
 * events reach a replica's clients relayed (`events.rs` follower), so they inherit the view instead of asserting
 * "here"; with no banner yet the engine we talk to is assumed to be the owner, as before 139. */
const whose = (r: ResumeBanner | undefined): Pick<ResumeBanner, "here" | "owner"> => ({ here: r?.here ?? true, owner: r?.owner });

/** What makes two renderings of one stored row the same card: kind plus its stable handle, not the process-local id. */
function signature(e: Entry): string {
  switch (e.kind) {
    case "tool":
      return `t${e.callId}`;
    case "compacted":
      return `c${e.epoch}`;
    case "peer":
      // 171F B13: an origin-less card's `from` differs live (`job j4`, `session:<id>`) and stored (`j4 · cmd · exit 0 · 3s`,
      // the subagent's name), so only the body pairs them: the live card keeps its id, and its open state, after the reload.
      return e.origin ? `p${e.origin.id}` : `p\n${e.text.trimEnd()}`;
    case "engine":
      return `h${e.notice}\n${e.text}`;
    case "desk":
      return `d${e.agent}\n${e.journal}`;
    case "wake":
      return `w${e.id ?? ""}\n${e.reason}\n${e.body ?? ""}`;
    case "resume":
      return `r${e.clock}\n${e.text}`;
    case "lost":
      return `l${e.session}\n${e.clock}`;
    case "child":
      return `ch${e.event}\n${e.session}\n${e.clock}\n${e.text}`;
    case "autoresearch":
      return `ar${e.slug}\n${e.event}\n${e.text}`;
    case "cell_result": // 171:U1-ADD
      return `cr${e.run}`;
    case "parentView":
      return `pv${e.lines.length}\n${e.bytes}`;
    case "vision":
      return e.seq === undefined ? `v${e.model}\n${e.text}` : `v${e.seq}`;
    case "user":
    case "assistant":
      return `${e.kind[0]}${e.turn ?? ""}\n${e.text}`;
    case "advisory":
      return e.seq === undefined ? `a${notesKey(e.notes)}` : `a${e.seq}`;
    case "file":
      return `f${e.file.holder}/${e.file.id}\n${e.file.state}`;
    default:
      return `${e.kind}\n${e.text}`;
  }
}

/**
 * `fresh` (a reloaded snapshot) wearing the ids of the cards `old` already shows for the same rows, so
 * React keys, virtualizer measurements, focus and scroll survive the replacement; a tool card also keeps
 * its live job tail. Matching is by `signature`, in order, so duplicate texts pair up one to one.
 */
export function adopt(old: readonly Entry[], fresh: Entry[]): Entry[] {
  if (!old.length || !fresh.length) return fresh;
  const seen = new Map<string, Entry[]>();
  for (const e of old) {
    const sig = signature(e);
    const q = seen.get(sig);
    if (q) q.push(e);
    else seen.set(sig, [e]);
  }
  return fresh.map((e) => {
    const prev = seen.get(signature(e))?.shift();
    if (!prev) return e;
    return e.kind === "tool" && prev.kind === "tool" && prev.job
      ? { ...e, id: prev.id, job: prev.job }
      : { ...e, id: prev.id };
  });
}

/** Deltas and in-place tool results keep a streaming tail open; new transcript entries close it. */
function closeTail(s: SessionSlice, e: WireEvent, ctx: Ctx): SessionSlice {
  // Background results patch an earlier card; they are not the model moving on.
  if (e.type === "ToolCallFinished" && s.entries.some((t) => t.kind === "tool" && t.callId === e.call_id))
    return s;
  const at = s.entries.length - 1;
  const last = s.entries[at];
  if (last?.kind === "reasoning" && !last.done && e.type !== "ReasoningDelta") {
    // 91: `ReasoningDone` carries the engine's measurement; anything else
    // closing the block (an older engine, a harness) gets the client clock.
    const ms =
      e.type === "ReasoningDone"
        ? e.ms
        : last.startedAt === undefined
          ? undefined
          : Math.max(0, ctx.now - last.startedAt);
    const { startedAt: _started, ...rest } = last;
    return { ...s, entries: replaceAt(s.entries, at, { ...rest, done: true, ms }) };
  }
  if (last?.kind === "assistant" && last.open && e.type !== "TextDelta") {
    return {
      ...s,
      entries: last.text.trim()
        ? replaceAt(s.entries, at, { ...last, open: false })
        : s.entries.slice(0, at),
    };
  }
  return s;
}

type Tool = Extract<Entry, { kind: "tool" }>;

/** Patch the newest tool card `pick` selects; the slice unchanged when none matches. */
function patchTool(
  s: SessionSlice,
  pick: (t: Tool) => boolean,
  patch: (t: Tool) => Entry,
): SessionSlice {
  for (let at = s.entries.length - 1; at >= 0; at--) {
    const t = s.entries[at];
    if (t?.kind === "tool" && pick(t))
      return { ...s, entries: replaceAt(s.entries, at, patch(t)) };
  }
  return s;
}

/** Patch the tail of job `id` on its tool card; `f` returning `undefined` drops the tail. */
function patchJob(
  s: SessionSlice,
  id: string,
  f: (job: JobTail) => JobTail | undefined,
): SessionSlice {
  return patchTool(
    s,
    (t) => t.job?.id === id,
    (t) => {
      const { job, ...rest } = t;
      const next = job && f(job);
      return next ? { ...rest, job: next } : rest;
    },
  );
}

/** Drop `id` from `key` (`ptys`: screen went quiet or exited; `jobs`: exited; `reminders`: fired or cancelled); a no-op when absent. */
function jobGone(s: SessionSlice, key: "ptys" | "jobs" | "reminders", id: string): SessionSlice {
  if (!s[key].has(id)) return s;
  const set = new Set(s[key]);
  set.delete(id);
  return { ...s, [key]: set };
}

/** A reply is streaming into the transcript's last entry (an open assistant text); a settled page cannot hold it yet. */
export function openTail(s: Pick<SessionSlice, "entries">): boolean {
  const last = s.entries.at(-1);
  return last?.kind === "assistant" && last.open === true;
}

/** The last user row's text and the reply text after it. */
function tail(entries: readonly Entry[]): { user: string | undefined; reply: string } {
  const at = entries.findLastIndex((e) => e.kind === "user");
  const last = entries[at];
  return {
    user: last?.kind === "user" ? last.text : undefined,
    reply: entries.slice(at + 1).flatMap((e) => (e.kind === "assistant" ? [e.text] : [])).join(""),
  };
}

/** 183 §5.6: `page` holds less of the current reply than the `live` entries drew (same last message, its reply a strict
 * prefix): a replica that has not caught up. Installing it would blank streamed text until the rows arrive. */
export function shorterTail(live: readonly Entry[], page: readonly Entry[]): boolean {
  const a = tail(live);
  const b = tail(page);
  return a.user !== undefined && a.user === b.user && a.reply.length > b.reply.length && a.reply.startsWith(b.reply);
}

type Reply = Extract<Entry, { kind: "assistant" }>;

/** cells-midturn D2: the closed replies of the running turn drawn live, so without their stored row's `context` yet, as
 * one key (`<turn>:<ids>`); `null` when there are none. */
export function unstampedKey(s: Pick<SessionSlice, "entries" | "turnSeq" | "running">): string | null {
  if (!s.running || s.turnSeq === undefined) return null;
  const ids = s.entries.flatMap((e) => (e.kind === "assistant" && e.turn === s.turnSeq && !e.open && !e.context ? [e.id] : []));
  return ids.length ? `${s.turnSeq}:${ids.join(" ")}` : null;
}

/** cells-midturn R1: reads that stamped nothing for `key`, the `unstampedKey` they began at. */
export type StampTries = { key: string; misses: number };
/** Reads that stamp nothing for one key before the stamp stops reading until the key changes. */
const STAMP_MISSES = 2;

/**
 * cells-midturn R1: whether a mid-turn stamp read is worth it for `key`. A reply `stampStored` refuses for good (a text
 * that differs, a salvage continuation, a page too short) keeps its key; after `STAMP_MISSES` reads for that key
 * nothing reads until the key changes — a reply closes or gets stamped, or another turn runs. The settled reload at
 * the turn's end still stamps everything.
 */
export function stampWanted(tries: StampTries | undefined, key: string | null): boolean {
  return key !== null && (tries?.key !== key || tries.misses < STAMP_MISSES);
}

/** cells-midturn R1: `tries` after a read that began at key `before` and left `after`: the same key is a miss. */
export function stampTried(tries: StampTries | undefined, before: string, after: string | null): StampTries | undefined {
  if (after !== before) return undefined;
  return { key: before, misses: (tries?.key === before ? tries.misses : 0) + 1 };
}

/**
 * cells-midturn D2: `entries` with the stored rows' `context` copied onto the live replies of `turn` that lack it, as
 * a reload mid-turn would show them (a chat cell needs its reply's stored seq, 171 §3.4). `history` is a newest
 * `load_session` page; it must reach back to the turn's last stamped reply (else its opener): the stored replies after
 * that point pair in order with the closed unstamped live replies, each pair with identical text. Any doubt — the page
 * too short, more stored replies than live ones, one text differs — leaves `entries` as they are. Only `context` is
 * added; nothing is replaced, reordered or dropped.
 */
export function stampStored(entries: Entry[], history: readonly HistoryEntry[], turn: number | undefined): Entry[] {
  if (turn === undefined) return entries;
  const mine = (e: Entry): e is Reply => e.kind === "assistant" && e.turn === turn;
  const anchorAt = entries.findLastIndex((e) => mine(e) && !!e.context);
  if (entries.some((e, i) => i < anchorAt && mine(e) && !e.context)) return entries;
  const anchor = seqOf(entries[anchorAt]?.context);
  // The anchor's reasoning row shares its seq: the reply is the last row with it.
  const from =
    anchorAt < 0
      ? history.findIndex((h) => h.role === "user" && h.turn === turn)
      : anchor === undefined
        ? -1
        : history.findLastIndex((h) => seqOf(h.context ?? undefined) === anchor);
  if (from < 0) return entries;
  const stored = history
    .slice(from + 1)
    .filter((h): h is Extract<HistoryEntry, { role: "assistant" }> => h.role === "assistant" && h.turn === turn && !!h.text.trim());
  const live = entries.flatMap((e, i) => (i > anchorAt && mine(e) && !e.context && !e.open ? [i] : []));
  if (!stored.length || stored.length > live.length) return entries;
  if (stored.some((h, k) => !h.context || h.text !== (entries[live[k] as number] as Reply).text)) return entries;
  const out = [...entries];
  stored.forEach((h, k) => {
    const at = live[k] as number;
    out[at] = { ...(out[at] as Reply), context: h.context ?? undefined };
  });
  return out;
}

/**
 * Fold one event into its session's slice (mirrors `tui::model::apply_agent`
 * and the job arm of `Model::apply`). Returns `slice` itself when nothing
 * changed; never mutates it. `ctx.sent` loses a text once its echo is skipped.
 */
export function apply(
  slice: SessionSlice,
  e: WireEvent,
  ctx: Ctx,
): SessionSlice {
  // Bookkeeping that must not finish a streaming tail.
  switch (e.type) {
    // 171F4 N7: `files` only on our own steer (the composer's attachments); the engine's echo of it changes nothing.
    case "InputQueued":
      return slice.queued.some((q) => q.id === e.id)
        ? slice
        : {
            ...slice,
            queued: [...slice.queued, { id: e.id, text: e.text, defer: e.defer, ...(e.channel ? { channel: e.channel } : {}), ...(e.files?.length ? { files: e.files } : {}) }],
          };
    case "InputWithdrawn": {
      const queued = slice.queued.filter((q) => q.id !== e.id);
      return queued.length === slice.queued.length
        ? slice
        : { ...slice, queued };
    }
    case "MessageDelivered":
      // 171F B4: a quiet cell run's result (`from: "cell <name>"`) shows in the cell itself, then with the message it joins.
      if (e.from.startsWith("cell ")) return slice;
      // Shown now, stored only once a turn injects it: a settled snapshot read meanwhile lacks the card.
      return interject(
        { ...slice, live: slice.live + 1 },
        { id: nextId(), kind: "peer", from: e.from, text: e.text },
      );
    // The consumed exchange as a typed card (50 §10). Its row is already stored, so a settled reload lands the same
    // card: one shows once, whichever arrives first (matched by exchange id, the wrapper echo on `TurnStarted` is skipped).
    // 199: `e.files` are the card's receiving placeholders (the same notes the stored row replays as) and seed the live
    // file states, never over a newer `ContactFileChanged`. An engine from before 199 sends no `files`.
    case "PeerDelivered": {
      const offered = e.files ?? [];
      const known = offered.filter((f) => !slice.files.has(fileKey(f.holder, f.id)));
      const files = known.length ? new Map([...slice.files, ...known.map((f): [string, ContactFile] => [fileKey(f.holder, f.id), f])]) : slice.files;
      if (
        slice.entries.some(
          (x) => x.kind === "peer" && x.origin?.id === e.origin.id,
        )
      )
        return files === slice.files ? slice : { ...slice, files };
      return interject(
        {
          ...slice,
          files,
          live: slice.live + 1,
          // 186: the row that opened the running turn (`TurnStarted` came first) is a full contact's message.
          ...(slice.running && e.seq === slice.turnSeq && e.origin.contact_mode === "full" ? { turnBy: e.origin.from } : {}),
        },
        { ...peerEntry(e.origin, e.text, e.seq), ...(offered.length ? { files: offered.map((f) => placeholderOf(e.origin.from, f)) } : {}) },
      );
    }
    // 199: a contact file moved (state or bytes); its cards read it from here, in either direction.
    case "ContactFileChanged":
      return { ...slice, files: new Map(slice.files).set(fileKey(e.file.holder, e.file.id), e.file) };
    // 125 §8.3: one Advisory card per stored row, whichever of the event, the wrapper echo or the reload lands first.
    case "AdvisoryAdded":
      return addAdvisory(slice, { id: nextId(), kind: "advisory", notes: e.advisory.notes, seq: e.seq, at: e.advisory.at_ms, question: questionOf(e.advisory) });
    // 125 §3: one `advisor → <model | off>` line when the advisor's model changes; state ticks (reviewing/idle) print nothing.
    case "AdvisorChanged": {
      const model = e.status.state === "off" ? null : (e.status.model ?? null);
      if ((slice.advisorModel ?? null) === model) return slice;
      return interject({ ...slice, advisorModel: model }, { id: nextId(), kind: "info", text: `advisor → ${model ?? "off"}` });
    }
    // 109 §8.2: the banner changes state at once; the dropped partial stays until the first regenerated delta.
    // 109 LIVE: a resumed turn is a running turn (no `TurnStarted` follows).
    case "TurnResumed": {
      const at = Date.parse(e.interrupted_at);
      return {
        ...slice,
        running: true,
        waiting: true,
        activity: "thinking",
        resume: {
          turn: e.turn,
          kind: e.kind,
          at: Number.isNaN(at) ? Date.now() : at,
          resumed: true,
          report: slice.resume?.turn === e.turn ? slice.resume.report : undefined,
          // 139 §9.3: a relayed `TurnResumed` from a followed owner must not flip a replica's view to "here".
          ...whose(slice.resume),
          cause: e.cause,
          from: e.from ?? undefined,
        },
      };
    }
    case "ResumeReport": {
      const base = slice.resume?.turn === e.turn ? slice.resume : { turn: e.turn, kind: "interrupted" as const, at: Date.now(), resumed: true, ...whose(slice.resume) };
      return { ...slice, resume: { ...base, report: reportCounts(e) } };
    }
    // 139 §6.1: `reason: "release"` is the owner side of a transfer (the taker resumes it — §9.3 "Moving to…").
    case "TurnSuspended":
      return {
        ...slice,
        resume: { turn: e.turn, kind: "suspended", at: Date.now(), resumed: false, ...whose(slice.resume), cause: e.reason === "release" ? "transfer" : undefined },
      };
    // 139 §6.3 (D5): a system takeover left the turn open here; the banner says so and offers Resume (§9.3).
    case "ResumePending":
      return {
        ...slice,
        resume: { turn: e.turn, kind: "interrupted", at: Date.now(), resumed: false, ...whose(slice.resume), cause: "takeover", from: e.from },
      };
    // 139 §9.3: the banner follows ownership — "Moving to B…" on the old owner's clients once B's row lands.
    // 181 D4: `moved_from` on the taker's own event says it was a move; the origin's record (`setMoved`) stands
    // while the owner is the engine it moved to; any other change ends both.
    case "SessionOwnerChanged": {
      const moved = e.moved_from && e.here ? { from: e.moved_from, at: Date.now() } : slice.moved?.to === e.owner ? slice.moved : undefined;
      const next = moved === slice.moved ? slice : { ...slice, moved };
      return next.resume ? { ...next, resume: { ...next.resume, here: e.here, owner: e.here ? undefined : e.owner } } : next;
    }
    case "PartialDropped":
      return slice;
    case "TitleChanged": {
      const tags = e.tags;
      const same =
        slice.title === e.title &&
        slice.tags?.length === tags.length &&
        tags.every((t, i) => slice.tags?.[i] === t);
      return same ? slice : { ...slice, title: e.title, tags };
    }
    case "JobStarted": {
      const next = patchTool(
        slice,
        (t) => t.callId === e.call_id,
        (t) => ({ ...t, job: { id: e.id, command: e.command, lines: [] } }),
      );
      return e.pty
        ? { ...next, terminal: true, ptys: new Set(next.ptys).add(e.id), terminals: new Set(next.terminals).add(e.id) }
        : { ...next, jobs: new Set(next.jobs).add(e.id) };
    }
    case "JobOutput": {
      const next = patchJob(slice, e.id, (j) => ({
        ...j,
        lines: pushChunk(j.lines, e.chunk),
      }));
      return slice.jobs.has(e.id) || slice.terminals.has(e.id)
        ? { ...next, jobBeats: { ...next.jobBeats, [e.id]: (next.jobBeats[e.id] ?? 0) + 1 } }
        : next;
    }
    case "JobScreen": {
      const next = patchJob(slice, e.id, (j) => ({
        ...j,
        screen: { rows: e.rows, cursor: e.cursor },
      }));
      // A changing screen is a live terminal again, however it went quiet.
      return {
        ...next,
        ptys: new Set(next.ptys).add(e.id),
        terminals: new Set(next.terminals).add(e.id),
        jobBeats: { ...next.jobBeats, [e.id]: (next.jobBeats[e.id] ?? 0) + 1 },
      };
    }
    case "JobIdle":
      return jobGone(slice, "ptys", e.id);
    case "JobFinished": {
      // Within grace the tool result carries the output: the tail has nothing to add.
      const next = patchJob(slice, e.id, (j) =>
        e.inline
          ? undefined
          : { ...j, exit: { exit: e.exit, secs: Number(e.secs) } },
      );
      const terminals = new Set(next.terminals);
      terminals.delete(e.id);
      const jobBeats = { ...next.jobBeats };
      delete jobBeats[e.id];
      return { ...jobGone(jobGone(next, "ptys", e.id), "jobs", e.id), terminals, jobBeats };
    }
    case "ReminderSet": {
      if (slice.reminderDetails.has(e.id)) return slice;
      const { id, message, set_at, fires_at } = e;
      return {
        ...slice,
        reminders: new Set(slice.reminders).add(id),
        reminderDetails: new Map(slice.reminderDetails).set(id, { id, message, set_at, fires_at }),
      };
    }
    case "ReminderFired":
    case "ReminderCancelled": {
      const next = jobGone(slice, "reminders", e.id);
      if (!slice.reminderDetails.has(e.id)) return next;
      const reminderDetails = new Map(slice.reminderDetails);
      reminderDetails.delete(e.id);
      return { ...next, reminderDetails };
    }
    case "BrowserShowLive":
      return { ...slice, browser: true, browserLive: true };
    case "CodeToolRan":
      return { ...slice, codemap: codeToolRan(slice.codemap, e) };
    case "ToolCallProgress": {
      const next = {
        ...slice,
        progress: new Map(slice.progress).set(e.call_id, {
          ...partialShow(e.args),
          bytes: e.args.length,
        }),
      };
      if (toolOf(slice, e.call_id)?.name !== "app") return next;
      // A chunk whose envelope does not repair yet keeps the previous source; only the byte count moves.
      return patchDraft(next, e.call_id, (d) => ({
        source: "",
        edit: false,
        ...d,
        ...partialApp(e.args),
        bytes: e.args.length,
        status: "streaming",
      }));
    }
    case "ArtifactShown":
      return patchDraft(
        {
          ...slice,
          artifacts: new Map(slice.artifacts).set(e.call_id, e.artifact),
          progress: without(slice.progress, e.call_id),
        },
        e.call_id,
        () => undefined,
      );
    case "AppCallChanged":
      return recordCall(slice, e);
    case "ScrapeProgress":
      return {
        ...slice,
        scrape: new Map(slice.scrape).set(e.call_id, {
          inflight: e.inflight,
          done: e.done,
          limit: e.limit,
        }),
      };
    // 79 §4.4: a `sandbox_*` tool's bring-up line, joined to its card by `call_id` like `ScrapeProgress`.
    case "Progress":
      if (!('call_id' in e) || e.call_id === null) return slice;
      return {
        ...slice,
        sandbox: new Map(slice.sandbox).set(e.call_id, { step: e.step, line: e.line, pct: e.pct }),
      };
    case "SkillsChanged":
      return { ...slice, skills: e.skills };
    // A full snapshot committed with its tool batch; the settled reload carries the same list, so `live` is untouched.
    case "TodoChanged":
      return reconcileTodos(slice, e.snapshot, true);
    // One round settled (51 §6.4): the gauge moves with the cost pill. Same figure `TurnFinished`
    // would carry, so it is not an estimate; a settled snapshot is unaffected, so `live` holds.
    case "ContextUsed":
      return { ...slice, context: e.context, contextEstimated: false, contextSize: e.size ?? slice.contextSize };
    // The store moved past what is on screen (48 §5): controls disable, in-flight pages and snapshots
    // are discarded, and the route reloads through the settled path. The revision we already installed
    // (our own edit's echo) changes nothing unless a refused reload left us stale. 183 §5.1: a `replicated` (or
    // `moved`) one always re-reads — the owner's relayed event may carry the same revision before its rows landed here.
    case "ContextChanged": {
      if (slice.contextRevision === e.revision && !slice.contextStale && e.reason !== "replicated" && e.reason !== "moved")
        return slice;
      // 183 §5.1: a viewer following the owner already shows the text a replicated 5 s partial carries; re-reading it
      // mid-stream swapped the live reply for a greyed partial, and the next delta restarted it from empty. The owner's
      // `TurnFinished` settles instead (c.$session). With no follow stream nothing is open here, so the partial redraws.
      // R2: only while the follow stream is up — once it dropped, no `TurnFinished` is coming to settle the tail.
      if (e.reason === "replicated" && slice.following === true && openTail(slice)) return slice;
      return {
        ...slice,
        contextRevision: e.revision,
        contextStale: true,
        contextEditable: false,
        contextEstimated:
          e.reason === "settled" ? slice.contextEstimated : true,
        historyGeneration: slice.historyGeneration + 1,
        live: slice.live + 1,
      };
    }
    // 183 §12.1 / R2: the follow stream to the owner came up, or ended. Ended mid-reply (a link blip, a dropped slow
    // viewer, an owner restart), the owner's `TurnFinished` may never reach this viewer: the tail is no longer live, so
    // the slice goes stale and the route settles from the replicated rows (`App.tsx` `relayGuards` sends the reload).
    case "FollowState":
      if (e.live) return slice.following === true ? slice : { ...slice, following: true };
      if (slice.following !== true && !openTail(slice)) return slice.following === false ? slice : { ...slice, following: false };
      return {
        ...slice,
        following: false,
        ...(slice.loaded
          ? { contextStale: true, todosStale: true, historyGeneration: slice.historyGeneration + 1, live: slice.live + 1 }
          : {}),
      };
    // An ACP child's harness reported its LLM (06 §8.2): header text, not transcript state — no tail close, no `live` bump.
    case "HarnessModelChanged":
      return slice.harnessModel === e.model ? slice : { ...slice, harnessModel: e.model };
    // The child's command manifest (75 §3): the skills page reads it — same shape of change as the harness model.
    case "HarnessCommandsChanged":
      return { ...slice, harnessCommands: e.commands };
    // The chat's knowledge harvester pick: the model menu's query holds it (`useInvalidateOn`) — no tail close, no `live` bump.
    case "KnowledgeHarvesterChanged":
      return slice;
  }
  // Unknown events (approvals, speech, …) leave the slice — and a streaming tail — alone.
  const next = turn(closeTail(slice, e, ctx), e, ctx);
  return next === undefined ? slice : { ...next, live: slice.live + 1 };
}

/** The turn's own events, after the tail was closed: anything that changes what a settled snapshot would have to contain. */
function turn(
  s: SessionSlice,
  e: WireEvent,
  ctx: Ctx,
): SessionSlice | undefined {
  const idle = s.running ? "thinking" : "idle";
  switch (e.type) {
    // Echo the turn's input unless we sent it, it was a steer whose card
    // moved up on `InputInjected`, or the engine injected it
    // (`MessageDelivered`/`PeerDelivered` show those; a wrapper never renders
    // as a plain `you` row). Own sends and injected steers are already on
    // screen: stamp them with the turn instead. 171F B4: parked cell results
    // lead the text; they ride on the user's entry, or show as result cards
    // when no words follow them (a review wake), as the reload does.
    case "TurnStarted": {
      const { view, text: raw } = userText(e.text);
      const { cells, rest: text } = splitLeading(raw);
      const cls = classify(text);
      // 152 D1: a `<resume_handoff>` has no delivery event of its own, so its card is echoed here.
      const wrapped = cls !== null && cls[0]?.kind !== "skill" && cls[0]?.kind !== "resume";
      const own =
        !wrapped && (s.lastInjected === e.turn || ctx.sent.delete(text));
      const now = Date.now();
      // 199: a file notice is automatic input with no delivery event of its own: its card is echoed here (a message
      // row's `receiving` placeholders ride on its `PeerDelivered` card instead).
      const notices: Entry[] = wrapped ? (cls ?? []).flatMap((c): Entry[] => (c.kind === "file" && c.file.state !== "receiving" ? [{ id: nextId(), ...c, at: now }] : [])) : [];
      const entries = own
        ? stampUser(s.entries, e.turn_seq, cells)
        : wrapped
          ? cells.length || notices.length ? [...s.entries, ...cells.map((c): Entry => ({ id: nextId(), ...c, at: now })), ...notices] : s.entries
          : [
              ...s.entries,
              ...(view ? [view] : []),
              ...userEntries(text, cls, undefined, e.turn_seq, undefined, e.channel).map((x): Entry => ({ ...x, at: now, ...(cells.length && x.kind === "user" ? { cells } : {}) })),
            ];
      return {
        ...s,
        entries,
        running: true,
        waiting: true,
        activity: "thinking",
        model: e.model,
        turnSeq: e.turn_seq,
        lastInjected: undefined,
        // 186: the opener's `PeerDelivered` came first: its row's seq is this turn's.
        turnBy: s.entries.findLast((x): x is Extract<Entry, { kind: "peer" }> => x.kind === "peer" && x.seq === e.turn_seq && x.origin?.contact_mode === "full")?.from,
        // The actor is busy; the settled reload after `TurnDone` restores the authoritative flag.
        contextEditable: false,
        // 109 §8.2: one banner, for the resumed turn only; the next turn clears it.
        resume: s.resume?.turn === e.turn ? s.resume : undefined,
        // 212 §12: the agent's status belongs to the turn that reported it.
        statusText: undefined,
        statusTitle: undefined,
      };
    }
    // The steer enters the context now: its card moves from the queue to
    // the transcript, after everything the model said before it.
    case "InputInjected": {
      const at = s.queued.findIndex((q) => q.id === e.id);
      const q = s.queued[at];
      if (!q) return s;
      const { view, text } = userText(q.text);
      const rows = userEntries(text, classify(text), true, undefined, undefined, q.channel);
      // 171F4 N7: the steer's attachments ride on its `you` row, as on a send's (`markSent`).
      const you = q.files ? rows.findIndex((x) => x.kind === "user") : -1;
      return {
        ...s,
        queued: s.queued.toSpliced(at, 1),
        entries: [...s.entries, ...(view ? [view] : []), ...rows.map((x, i): Entry => (i === you && x.kind === "user" ? { ...x, at: Date.now(), files: q.files } : { ...x, at: Date.now() }))],
        lastInjected: e.turn,
      };
    }
    case "TextDelta": {
      // 109 §8.2: the regenerated reply's first delta replaces the greyed partial.
      const base = s.entries.some((x) => x.kind === "interrupted") ? s.entries.filter((x) => x.kind !== "interrupted") : s.entries;
      const at = base.length - 1;
      const last = base[at];
      const entries =
        last?.kind === "assistant" && last.open
          ? replaceAt(base, at, { ...last, text: last.text + e.text })
          : [
              ...base,
              {
                id: nextId(),
                kind: "assistant" as const,
                text: e.text,
                open: true,
                turn: s.turnSeq,
                at: Date.now(),
              },
            ];
      // 146 D7: a turn-scoped event means a turn runs, whatever a late reseed said.
      return { ...s, entries, running: true, waiting: false, activity: "streaming" };
    }
    case "ReasoningDelta": {
      const at = s.entries.length - 1;
      const last = s.entries[at];
      const entries =
        last?.kind === "reasoning" && !last.done
          ? replaceAt(s.entries, at, { ...last, text: last.text + e.text })
          : [
              ...s.entries,
              {
                id: nextId(),
                kind: "reasoning" as const,
                text: e.text,
                done: false,
                startedAt: ctx.now,
              },
            ];
      return { ...s, entries, waiting: false, activity: "reasoning" };
    }
    case "ToolCallPending":
      return {
        ...s,
        browser: s.browser || e.name.startsWith("browser_"),
        running: true,
        waiting: false,
        activity: "tool",
        entries: [
          ...s.entries,
          {
            id: nextId(),
            kind: "tool",
            callId: e.call_id,
            name: e.name,
            args: undefined,
            pending: true,
            child: e.call_id.includes("."),
            ...(s.turnBy ? { by: s.turnBy } : {}),
          },
        ],
      };
    case "ToolCallStarted": {
      const at = s.entries.findIndex(
        (t) => t.kind === "tool" && t.pending && t.callId === e.call_id,
      );
      const tool: Entry = {
        id: s.entries[at]?.id ?? nextId(),
        kind: "tool",
        callId: e.call_id,
        name: e.name,
        args: e.args,
        pending: false,
        // A `.` in the id marks a call made from inside a `python` cell (08 §4.1).
        child: e.call_id.includes("."),
        startedAt: Date.now(),
        at: Date.now(),
        ...(s.turnBy ? { by: s.turnBy } : {}),
      };
      const entries =
        at < 0 ? [...s.entries, tool] : replaceAt(s.entries, at, tool);
      const next = {
        ...s,
        entries,
        progress: without(s.progress, e.call_id),
        browser: s.browser || e.name.startsWith("browser_"),
        codemap: codeEdited(s.codemap, e.name, e.args),
        running: true,
        waiting: false,
        activity: "tool" as const,
      };
      if (e.name !== "app") return next;
      // The complete arguments: a provider that buffered them still yields a preview until the commit lands.
      return patchDraft(next, e.call_id, (d) => ({
        source: "",
        edit: false,
        ...d,
        ...partialApp(e.args),
        bytes: d?.bytes ?? 0,
        status: "validating",
      }));
    }
    case "ToolCallFinished": {
      // A repeated finish neither replaces the first result nor appends a fallback info line.
      const tool = s.entries.findLast((t): t is Tool => t.kind === "tool" && t.callId === e.call_id);
      if (tool?.result) return undefined;
      const last = s.entries.at(-1);
      const streaming = (last?.kind === "reasoning" && !last.done) || (last?.kind === "assistant" && last.open);
      // 183 §12.4: a resumed batch's first result (relayed from its new owner, or here) opens with `<engine_restart/>`.
      const split = e.output.startsWith("<engine_restart ") ? splitRestart(e.output) : { text: e.output };
      const result = {
        output: split.text,
        isError: e.is_error,
        files: e.files ?? [],
      };
      const next = patchTool(
        s,
        (t) => t.callId === e.call_id,
        (t) => ({ ...t, result, finishedAt: Date.now(), ...(split.restart ? { restart: split.restart } : {}) }),
      );
      // 183 R1: followed mid-turn, a replica sees the result of a call it never saw start (the follow stream is live
      // only from the focus on); a bare "✓ …" line would sit in the transcript until the settle brings the real row.
      const entries =
        next === s
          ? s.resume?.here === false
            ? s.entries
            : [
                ...s.entries,
                {
                  id: nextId(),
                  kind: "info" as const,
                  text: `${e.is_error ? "✗" : "✓"} ${resultLine(e.output)}`,
                },
              ]
          : next.entries;
      return patchDraft(
        {
          ...s,
          entries,
          scrape: without(s.scrape, e.call_id),
          sandbox: without(s.sandbox, e.call_id),
          activity: streaming ? s.activity : "thinking",
        },
        e.call_id,
        (d) =>
          d === undefined
            ? d
            : e.is_error
              ? { ...d, status: "failed", error: e.output }
              : undefined,
      );
    }
    case "TurnFinished": {
      let appDrafts = s.appDrafts;
      for (const [id, d] of s.appDrafts) {
        if (d.status !== "streaming") continue;
        if (appDrafts === s.appDrafts) appDrafts = new Map(appDrafts);
        appDrafts.set(id, { ...d, status: "cancelled" });
      }
      return {
        ...s,
        // A call the model abandoned mid-stream never starts; its app draft stays readable as cancelled source.
        entries: s.entries.filter((t) => !(t.kind === "tool" && t.pending)),
        progress: s.progress.size ? new Map() : s.progress,
        appDrafts,
        usage: e.usage,
        context: e.context,
        contextEstimated: false,
        running: false,
        waiting: false,
        activity: "idle",
        turnBy: undefined,
        statusText: undefined,
        statusTitle: undefined,
      };
    }
    case "Error":
      return {
        ...s,
        activity: idle,
        statusText: undefined,
        statusTitle: undefined,
        entries: [
          ...s.entries,
          { id: nextId(), kind: "error", text: e.message },
        ],
      };
    case "ModelChanged":
      return {
        ...s,
        model: e.model,
        entries: [
          ...s.entries,
          { id: nextId(), kind: "info", text: `model → ${e.model}` },
        ],
      };
    case "EffortChanged": {
      if (s.effort === e.effort) return s;
      const text =
        e.effort === null
          ? `effort reset: not offered by ${s.model ?? ""}`
          : `effort → ${e.effort}`;
      return {
        ...s,
        effort: e.effort,
        entries: [...s.entries, { id: nextId(), kind: "info", text }],
      };
    }
    case "Compacting":
      // Idempotent: the composer dispatches one optimistically before `api.compact` (87 COMPACT-NONBLOCK)
      // and the engine's own arrives moments later — `undefined` keeps the slice (and `live`) untouched.
      return s.activity === "compacting" ? undefined : { ...s, activity: "compacting" };
    case "Compacted":
      // `after` is the engine's calibrated whole-frame estimate (70 COMPACT-SIZE):
      // shown as `≈` until the provider's next `ContextUsed`/`TurnFinished` replaces it.
      // Clear is a clean slate on screen too; `load_session` still shows everything.
      return e.style === "clear"
        ? {
            ...s,
            activity: idle,
            context: e.after,
            contextEstimated: true,
            entries: [
              {
                id: nextId(),
                kind: "info",
                text: `context cleared (epoch ${e.epoch})`,
              },
            ],
          }
        : interject(
            { ...s, activity: idle, context: e.after, contextEstimated: true },
            { id: nextId(), kind: "compacted", epoch: e.epoch, style: e.style },
          );
    // 10 §5.2 as built: the harvest runs detached — may start after `TurnFinished`, may outlive the next
    // `TurnStarted` — so it never touches `activity`; the label is derived (`shownActivity`).
    case "Harvesting":
      return { ...s, harvesting: true };
    case "Harvested":
      return interject(
        { ...s, harvesting: false },
        {
          id: nextId(),
          kind: "info",
          text: `knowledge +${e.added} ~${e.updated}`,
        },
      );
    // 212 §12: the agent's own status line; it outranks the activity word until the turn ends.
    case "StatusChanged":
      return { ...s, statusText: e.status, statusTitle: e.title ?? undefined };
    // 52 §6: the consultation's status line while it runs; its committed note becomes a card once it ends. The live
    // card knows the sources and the model only; the settled reload supplies the body text and per-source statuses.
    case "VisionProgress": {
      if (e.stage === "questioning")
        return { ...s, activity: "vision", visionStatus: `asking ${e.model} what to look for…` };
      if (e.stage === "inspecting") {
        const n = e.sources.length;
        return {
          ...s,
          activity: "vision",
          visionStatus: `${e.model} inspecting ${n} image${n === 1 ? "" : "s"}…`,
        };
      }
      return interject(
        { ...s, activity: idle, visionStatus: undefined },
        {
          id: nextId(),
          kind: "vision",
          model: e.model,
          sources: e.sources,
          text: "",
          seq: e.seq ?? undefined,
          ...(e.stage === "failed" ? { failed: true } : {}),
        },
      );
    }
    // 91: `closeTail` already stamped the block with `e.ms`; the event only
    // has to count as the turn's own so the change is kept.
    case "ReasoningDone":
      return s;
    default:
      return undefined;
  }
}

/** A subtree row's change for one of its session's events, and the line a direct parent shows for it. */
function trackRow(
  e: WireEvent,
): {
  patch?: (row: AgentInfo) => AgentInfo;
  note?: (name: string) => Entry;
} | null {
  const now = () => new Date().toISOString();
  const wake = (row: AgentInfo): AgentInfo =>
    row.state.state === "idle" ? { ...row, state: { state: "running" }, started_at: row.started_at ?? now() } : row;
  switch (e.type) {
    case "TurnStarted":
    // 109 LIVE: a turn resumed after a restart runs without a `TurnStarted`.
    case "TurnResumed":
      return {
        patch: (row) => ({
          ...row,
          state: { state: "running" },
          started_at: now(),
        }),
      };
    case "TurnFinished": {
      const state =
        e.stop_reason === "error"
          ? "error"
          : e.stop_reason === "cancelled"
            ? "cancelled"
            : "idle";
      return {
        patch: (row) => ({
          ...row,
          state: { state },
          started_at: null,
          finished_at: now(),
        }),
        ...(state === "cancelled" && {
          note: (name) => ({
            id: nextId(),
            kind: "info",
            text: `↳ ${name} cancelled`,
          }),
        }),
      };
    }
    // 146 D7: a respawned child's first turn-scoped event lights its dot and pill even if a reseed landed late.
    case "ToolCallPending":
    case "TextDelta":
      return { patch: wake };
    case "ToolCallStarted": {
      const args = e.args;
      if (
        e.name !== "send_message" ||
        typeof args !== "object" ||
        args === null ||
        Array.isArray(args)
      )
        return { patch: wake };
      if (args.await_reply !== true) return null;
      const from = typeof args.to === "string" ? args.to : "?";
      return {
        patch: (row) => ({ ...row, state: { state: "awaiting", from } }),
      };
    }
    case "ToolCallFinished":
      return {
        patch: (row) =>
          row.state.state === "awaiting"
            ? { ...row, state: { state: "running" } }
            : row,
      };
    case "Error":
      return {
        note: (name) => ({
          id: nextId(),
          kind: "error",
          text: `↳ ${name} error: ${e.message}`,
        }),
      };
    // Subagents card: the model the harness reports (or a native switch) lands on every ancestor's row, no reload.
    case "HarnessModelChanged":
    case "ModelChanged":
      return { patch: (row) => (row.model === e.model ? row : { ...row, model: e.model }) };
    default:
      return null;
  }
}

/**
 * 142 F4: the liveness a slice keeps in memory — its own `running`/`activity` and the live bit of every subtree row —
 * is advanced by events only, and a relay socket parked overnight loses the `TurnFinished`s that clear it. Reset both
 * to `running(id)`: the engine's `list_sessions` answer, or `() => false` to drop every mark until one lands. A session
 * it has no answer for (`undefined`) keeps what it has. Returns `s` itself when nothing changes.
 */
export function reseedRunning(
  s: SessionSlice,
  running: (id: string) => boolean | undefined,
  now: string,
): SessionSlice {
  let next = s;
  const own = running(s.id);
  if (own !== undefined && own !== s.running) {
    next = own
      ? { ...next, running: true, waiting: true, activity: "thinking", statusText: undefined, statusTitle: undefined }
      : { ...next, running: false, waiting: false, activity: "idle", statusText: undefined, statusTitle: undefined };
  }
  let children: Record<string, AgentInfo> | undefined;
  for (const [id, row] of Object.entries(s.children)) {
    const r = running(id);
    if (r === undefined || r === live(row.state.state)) continue;
    children ??= { ...s.children };
    children[id] = r
      ? { ...row, state: { state: "running" }, started_at: row.started_at ?? now }
      : { ...row, state: { state: "idle" }, started_at: null, finished_at: row.finished_at ?? now };
  }
  return children ? { ...next, children } : next;
}

/** 93: a child's streaming/activity events; each one beats the ancestors' rows for it (not `TurnFinished`: a finished row sits still). */
const BEATS: Partial<Record<WireEvent["type"], true>> = {
  TextDelta: true,
  ReasoningDelta: true,
  ReasoningDone: true,
  ToolCallPending: true,
  ToolCallStarted: true,
  ToolCallProgress: true,
  ToolCallFinished: true,
  TurnStarted: true,
};
/** A row beats at most this often; a stream of deltas is one blink per quarter second. */
const BEAT_MS = 250;

/** The row stamped with the batch's clock, unless its last beat is younger than `BEAT_MS`. */
function beat(row: AgentInfo, now: number): AgentInfo {
  return row.beat !== undefined && now - row.beat < BEAT_MS ? row : { ...row, beat: now };
}

/**
 * Apply one event to a draft record of slices: the session's own slice, the
 * subtree rows of every ancestor that lists it (design doc 19 §3.3), and on
 * `SubagentSpawned` the child's identity plus the parent's `↳ spawned` line.
 * Touched slices are replaced; untouched ones keep their identity. Returns
 * whether anything changed.
 */
export function route(
  draft: Record<string, SessionSlice>,
  e: WireEvent,
  ctx: Ctx,
): boolean {
  if (e.type === "SubagentSpawned") {
    const child = draft[e.child] ?? empty(e.child);
    draft[e.child] = { ...child, name: e.name, parent: e.parent };
    for (let id: string | null | undefined = e.parent, depth = 1; id; depth++) {
      const s: SessionSlice | undefined = draft[id];
      if (!s) break;
      const row: AgentInfo = receiveComponent("method.agent.output.AgentInfo", {
        id: e.child,
        name: e.name,
        parent: e.parent,
        depth,
        agent: e.agent ?? null,
        // The engine's effective model; a `HarnessModelChanged` that beat this event fills it.
        model: e.model || child.harnessModel || "",
        state: { state: "running" },
        started_at: new Date().toISOString(),
        finished_at: null,
        last: "",
        cap: e.cap,
      });
      const next = { ...s, children: { ...s.children, [e.child]: row } };
      draft[id] =
        depth === 1
          ? interject(next, {
              id: nextId(),
              kind: "info",
              text: `↳ spawned ${e.name}`,
            })
          : next;
      id = s.parent;
    }
    return true;
  }
  // 141 §9.1: a watch-only child the harness announced or moved — the child's slice and a row on every ancestor,
  // no `↳ spawned` line (the roster card is the parent-transcript artefact). A repeat upserts the same row.
  if (e.type === "ObservedChildChanged") {
    const child = draft[e.child] ?? empty(e.child);
    draft[e.child] = {
      ...child,
      name: e.name,
      parent: e.parent,
      kind: "observed",
      observed: e.observed,
      harnessModel: e.model ?? child.harnessModel,
    };
    const state = observedState(e.observed.state);
    const terminal =
      e.observed.state === "completed" ||
      e.observed.state === "failed" ||
      e.observed.state === "cancelled";
    const now = new Date().toISOString();
    for (let id: string | null | undefined = e.parent, depth = 1; id; depth++) {
      const s: SessionSlice | undefined = draft[id];
      if (!s) break;
      const prev = s.children[e.child];
      const row: AgentInfo = receiveComponent("method.agent.output.AgentInfo", {
        ...prev,
        id: e.child,
        name: e.name,
        parent: e.parent,
        depth,
        agent: null,
        model: e.model ?? prev?.model ?? "",
        state,
        started_at: prev?.started_at ?? now,
        finished_at: terminal ? (prev?.finished_at ?? now) : null,
        last: prev?.last ?? "",
        kind: "observed",
        observed: e.observed,
      });
      draft[id] = { ...s, children: { ...s.children, [e.child]: row } };
      id = s.parent;
    }
    return true;
  }
  if (e.type === "SessionDeleted") {
    delete draft[e.session];
    for (const s of Object.values(draft)) {
      if (!(e.session in s.children)) continue;
      const { [e.session]: _gone, ...children } = s.children;
      draft[s.id] = { ...s, children };
    }
    return true;
  }
  // Policy revisions (global `session: null` or a subtree) are query invalidations, not slice state: see `useInvalidateOn`.
  if (e.type === "CommunicationChanged" || !("session" in e) || e.session == null) return false;
  const cur = draft[e.session] ?? empty(e.session);
  const next = apply(cur, e, ctx);
  if (next !== cur) draft[e.session] = next;
  let changed = next !== cur;
  const track = trackRow(e);
  const beats = BEATS[e.type] === true;
  if (track || beats) {
    for (let id = cur.parent, direct = true; id; direct = false) {
      const s: SessionSlice | undefined = draft[id];
      const row = s?.children[e.session];
      if (!s || !row) break;
      let next = track?.patch ? track.patch(row) : row;
      if (beats) next = beat(next, ctx.now);
      let up: SessionSlice = next !== row ? { ...s, children: { ...s.children, [e.session]: next } } : s;
      if (direct && track?.note) up = interject(up, track.note(row.name));
      if (up !== s) {
        draft[id] = up;
        changed = true;
      }
      id = s.parent;
    }
  }
  return changed;
}