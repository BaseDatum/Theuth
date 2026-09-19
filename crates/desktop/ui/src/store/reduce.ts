import { jsonrepair } from "jsonrepair";
import { resultLine } from "@/lib/describe";
import { classify, type Injected } from "@/lib/injected";
import { pushChunk } from "@/lib/jobTail";
import type { ContextEntry } from "@/rpc/bindings/ContextEntry";
import type { HistoryEntry } from "@/rpc/bindings/HistoryEntry";
import type { PeerOrigin } from "@/rpc/bindings/PeerOrigin";
import type { TodoSnapshot } from "@/rpc/bindings/TodoSnapshot";
import type { WireEvent } from "@/rpc/events";
import type { AgentInfo, AppCallChanged } from "@/rpc/extra";
import type { AppDraft, Entry, JobTail, SessionSlice } from "./sessions";

/** What the reducer needs beside the slice: texts we sent, to skip their echo. */
export type Ctx = { sent: Set<string> };

let ids = 0;

/** A fresh, process-unique entry id. */
export function nextId(): string {
  return `e${++ids}`;
}

/** A session we know nothing about yet. */
export function empty(id: string): SessionSlice {
  return {
    id,
    entries: [],
    running: false,
    waiting: false,
    activity: "idle",
    queued: [],
    skills: [],
    undone: 0,
    loaded: false,
    more: false,
    firstSeq: null,
    children: {},
    browser: false,
    browserLive: false,
    artifacts: new Map(),
    progress: new Map(),
    scrape: new Map(),
    appDrafts: new Map(),
    appCalls: new Map(),
    artifactPanel: null,
    contextEstimated: false,
    contextEditable: false,
    contextStale: false,
    historyGeneration: 0,
    live: 0,
    todos: null,
    todosStale: false,
    todosExpanded: false,
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

/** Transcript entries for one user input: the skill chip plus words, one card per harness wrapper, or `you`. `turn` stamps
 * the `you` row (29 §5.1); every card of one stored row shares its `context` (48 §3.2). */
function userEntries(
  text: string,
  cls: Injected[] | null,
  injected?: true,
  turn?: number,
  context?: ContextEntry,
): Entry[] {
  const flag = injected ? { injected } : {};
  if (cls === null)
    return [{ id: nextId(), kind: "user", text, turn, context, ...flag }];
  return cls.map((c): Entry => {
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
      case "schedule":
        return {
          id: nextId(),
          kind: "peer",
          from: `${c.name} · ${hhmm(c.at)}`,
          text: c.text,
          context,
        };
      case "harness":
        return { id: nextId(), ...c, context };
    }
  });
}

/** The newest `you` row stamped with the turn it opened: an own send or an injected steer whose `TurnStarted` echo was skipped. */
function stampUser(entries: Entry[], turn: number): Entry[] {
  const at = entries.findLastIndex((u) => u.kind === "user");
  const u = entries[at];
  return u?.kind === "user" ? replaceAt(entries, at, { ...u, turn }) : entries;
}

/** The body of a stored peer row: the `<message>` wrapper's content when the row is exactly one wrapper (50 §9 stores
 * the provider-visible form), the raw text otherwise. Display only; identity comes from the typed `PeerOrigin`. */
function peerBody(text: string): string {
  const cls = classify(text)
  const only = cls?.length === 1 ? cls[0] : undefined
  return only?.kind === 'peer' ? only.text : text
}

/** The card a peer exchange renders as, from its authenticated provenance; never from the wrapper's attributes. */
function peerEntry(origin: PeerOrigin, text: string, seq?: number, context?: ContextEntry): Entry {
  return { id: nextId(), kind: 'peer', from: origin.from, text, origin, seq, context }
}

/** Transcript entries for one history record; `context` (48 §3.3) rides along on every card the record becomes. */
export function fromHistory(h: HistoryEntry): Entry[] {
  const context = h.context ?? undefined;
  switch (h.role) {
    case "user":
      if (h.peer)
        return [peerEntry(h.peer, peerBody(h.text), undefined, context)];
      return userEntries(
        h.text,
        classify(h.text),
        undefined,
        h.turn ?? undefined,
        context,
      );
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
    case "tool":
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
          ...(h.result
            ? {
                result: {
                  output: h.result[0],
                  isError: h.result[1],
                  files: h.files ?? [],
                },
              }
            : {}),
        },
      ];
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
  }
}

/** What makes two renderings of one stored row the same card: kind plus its stable handle, not the process-local id. */
function signature(e: Entry): string {
  switch (e.kind) {
    case "tool":
      return `t${e.callId}`;
    case "compacted":
      return `c${e.epoch}`;
    case "peer":
      return e.origin ? `p${e.origin.id}` : `p${e.from}\n${e.text}`;
    case "harness":
      return `h${e.notice}\n${e.text}`;
    case "user":
    case "assistant":
      return `${e.kind[0]}${e.turn ?? ""}\n${e.text}`;
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

/** Anything but its own delta finishes a streaming tail: text before and after a tool call are separate entries. */
function closeTail(s: SessionSlice, e: WireEvent): SessionSlice {
  const at = s.entries.length - 1;
  const last = s.entries[at];
  if (last?.kind === "reasoning" && !last.done && e.type !== "ReasoningDelta") {
    return { ...s, entries: replaceAt(s.entries, at, { ...last, done: true }) };
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
    case "InputQueued":
      return slice.queued.some((q) => q.id === e.id)
        ? slice
        : { ...slice, queued: [...slice.queued, { id: e.id, text: e.text }] };
    case "InputWithdrawn": {
      const queued = slice.queued.filter((q) => q.id !== e.id);
      return queued.length === slice.queued.length
        ? slice
        : { ...slice, queued };
    }
    case "MessageDelivered":
      // Shown now, stored only once a turn injects it: a settled snapshot read meanwhile lacks the card.
      return interject(
        { ...slice, live: slice.live + 1 },
        { id: nextId(), kind: "peer", from: e.from, text: e.text },
      );
    // The consumed exchange as a typed card (50 §10). Its row is already stored, so a settled reload lands the same
    // card: one shows once, whichever arrives first (matched by exchange id, the wrapper echo on `TurnStarted` is skipped).
    case "PeerDelivered":
      if (
        slice.entries.some(
          (x) => x.kind === "peer" && x.origin?.id === e.origin.id,
        )
      )
        return slice;
      return interject(
        { ...slice, live: slice.live + 1 },
        peerEntry(e.origin, e.text, e.seq),
      );
    case "TitleChanged":
      return slice.title === e.title ? slice : { ...slice, title: e.title };
    case "JobStarted":
      return patchTool(
        slice,
        (t) => t.callId === e.call_id,
        (t) => ({ ...t, job: { id: e.id, command: e.command, lines: [] } }),
      );
    case "JobOutput":
      return patchJob(slice, e.id, (j) => ({
        ...j,
        lines: pushChunk(j.lines, e.chunk),
      }));
    case "JobScreen":
      return patchJob(slice, e.id, (j) => ({
        ...j,
        screen: { rows: e.rows, cursor: e.cursor },
      }));
    case "JobFinished":
      // Within grace the tool result carries the output: the tail has nothing to add.
      return patchJob(slice, e.id, (j) =>
        e.inline
          ? undefined
          : { ...j, exit: { exit: e.exit, secs: Number(e.secs) } },
      );
    case "BrowserShowLive":
      return { ...slice, browser: true, browserLive: true };
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
    case "SkillsChanged":
      return { ...slice, skills: e.skills };
    // A full snapshot committed with its tool batch; the settled reload carries the same list, so `live` is untouched.
    case "TodoChanged":
      return reconcileTodos(slice, e.snapshot, true);
    // The store moved past what is on screen (48 §5): controls disable, in-flight pages and snapshots
    // are discarded, and the route reloads through the settled path. The revision we already installed
    // (our own edit's echo) changes nothing unless a refused reload left us stale.
    case "ContextChanged":
      if (slice.contextRevision === e.revision && !slice.contextStale)
        return slice;
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
  // Unknown events (approvals, speech, …) leave the slice — and a streaming tail — alone.
  const next = turn(closeTail(slice, e), e, ctx);
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
    // moved up on `InputInjected`, or the harness injected it
    // (`MessageDelivered`/`PeerDelivered` show those; a wrapper never renders
    // as a plain `you` row). Own sends and injected steers are already on
    // screen: stamp them with the turn instead.
    case "TurnStarted": {
      const cls = classify(e.text);
      const wrapped = cls !== null && cls[0]?.kind !== "skill";
      const own =
        !wrapped && (s.lastInjected === e.turn || ctx.sent.delete(e.text));
      const entries = own
        ? stampUser(s.entries, e.turn_seq)
        : wrapped
          ? s.entries
          : [...s.entries, ...userEntries(e.text, cls, undefined, e.turn_seq)];
      return {
        ...s,
        entries,
        running: true,
        waiting: true,
        activity: "thinking",
        model: e.model,
        turnSeq: e.turn_seq,
        lastInjected: undefined,
        // The actor is busy; the settled reload after `TurnDone` restores the authoritative flag.
        contextEditable: false,
      };
    }
    // The steer enters the context now: its card moves from the queue to
    // the transcript, after everything the model said before it.
    case "InputInjected": {
      const at = s.queued.findIndex((q) => q.id === e.id);
      const q = s.queued[at];
      if (!q) return s;
      return {
        ...s,
        queued: s.queued.toSpliced(at, 1),
        entries: [...s.entries, ...userEntries(q.text, classify(q.text), true)],
        lastInjected: e.turn,
      };
    }
    case "TextDelta": {
      const at = s.entries.length - 1;
      const last = s.entries[at];
      const entries =
        last?.kind === "assistant" && last.open
          ? replaceAt(s.entries, at, { ...last, text: last.text + e.text })
          : [
              ...s.entries,
              {
                id: nextId(),
                kind: "assistant" as const,
                text: e.text,
                open: true,
                turn: s.turnSeq,
              },
            ];
      return { ...s, entries, waiting: false, activity: "streaming" };
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
              },
            ];
      return { ...s, entries, waiting: false, activity: "reasoning" };
    }
    case "ToolCallPending":
      return {
        ...s,
        browser: s.browser || e.name.startsWith("browser_"),
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
      };
      const entries =
        at < 0 ? [...s.entries, tool] : replaceAt(s.entries, at, tool);
      const next = {
        ...s,
        entries,
        progress: without(s.progress, e.call_id),
        browser: s.browser || e.name.startsWith("browser_"),
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
      const result = {
        output: e.output,
        isError: e.is_error,
        files: e.files ?? [],
      };
      const next = patchTool(
        s,
        (t) => t.callId === e.call_id && !t.result,
        (t) => ({ ...t, result }),
      );
      const entries =
        next === s
          ? [
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
          activity: "thinking",
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
      };
    }
    case "Error":
      return {
        ...s,
        activity: idle,
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
      return { ...s, activity: "compacting" };
    case "Compacted":
      // Clear is a clean slate on screen too; `load_session` still shows everything.
      return e.style === "clear"
        ? {
            ...s,
            activity: idle,
            context: e.after,
            entries: [
              {
                id: nextId(),
                kind: "info",
                text: `context cleared (epoch ${e.epoch})`,
              },
            ],
          }
        : interject(
            { ...s, activity: idle, context: e.after },
            { id: nextId(), kind: "compacted", epoch: e.epoch, style: e.style },
          );
    case "Harvesting":
      return { ...s, activity: "harvesting" };
    case "Harvested":
      return interject(
        { ...s, activity: idle },
        {
          id: nextId(),
          kind: "info",
          text: `knowledge +${e.added} ~${e.updated}`,
        },
      );
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
  switch (e.type) {
    case "TurnStarted":
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
    case "ToolCallStarted": {
      const args = e.args;
      if (
        e.name !== "send_message" ||
        typeof args !== "object" ||
        args === null ||
        Array.isArray(args)
      )
        return null;
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
    default:
      return null;
  }
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
      const row: AgentInfo = {
        id: e.child,
        name: e.name,
        parent: e.parent,
        depth,
        agent: null,
        model: "",
        state: { state: "running" },
        started_at: new Date().toISOString(),
        finished_at: null,
        last: "",
      };
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
  if (e.type === "CommunicationChanged" || !("session" in e)) return false;
  const cur = draft[e.session] ?? empty(e.session);
  const next = apply(cur, e, ctx);
  if (next !== cur) draft[e.session] = next;
  let changed = next !== cur;
  const track = trackRow(e);
  if (track) {
    for (let id = cur.parent, direct = true; id; direct = false) {
      const s: SessionSlice | undefined = draft[id];
      const row = s?.children[e.session];
      if (!s || !row) break;
      let up: SessionSlice = track.patch
        ? { ...s, children: { ...s.children, [e.session]: track.patch(row) } }
        : s;
      if (direct && track.note) up = interject(up, track.note(row.name));
      if (up !== s) {
        draft[id] = up;
        changed = true;
      }
      id = s.parent;
    }
  }
  return changed;
}
