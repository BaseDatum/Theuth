// 163 D5: the shutdown overlay's state. Events in — the quit request (with the running turns read off the
// stores), session-store changes, the shell's `quit-progress` steps — and the overlay's title and steps out.
// Pure: `App.tsx` feeds it, `components/launch/QuitOverlay.tsx` draws it, the tests drive it with stubs.

import type { QuitProgress } from '@/lib/tauri'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionSlice } from '@/store/sessions'

/** `durability.drain_secs`' default (109 §9.2): the engine drains running turns this long after `shutdown`. */
export const DRAIN_MS = 10_000

/** What the overlay reads of a session slice and of a `list_sessions` row. */
export type QuitSlice = Pick<SessionSlice, 'running' | 'title' | 'parent' | 'resume'>
export type QuitSummary = Pick<SessionSummary, 'id' | 'title' | 'name' | 'parent' | 'running' | 'archived'>

/** A turn the engine drains at quit, by session; `done` once it finished or was suspended. */
export type QuitTurn = { id: string; title: string; done: boolean }

export type QuitState = {
  restart: boolean
  /** The engine is this app's (18 §3): it drains and stops. A reused engine keeps running. */
  owned: boolean
  /** When the quit was requested (ms since epoch): a suspension after it ticks its turn. */
  at: number
  turns: QuitTurn[]
  /** A `companion` step arrived: a companion runs and is being closed. */
  companion: boolean
  /** The furthest worker step so far; `null` until the worker starts. */
  step: QuitProgress['step'] | null
  /** The engine outlived its grace and was killed. */
  forced: boolean
}

export type QuitEvent =
  | {
      type: 'requested'
      restart: boolean
      owned: boolean
      at: number
      slices: Readonly<Record<string, QuitSlice>>
      summaries: readonly QuitSummary[]
    }
  | { type: 'slices'; slices: Readonly<Record<string, QuitSlice>> }
  | ({ type: 'progress' } & QuitProgress)

export type QuitStepStatus = 'pending' | 'active' | 'done'
export type QuitStep = {
  id: 'saving' | 'companion' | 'engine' | 'closing'
  label: string
  status: QuitStepStatus
  detail?: string
  /** The saving step's turns, each ticked as it ends. */
  items?: { id: string; label: string; done: boolean }[]
  /** Draw the drain-deadline bar under the step. */
  bar?: boolean
}

/**
 * The top-level chats with a turn in flight, sidebar order aside: a slice (fed by live events) is the truth for
 * its session, the `list_sessions` row for a session without one. Subagents end with their parent's turn and
 * archived chats are not shown, so neither gets a row.
 */
function runningTurns(slices: Readonly<Record<string, QuitSlice>>, summaries: readonly QuitSummary[]): QuitTurn[] {
  const rows = new Map(summaries.map((s) => [s.id, s]))
  const ids = new Set([...rows.keys(), ...Object.keys(slices)])
  return [...ids].flatMap((id) => {
    const slice = slices[id]
    const row = rows.get(id)
    if (!(slice ? slice.running : row?.running) || (slice?.parent ?? row?.parent) != null || row?.archived) return []
    return [{ id, title: slice?.title || row?.title || row?.name || 'Untitled chat', done: false }]
  })
}

/** The turn finished (`TurnFinished` clears `running`) or the drain suspended it (`TurnSuspended` after `at`). */
const ended = (slice: QuitSlice | undefined, at: number): boolean =>
  slice !== undefined && (!slice.running || (slice.resume?.kind === 'suspended' && !slice.resume.resumed && slice.resume.at >= at))

/** `null` until a quit is requested. A second request (the shell's after the webview's own) only adds `restart`. */
export function reduce(s: QuitState | null, e: QuitEvent): QuitState | null {
  switch (e.type) {
    case 'requested':
      if (s) return e.restart && !s.restart ? { ...s, restart: true } : s
      return {
        restart: e.restart,
        owned: e.owned,
        at: e.at,
        turns: e.owned ? runningTurns(e.slices, e.summaries) : [],
        companion: false,
        step: null,
        forced: false,
      }
    case 'slices': {
      if (!s || s.turns.every((t) => t.done || !ended(e.slices[t.id], s.at))) return s
      return { ...s, turns: s.turns.map((t) => (t.done || !ended(e.slices[t.id], s.at) ? t : { ...t, done: true })) }
    }
    case 'progress':
      if (!s) return s
      return {
        ...s,
        step: e.step,
        companion: s.companion || e.step === 'companion',
        forced: s.forced || e.forced === true,
      }
  }
}

export const quitTitle = (s: QuitState): string => (s.restart ? 'Restarting to update' : 'Shutting down theuth')

const RANK: Record<QuitProgress['step'], number> = { companion: 1, engine: 2, closing: 3 }

/** The overlay's rows, in D5 order: saving, the companion (only when it runs), the engine (only when ours), closing. */
export function quitSteps(s: QuitState): QuitStep[] {
  const at = s.step ? RANK[s.step] : 0
  const closing = at >= RANK.closing
  const turns = s.turns.map((t) => ({ id: t.id, label: t.title, done: t.done || closing }))
  const saved = !s.owned || turns.every((t) => t.done)
  const saving: QuitStep = {
    id: 'saving',
    label: 'Saving your conversations',
    status: saved ? 'done' : 'active',
    detail: !s.owned ? 'The engine keeps running' : turns.length === 0 ? 'Nothing to save' : undefined,
    items: turns.length ? turns : undefined,
    bar: !saved,
  }
  const steps = [saving]
  if (s.companion)
    steps.push({ id: 'companion', label: 'Closing the companion', status: at > RANK.companion ? 'done' : 'active' })
  if (s.owned)
    steps.push({
      id: 'engine',
      label: 'Stopping the engine',
      status: closing ? 'done' : at >= RANK.engine ? 'active' : 'pending',
      detail: s.forced ? 'stopped after the time limit' : undefined,
    })
  steps.push({ id: 'closing', label: 'Closing', status: closing ? 'active' : 'pending' })
  return steps
}
