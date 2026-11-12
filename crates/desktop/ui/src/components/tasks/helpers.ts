import type { AgentSummary } from "@/rpc/bindings/method.roster.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { Owner } from "@/rpc/bindings/method.roster.output"
import type { Task } from "@/rpc/bindings/method.roster.output"
import type { TaskState } from "@/rpc/bindings/method.roster.output"

/** Board columns in order — Backlog first (69 TASKS); `cancelled` sits behind a toggle (67 §12). */
export const COLUMNS: readonly TaskState[] = ['backlog', 'todo', 'doing', 'review', 'blocked', 'done']
export const STATES: readonly TaskState[] = [...COLUMNS, 'cancelled']
/** The two columns a card may be dragged between (user-intent moves; everything else is a button). */
export const DRAG_STATES: readonly TaskState[] = ['backlog', 'todo']
export const canDrag = (from: TaskState, to: TaskState): boolean => from !== to && DRAG_STATES.includes(from) && DRAG_STATES.includes(to)

/** A node the work can run on: this one first, then every mesh peer (the UI says node where the record says engine). */
export type Node = { name: string; here: boolean; reachable: boolean }
export const nodesOf = (m: MeshStatus | undefined): Node[] =>
  m ? [{ name: m.self_name, here: true, reachable: true }, ...m.peers.filter((p) => p.name !== m.self_name).map((p) => ({ name: p.name, here: false, reachable: p.reachable }))] : []

/** The built-in default agent (67 decision 1): root of every reporting line, never a roster entry. */
export const THEUTH = 'theuth'

export const USER: Owner = { type: 'user' }
export const agent = (name: string): Owner => ({ type: 'agent', name })

/** 179 D9: what an action does to agent `name`'s desk: `wakes ops`, or `queues a wake for ops (paused)` while it is paused. */
export const wakePhrase = (name: string, agents: readonly AgentSummary[] | undefined, verb: 'wakes' | 'wake' = 'wakes'): string =>
  agents?.some((a) => a.file.name === name && a.status === 'paused') ? `queues a wake for ${name} (paused)` : `${verb} ${name}`

/** `Owner` → its text form (`agent:ops` | `user` | `system`), the shape filters, `created_by` and `actor` carry. */
export const ownerText = (o: Owner): string => (o.type === 'agent' ? `agent:${o.name}` : o.type)

/** Text form → `Owner`; `theuth` and `user` both mean the user's line ends here. */
export function parseOwner(s: string): Owner {
  return s.startsWith('agent:') ? agent(s.slice(6)) : s === 'system' ? { type: 'system' } : USER
}

/** Chip text: the agent's name, "you", or "system". */
export const ownerLabel = (o: Owner | null): string => (o === null ? '—' : o.type === 'agent' ? o.name : o.type === 'user' ? 'you' : 'system')

/**
 * State chip text (Sky's rulings, 69 TASKS): `backlog` → "not started"; `todo` owned by an agent →
 * "queued for ‹agent›'s desk"; `doing` with a checkout → "at ‹agent›'s desk · open session" (the user's own:
 * "in progress · open session"); `doing` without one (sent back from review, not yet picked up) → "queued for
 * ‹agent›'s desk" / "in progress"; every other state is its name. The UI says node where the record says engine.
 */
export function stateChip(t: Task): string {
  switch (t.state) {
    case 'backlog':
      return 'not started'
    case 'todo':
      return t.owner.type === 'agent' ? `queued for ${t.owner.name}'s desk` : 'todo'
    case 'doing':
      if (t.owner.type !== 'agent') return t.checkout ? 'in progress · open session' : 'in progress'
      return t.checkout ? `at ${t.owner.name}'s desk · open session` : `queued for ${t.owner.name}'s desk`
    default:
      return t.state
  }
}

/** Who reviews by default (67 §7.1): the owner's `reports_to`; `theuth`/unknown manager → the user. */
export function defaultReviewer(owner: Owner, agents: readonly AgentSummary[]): Owner {
  if (owner.type !== 'agent') return USER
  const mgr = agents.find((a) => a.file.name === owner.name)?.file.reports_to
  return !mgr || mgr === THEUTH || mgr === 'user' ? USER : agent(mgr)
}

/** Blocker kinds (67 §7.1): a task id, `decision:<approval id>`, `person:<name>`, or a system phrase (`no continuation`, `engine:*`). */
export type Blocker = { kind: 'task' | 'decision' | 'person' | 'other'; id: string; raw: string }
export function parseBlocker(raw: string): Blocker {
  const i = raw.indexOf(':')
  if (i === -1) return { kind: raw.includes(' ') ? 'other' : 'task', id: raw, raw }
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

/** `n decision(s)`; the queue bar's breakdown pluralises each kind on its own. */
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/**
 * The collapsed queue bar's text (115): `4 need you` · `2 decisions · 1 review · 1 proposed agent`. Kinds at zero are
 * left out, order is decisions · reviews · proposed agents, and a lone item reads `1 needs you`. Pure, so the bar is
 * one line and the wording is tested.
 */
export function queueSummary({ decisions, reviews, proposed }: { decisions: number; reviews: number; proposed: number }): { headline: string; breakdown: string } {
  const total = decisions + reviews + proposed
  const parts = [
    ...(decisions > 0 ? [plural(decisions, 'decision')] : []),
    ...(reviews > 0 ? [plural(reviews, 'review')] : []),
    ...(proposed > 0 ? [plural(proposed, 'proposed agent')] : []),
  ]
  return { headline: total === 1 ? '1 needs you' : `${total} need you`, breakdown: parts.join(' · ') }
}

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

/** Actions the detail page offers per state (67 §7.2 transitions the user may take, 69 backlog ↔ todo). */
export function actionsFor(t: Task): { start: boolean; toBacklog: boolean; sendBack: boolean; markDone: boolean; cancel: boolean; canMarkDone: boolean; unblock: boolean } {
  const open = t.state !== 'done' && t.state !== 'cancelled'
  return {
    start: t.state === 'backlog',
    toBacklog: t.state === 'todo',
    sendBack: t.state === 'review',
    markDone: t.state === 'review',
    // `done` needs a reviewer other than the owner unless the owner is the user (67 §7.2).
    canMarkDone: t.state === 'review' && awaitsMyReview(t),
    cancel: open && t.state !== 'review',
    // A user's unblock clears system blockers; a decision needs its answer, so it is offered where one is pending.
    unblock: t.state === 'blocked' && !t.blockers.some((b) => b.startsWith('decision:')),
  }
}
