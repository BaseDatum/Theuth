import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { Owner } from '@/rpc/bindings/Owner'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskState } from '@/rpc/bindings/TaskState'

/** Board columns in order; `cancelled` sits behind a toggle (67 §12). */
export const COLUMNS: readonly TaskState[] = ['todo', 'doing', 'review', 'blocked', 'done']
export const STATES: readonly TaskState[] = [...COLUMNS, 'cancelled']

/** The built-in default agent (67 decision 1): root of every reporting line, never a roster entry. */
export const THEUTH = 'theuth'

export const USER: Owner = { type: 'user' }
export const agent = (name: string): Owner => ({ type: 'agent', name })

/** `Owner` → its text form (`agent:ops` | `user`), the shape filters, `created_by` and `actor` carry. */
export const ownerText = (o: Owner): string => (o.type === 'user' ? 'user' : `agent:${o.name}`)

/** Text form → `Owner`; `theuth` and `user` both mean the user's line ends here. */
export function parseOwner(s: string): Owner {
  return s.startsWith('agent:') ? agent(s.slice(6)) : USER
}

/** Chip text: the agent's name, or "you". */
export const ownerLabel = (o: Owner | null): string => (o === null ? '—' : o.type === 'user' ? 'you' : o.name)

/** Who reviews by default (67 §7.1): the owner's `reports_to`; `theuth`/unknown manager → the user. */
export function defaultReviewer(owner: Owner, agents: readonly AgentSummary[]): Owner {
  if (owner.type === 'user') return USER
  const mgr = agents.find((a) => a.file.name === owner.name)?.file.reports_to
  return !mgr || mgr === THEUTH || mgr === 'user' ? USER : agent(mgr)
}

/** Blocker kinds (67 §7.1): a task id, `decision:<approval id>`, or `person:<name>`. */
export type Blocker = { kind: 'task' | 'decision' | 'person' | 'other'; id: string; raw: string }
export function parseBlocker(raw: string): Blocker {
  const i = raw.indexOf(':')
  if (i === -1) return { kind: 'task', id: raw, raw }
  const prefix = raw.slice(0, i)
  const id = raw.slice(i + 1)
  return prefix === 'decision' || prefix === 'person' ? { kind: prefix, id, raw } : { kind: 'other', id: raw, raw }
}

/** A blocked task that needs a human: its first blocker is a decision or a person (67 §11). */
export const needsPerson = (t: Task): boolean =>
  t.state === 'blocked' && t.blockers.some((b) => b.startsWith('decision:') || b.startsWith('person:'))

/** `review` and the user is the reviewer, or the owner is the user and no reviewer is named. */
export const awaitsMyReview = (t: Task): boolean =>
  t.state === 'review' && (t.reviewer ? t.reviewer.type === 'user' : t.owner.type === 'user')

/** `3s` / `12m` / `5h` / `3d` since `ms`; `now` is injectable for tests. */
export function age(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 60) return `${s}s`
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.round(m / 60)
  if (h < 48) return `${h}h`
  return `${Math.round(h / 24)}d`
}

/** Short id for chips and the TUI: the last six characters of a uuidv7 (the time-ordered head repeats). */
export const shortId = (id: string): string => id.slice(-6)

/** Group tasks by column; `cancelled` only when asked. Each column keeps the engine's order (oldest first). */
export function columns(tasks: readonly Task[], withCancelled: boolean): { state: TaskState; tasks: Task[] }[] {
  const states = withCancelled ? STATES : COLUMNS
  return states.map((state) => ({ state, tasks: tasks.filter((t) => t.state === state) }))
}

/** Actions the detail page offers per state (67 §7.2 transitions the user may take). */
export function actionsFor(t: Task): { sendBack: boolean; markDone: boolean; cancel: boolean; canMarkDone: boolean } {
  const open = t.state !== 'done' && t.state !== 'cancelled'
  return {
    sendBack: t.state === 'review',
    markDone: t.state === 'review',
    // `done` needs a reviewer other than the owner unless the owner is the user (67 §7.2).
    canMarkDone: t.state === 'review' && awaitsMyReview(t),
    cancel: open && t.state !== 'review',
  }
}
