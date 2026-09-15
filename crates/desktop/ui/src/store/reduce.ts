import { resultLine } from '@/lib/describe'
import { classify, type Injected } from '@/lib/injected'
import { pushChunk } from '@/lib/jobTail'
import type { HistoryEntry } from '@/rpc/bindings/HistoryEntry'
import type { WireEvent } from '@/rpc/events'
import type { AgentInfo } from '@/rpc/extra'
import type { Entry, JobTail, SessionSlice } from './sessions'

/** What the reducer needs beside the slice: texts we sent, to skip their echo. */
export type Ctx = { sent: Set<string> }

let ids = 0

/** A fresh, process-unique entry id. */
export function nextId(): string {
  return `e${++ids}`
}

/** A session we know nothing about yet. */
export function empty(id: string): SessionSlice {
  return {
    id,
    entries: [],
    running: false,
    waiting: false,
    activity: 'idle',
    queued: [],
    skills: [],
    undone: 0,
    loaded: false,
    more: false,
    firstSeq: null,
    children: {},
    browser: false,
    browserLive: false,
  }
}

function replaceAt(entries: readonly Entry[], at: number, entry: Entry): Entry[] {
  const out = entries.slice()
  out[at] = entry
  return out
}

/**
 * Add an entry that did not come from the model's stream (a peer message, a
 * spawn notice, a divider). It goes above a still-streaming tail so the text
 * keeps accumulating in one entry instead of splitting a markdown block.
 */
function interject(s: SessionSlice, entry: Entry): SessionSlice {
  const last = s.entries[s.entries.length - 1]
  const streaming = (last?.kind === 'assistant' && last.open) || (last?.kind === 'reasoning' && !last.done)
  const at = s.entries.length - (streaming ? 1 : 0)
  return { ...s, entries: s.entries.toSpliced(at, 0, entry) }
}

/** Transcript entries for one user input: the skill chip plus words, one card per harness wrapper, or `you`. `turn` stamps the `you` row (29 §5.1). */
function userEntries(text: string, cls: Injected[] | null, injected?: true, turn?: number): Entry[] {
  const flag = injected ? { injected } : {}
  if (cls === null) return [{ id: nextId(), kind: 'user', text, turn, ...flag }]
  return cls.map((c): Entry => {
    switch (c.kind) {
      case 'skill':
        return { id: nextId(), kind: 'user', text: c.words, skill: c.name, turn, ...flag }
      case 'peer':
        return { id: nextId(), kind: 'peer', from: c.from, text: c.text }
      case 'subagent':
        return { id: nextId(), kind: 'peer', from: c.name, text: c.text }
      case 'job':
        return {
          id: nextId(),
          kind: 'peer',
          from: `${c.id} · ${c.command} · exit ${c.exit} · ${c.secs}s${c.pty ? ' · pty' : ''}`,
          text: c.text,
        }
    }
  })
}

/** The newest `you` row stamped with the turn it opened: an own send or an injected steer whose `TurnStarted` echo was skipped. */
function stampUser(entries: Entry[], turn: number): Entry[] {
  const at = entries.findLastIndex((u) => u.kind === 'user')
  const u = entries[at]
  return u?.kind === 'user' ? replaceAt(entries, at, { ...u, turn }) : entries
}

/** Transcript entries for one history record. */
export function fromHistory(h: HistoryEntry): Entry[] {
  switch (h.role) {
    case 'user':
      return userEntries(h.text, classify(h.text), undefined, h.turn ?? undefined)
    case 'assistant':
      return h.text.trim() ? [{ id: nextId(), kind: 'assistant', text: h.text, open: false, turn: h.turn ?? undefined }] : []
    case 'tool':
      return [
        {
          id: nextId(),
          kind: 'tool',
          callId: h.call_id,
          name: h.name,
          args: h.args,
          pending: false,
          child: h.call_id.includes('.'),
          ...(h.result ? { result: { output: h.result[0], isError: h.result[1], files: h.files } } : {}),
        },
      ]
    case 'compacted':
      return [{ id: nextId(), kind: 'compacted', epoch: h.epoch, style: h.style, summary: h.summary }]
  }
}

/** Anything but its own delta finishes a streaming tail: text before and after a tool call are separate entries. */
function closeTail(s: SessionSlice, e: WireEvent): SessionSlice {
  const at = s.entries.length - 1
  const last = s.entries[at]
  if (last?.kind === 'reasoning' && !last.done && e.type !== 'ReasoningDelta') {
    return { ...s, entries: replaceAt(s.entries, at, { ...last, done: true }) }
  }
  if (last?.kind === 'assistant' && last.open && e.type !== 'TextDelta') {
    return { ...s, entries: last.text.trim() ? replaceAt(s.entries, at, { ...last, open: false }) : s.entries.slice(0, at) }
  }
  return s
}

type Tool = Extract<Entry, { kind: 'tool' }>

/** Patch the newest tool card `pick` selects; the slice unchanged when none matches. */
function patchTool(s: SessionSlice, pick: (t: Tool) => boolean, patch: (t: Tool) => Entry): SessionSlice {
  for (let at = s.entries.length - 1; at >= 0; at--) {
    const t = s.entries[at]
    if (t?.kind === 'tool' && pick(t)) return { ...s, entries: replaceAt(s.entries, at, patch(t)) }
  }
  return s
}

/** Patch the tail of job `id` on its tool card; `f` returning `undefined` drops the tail. */
function patchJob(s: SessionSlice, id: string, f: (job: JobTail) => JobTail | undefined): SessionSlice {
  return patchTool(
    s,
    (t) => t.job?.id === id,
    (t) => {
      const { job, ...rest } = t
      const next = job && f(job)
      return next ? { ...rest, job: next } : rest
    },
  )
}

/**
 * Fold one event into its session's slice (mirrors `tui::model::apply_agent`
 * and the job arm of `Model::apply`). Returns `slice` itself when nothing
 * changed; never mutates it. `ctx.sent` loses a text once its echo is skipped.
 */
export function apply(slice: SessionSlice, e: WireEvent, ctx: Ctx): SessionSlice {
  // Bookkeeping that must not finish a streaming tail.
  switch (e.type) {
    case 'InputQueued':
      return slice.queued.some((q) => q.id === e.id) ? slice : { ...slice, queued: [...slice.queued, { id: e.id, text: e.text }] }
    case 'InputWithdrawn': {
      const queued = slice.queued.filter((q) => q.id !== e.id)
      return queued.length === slice.queued.length ? slice : { ...slice, queued }
    }
    case 'MessageDelivered':
      return interject(slice, { id: nextId(), kind: 'peer', from: e.from, text: e.text })
    case 'TitleChanged':
      return slice.title === e.title ? slice : { ...slice, title: e.title }
    case 'JobStarted':
      return patchTool(
        slice,
        (t) => t.callId === e.call_id,
        (t) => ({ ...t, job: { id: e.id, command: e.command, lines: [] } }),
      )
    case 'JobOutput':
      return patchJob(slice, e.id, (j) => ({ ...j, lines: pushChunk(j.lines, e.chunk) }))
    case 'JobScreen':
      return patchJob(slice, e.id, (j) => ({ ...j, screen: { rows: e.rows, cursor: e.cursor } }))
    case 'JobFinished':
      // Within grace the tool result carries the output: the tail has nothing to add.
      return patchJob(slice, e.id, (j) => (e.inline ? undefined : { ...j, exit: { exit: e.exit, secs: Number(e.secs) } }))
    case 'BrowserShowLive':
      return { ...slice, browser: true, browserLive: true }
  }
  const s = closeTail(slice, e)
  const idle = s.running ? 'thinking' : 'idle'
  switch (e.type) {
    // Echo the turn's input unless we sent it, it was a steer whose card
    // moved up on `InputInjected`, or the harness injected it
    // (`MessageDelivered` already showed those). Own sends and injected
    // steers are already on screen: stamp them with the turn instead.
    case 'TurnStarted': {
      const cls = classify(e.text)
      const wrapped = cls !== null && cls[0]?.kind !== 'skill'
      const own = !wrapped && (s.lastInjected === e.turn || ctx.sent.delete(e.text))
      const entries = own
        ? stampUser(s.entries, e.turn_seq)
        : wrapped
          ? s.entries
          : [...s.entries, ...userEntries(e.text, cls, undefined, e.turn_seq)]
      return {
        ...s,
        entries,
        running: true,
        waiting: true,
        activity: 'thinking',
        model: e.model,
        turnSeq: e.turn_seq,
        lastInjected: undefined,
      }
    }
    // The steer enters the context now: its card moves from the queue to
    // the transcript, after everything the model said before it.
    case 'InputInjected': {
      const at = s.queued.findIndex((q) => q.id === e.id)
      const q = s.queued[at]
      if (!q) return s
      return {
        ...s,
        queued: s.queued.toSpliced(at, 1),
        entries: [...s.entries, ...userEntries(q.text, classify(q.text), true)],
        lastInjected: e.turn,
      }
    }
    case 'TextDelta': {
      const at = s.entries.length - 1
      const last = s.entries[at]
      const entries =
        last?.kind === 'assistant' && last.open
          ? replaceAt(s.entries, at, { ...last, text: last.text + e.text })
          : [...s.entries, { id: nextId(), kind: 'assistant' as const, text: e.text, open: true, turn: s.turnSeq }]
      return { ...s, entries, waiting: false, activity: 'streaming' }
    }
    case 'ReasoningDelta': {
      const at = s.entries.length - 1
      const last = s.entries[at]
      const entries =
        last?.kind === 'reasoning' && !last.done
          ? replaceAt(s.entries, at, { ...last, text: last.text + e.text })
          : [...s.entries, { id: nextId(), kind: 'reasoning' as const, text: e.text, done: false }]
      return { ...s, entries, waiting: false, activity: 'reasoning' }
    }
    case 'ToolCallPending':
      return {
        ...s,
        browser: s.browser || e.name.startsWith('browser_'),
        waiting: false,
        activity: 'tool',
        entries: [
          ...s.entries,
          { id: nextId(), kind: 'tool', callId: e.call_id, name: e.name, args: undefined, pending: true, child: e.call_id.includes('.') },
        ],
      }
    case 'ToolCallStarted': {
      const at = s.entries.findIndex((t) => t.kind === 'tool' && t.pending && t.callId === e.call_id)
      const tool: Entry = {
        id: s.entries[at]?.id ?? nextId(),
        kind: 'tool',
        callId: e.call_id,
        name: e.name,
        args: e.args,
        pending: false,
        // A `.` in the id marks a call made from inside a `python` cell (08 §4.1).
        child: e.call_id.includes('.'),
      }
      const entries = at < 0 ? [...s.entries, tool] : replaceAt(s.entries, at, tool)
      return { ...s, entries, browser: s.browser || e.name.startsWith('browser_'), waiting: false, activity: 'tool' }
    }
    case 'ToolCallFinished': {
      const result = { output: e.output, isError: e.is_error, files: e.files ?? [] }
      const next = patchTool(
        s,
        (t) => t.callId === e.call_id && !t.result,
        (t) => ({ ...t, result }),
      )
      const entries =
        next === s
          ? [...s.entries, { id: nextId(), kind: 'info' as const, text: `${e.is_error ? '✗' : '✓'} ${resultLine(e.output)}` }]
          : next.entries
      return { ...s, entries, activity: 'thinking' }
    }
    case 'TurnFinished':
      return {
        ...s,
        // A call the model abandoned mid-stream never starts.
        entries: s.entries.filter((t) => !(t.kind === 'tool' && t.pending)),
        usage: e.usage,
        context: e.context,
        running: false,
        waiting: false,
        activity: 'idle',
      }
    case 'Error':
      return { ...s, activity: idle, entries: [...s.entries, { id: nextId(), kind: 'error', text: e.message }] }
    case 'ModelChanged':
      return { ...s, model: e.model, entries: [...s.entries, { id: nextId(), kind: 'info', text: `model → ${e.model}` }] }
    case 'EffortChanged': {
      if (s.effort === e.effort) return s
      const text = e.effort === null ? `effort reset: not offered by ${s.model ?? ''}` : `effort → ${e.effort}`
      return { ...s, effort: e.effort, entries: [...s.entries, { id: nextId(), kind: 'info', text }] }
    }
    case 'Compacting':
      return { ...s, activity: 'compacting' }
    case 'Compacted':
      // Clear is a clean slate on screen too; `load_session` still shows everything.
      return e.style === 'clear'
        ? { ...s, activity: idle, entries: [{ id: nextId(), kind: 'info', text: `context cleared (epoch ${e.epoch})` }] }
        : interject({ ...s, activity: idle }, { id: nextId(), kind: 'compacted', epoch: e.epoch, style: e.style })
    case 'Harvesting':
      return { ...s, activity: 'harvesting' }
    case 'Harvested':
      return interject({ ...s, activity: idle }, { id: nextId(), kind: 'info', text: `knowledge +${e.added} ~${e.updated}` })
    case 'SkillsChanged':
      return { ...s, skills: e.skills }
    default:
      return slice
  }
}

/** A subtree row's change for one of its session's events, and the line a direct parent shows for it. */
function trackRow(e: WireEvent): { patch?: (row: AgentInfo) => AgentInfo; note?: (name: string) => Entry } | null {
  const now = () => new Date().toISOString()
  switch (e.type) {
    case 'TurnStarted':
      return { patch: (row) => ({ ...row, state: { state: 'running' }, started_at: now() }) }
    case 'TurnFinished': {
      const state = e.stop_reason === 'error' ? 'error' : e.stop_reason === 'cancelled' ? 'cancelled' : 'idle'
      return {
        patch: (row) => ({ ...row, state: { state }, started_at: null, finished_at: now() }),
        ...(state === 'cancelled' && { note: (name) => ({ id: nextId(), kind: 'info', text: `↳ ${name} cancelled` }) }),
      }
    }
    case 'ToolCallStarted': {
      const args = e.args
      if (e.name !== 'send_message' || typeof args !== 'object' || args === null || Array.isArray(args)) return null
      if (args.await_reply !== true) return null
      const from = typeof args.to === 'string' ? args.to : '?'
      return { patch: (row) => ({ ...row, state: { state: 'awaiting', from } }) }
    }
    case 'ToolCallFinished':
      return { patch: (row) => (row.state.state === 'awaiting' ? { ...row, state: { state: 'running' } } : row) }
    case 'Error':
      return { note: (name) => ({ id: nextId(), kind: 'error', text: `↳ ${name} error: ${e.message}` }) }
    default:
      return null
  }
}

/**
 * Apply one event to a draft record of slices: the session's own slice, the
 * subtree rows of every ancestor that lists it (design doc 19 §3.3), and on
 * `SubagentSpawned` the child's identity plus the parent's `↳ spawned` line.
 * Touched slices are replaced; untouched ones keep their identity. Returns
 * whether anything changed.
 */
export function route(draft: Record<string, SessionSlice>, e: WireEvent, ctx: Ctx): boolean {
  if (e.type === 'SubagentSpawned') {
    const child = draft[e.child] ?? empty(e.child)
    draft[e.child] = { ...child, name: e.name, parent: e.parent }
    for (let id: string | null | undefined = e.parent, depth = 1; id; depth++) {
      const s: SessionSlice | undefined = draft[id]
      if (!s) break
      const row: AgentInfo = {
        id: e.child,
        name: e.name,
        parent: e.parent,
        depth,
        agent: null,
        model: '',
        state: { state: 'running' },
        started_at: new Date().toISOString(),
        finished_at: null,
        last: '',
      }
      const next = { ...s, children: { ...s.children, [e.child]: row } }
      draft[id] = depth === 1 ? interject(next, { id: nextId(), kind: 'info', text: `↳ spawned ${e.name}` }) : next
      id = s.parent
    }
    return true
  }
  if (e.type === 'SessionDeleted') {
    delete draft[e.session]
    for (const s of Object.values(draft)) {
      if (!(e.session in s.children)) continue
      const { [e.session]: _gone, ...children } = s.children
      draft[s.id] = { ...s, children }
    }
    return true
  }
  if (!('session' in e)) return false
  const cur = draft[e.session] ?? empty(e.session)
  const next = apply(cur, e, ctx)
  if (next !== cur) draft[e.session] = next
  let changed = next !== cur
  const track = trackRow(e)
  if (track) {
    for (let id = cur.parent, direct = true; id; direct = false) {
      const s: SessionSlice | undefined = draft[id]
      const row = s?.children[e.session]
      if (!s || !row) break
      let up: SessionSlice = track.patch ? { ...s, children: { ...s.children, [e.session]: track.patch(row) } } : s
      if (direct && track.note) up = interject(up, track.note(row.name))
      if (up !== s) {
        draft[id] = up
        changed = true
      }
      id = s.parent
    }
  }
  return changed
}
