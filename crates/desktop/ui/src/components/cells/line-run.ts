// 171F4 T2-6: the gutter's answer to a line or selection ►. While the run is in flight the pressed line shows it is
// running and a repeat press of that line is ignored; a run that ends with no output shows ✓ (exit 0) or ✗ for
// FEEDBACK_MS, then the normal ►. A run with output answers in the output pane: the mark just clears.
// CELLAFTER (§7.1, §7.6): a ► on a danger cell asks first (`LineConfirm`, unless "Don't ask again" for that cell); on a
// non-danger cell whose `after` gate is not armed the first ► only warns by its line (`pastGate`).
import { useEffect, useRef, useState } from 'react'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { announce } from './Cell'
import { ended, refKey, scopeKey, scopeOf } from './store'

export const FEEDBACK_MS = 1500

/** `run`/`queued_ms` stay `null` until `cell_run` answers with the record. */
export type LineMark = { line: number; phase: 'running' | 'ok' | 'fail'; run: string | null; queued_ms: number | null }

/** How each phase looks in the chat (CodeMirror) and note gutters. */
export const MARK_LOOK: Record<LineMark['phase'], { glyph: string; text: string; tone: string }> = {
  running: { glyph: '◌', text: 'Running', tone: 'animate-pulse text-fg-accent' },
  ok: { glyph: '✓', text: 'Ran · exit 0 · no output', tone: 'text-success' },
  fail: { glyph: '✗', text: 'Ran · failed · no output', tone: 'text-danger' },
}

/** A press of `line`: ignored (the same mark back) while that line's run is in flight. */
export function pressed(m: LineMark | null, line: number): LineMark | null {
  return busy(m, line) ? m : { line, phase: 'running', run: null, queued_ms: null }
}

export const busy = (m: LineMark | null, line: number) => m?.line === line && m.phase === 'running'

/** `cell_run` answered for `line`'s press. */
export function started(m: LineMark | null, line: number, run: CellRun): LineMark | null {
  return m?.line === line && m.phase === 'running' && m.run === null ? { ...m, run: run.id, queued_ms: run.queued_ms } : m
}

/** The press failed (the cell's notice says why): no mark. */
export const refused = (m: LineMark | null, line: number): LineMark | null => (m?.line === line && m.run === null ? null : m)

/** The cell's latest run moved: ✓/✗ when ours ended quiet, nothing when it printed or a newer run took over. */
export function settled(m: LineMark | null, latest: CellRun | null): LineMark | null {
  if (!m || m.phase !== 'running' || m.run === null || !latest) return m
  if (latest.id !== m.run) return m.queued_ms !== null && latest.queued_ms > m.queued_ms ? null : m
  if (!ended(latest)) return m
  if (latest.output_bytes) return null
  return { ...m, phase: latest.exit?.type === 'code' && latest.exit.value === 0 ? 'ok' : 'fail' }
}

export type HoverEvent = { kind: 'move'; line: number | null } | { kind: 'leave'; inEditor: boolean }

/**
 * 171F B2: the hovered line after a pointer event. A move (text or the ► gutter) hovers its line; leaving the text
 * for somewhere inside the editor (the ► itself) keeps it; only leaving the editor clears it.
 */
export function hoverAfter(current: number | null, e: HoverEvent): number | null {
  if (e.kind === 'move') return e.line
  return e.inEditor ? current : null
}

/**
 * 171F4 N2: the note overlay's hovered line after a move at `x`. The band left of the code text (`textLeft`), where
 * the ► sits, is the chat gutter's grace: it keeps the line (WebKit's `caretRangeFromPoint` there answers a header
 * offset, which used to clear the ►). Over the text it follows `line` (`null` on a header line).
 */
export const noteHover = (current: number | null, x: number, textLeft: number, line: number | null) =>
  hoverAfter(current, x < textLeft ? { kind: 'leave', inEditor: true } : { kind: 'move', line })

/** CELLAFTER: how long a ►'s warning stays, and waits for the second press. */
export const WARN_MS = 5000

/** The warned ►: its line and when. */
export type Warned = { line: number; at: number }

/**
 * 171 §7.6 (CELLAFTER): a line or selection ► on a non-danger cell whose `after` gate is not armed. The first press
 * only warns; a second press of the same line within `WARN_MS` runs; a later one, or one on another line, warns again.
 */
export function pastGate(w: Warned | null, line: number, now: number): { run: boolean; warned: Warned | null } {
  if (w?.line === line && now - w.at <= WARN_MS) return { run: true, warned: null }
  return { run: false, warned: { line, at: now } }
}

/** What a pointer ► does (§7.1, §7.6): asks (a danger cell, with the gate's reason when it is also disarmed), warns first (a disarmed cell), or runs. */
export type LineGuard = { kind: 'run' } | { kind: 'confirm'; reason: string | null } | { kind: 'warn'; text: string }

/**
 * `skipAsk`: the user ticked "Don't ask again for this cell" (`askSkipped`); the gate still warns then. The warning
 * drops the reason's own "— run it again" advice, which the "press ► again" offer replaces.
 */
export function lineGuard(danger: boolean, skipAsk: boolean, disarmed: string | null): LineGuard {
  if (danger && !skipAsk) return { kind: 'confirm', reason: disarmed }
  if (disarmed !== null) return { kind: 'warn', text: `${disarmed.replace(/ — run it again$/, '')} — press ► again to run anyway` }
  return { kind: 'run' }
}

/**
 * A cell's stable identity for "Don't ask again": scope + `name` (survives edits, reloads and a re-presented chat
 * cell), else its ref (the note block or the chat position).
 */
export const cellIdentity = (ref: CellRef, cell: CellSpec) => (cell.options.name ? `${scopeKey(scopeOf(ref))}/${cell.options.name}` : refKey(ref))

const SKIP_ASK = 'cells.skip-danger-line-confirm:'
/** "Don't ask again for this cell": a per-device UI preference in localStorage (like the theme), keyed by `cellIdentity`. */
export const askSkipped = (id: string) => globalThis.localStorage?.getItem(SKIP_ASK + id) === '1'
export const skipAsking = (id: string) => localStorage.setItem(SKIP_ASK + id, '1')

/**
 * An open danger ask (§7.1): everything fixed at the ► press — the command's first and last line, what runs, and the
 * cell text then (`text`; its digest is the `seen` the run sends, so a cell changed since is refused, inv. 5).
 */
export type Ask = {
  line: number
  last: number
  mode: CellRunMode
  what: 'line' | 'selection'
  text: string
  reason: string | null
  identity: string
}

/** What happens to an open ask: Run (with the "Don't ask again" tick), Cancel / Esc / a press outside it, or the cell's text now. */
export type AskEvent = { kind: 'run'; skip: boolean } | { kind: 'cancel' } | { kind: 'code'; text: string }

/**
 * CELLAFTER M1: the ask after `e`, and the run it starts. Only Run starts one, on the press-time text and mode, always
 * confirmed; a tick is persisted then. Any change of the cell's text closes the ask without running.
 */
export function askStep(ask: Ask, e: AskEvent): { ask: Ask | null; run: { line: number; mode: CellRunMode; text: string } | null } {
  if (e.kind === 'run') {
    if (e.skip) skipAsking(ask.identity)
    return { ask: null, run: { line: ask.line, mode: ask.mode, text: ask.text } }
  }
  if (e.kind === 'code' && e.text === ask.text) return { ask, run: null }
  return { ask: null, run: null }
}

/**
 * The gutter mark of one cell. `run(line, start, warning)` presses `line` (`start` sends it) unless that line's run
 * is in flight, or `warning` (`lineGuard`'s warn) makes this the first press, which only shows it (`warn`); returns whether
 * it pressed.
 */
export function useLineRun(latest: CellRun | null) {
  const [mark, setMark] = useState<LineMark | null>(null)
  // Read synchronously by `run`: repeat presses land before React re-renders.
  const now = useRef(mark)
  const last = useRef(latest)
  last.current = latest
  const update = (f: (m: LineMark | null) => LineMark | null) => {
    now.current = f(now.current)
    setMark(now.current)
  }
  useEffect(() => update((m) => settled(m, latest)), [latest])
  useEffect(() => {
    if (mark?.phase !== 'ok' && mark?.phase !== 'fail') return
    const t = setTimeout(() => update((m) => (m === mark ? null : m)), FEEDBACK_MS)
    return () => clearTimeout(t)
  }, [mark])
  const [warn, setWarn] = useState<{ line: number; text: string } | null>(null)
  const warned = useRef<Warned | null>(null)
  useEffect(() => {
    if (!warn) return
    const t = setTimeout(() => setWarn(null), WARN_MS)
    return () => clearTimeout(t)
  }, [warn])
  const run = (line: number, start: () => Promise<CellRun> | undefined, warning: string | null = null): boolean => {
    if (busy(now.current, line)) return false
    if (warning) {
      const g = pastGate(warned.current, line, Date.now())
      warned.current = g.warned
      setWarn(g.warned && { line, text: warning })
      // S3: spoken through the one live region on <body> (CELLHIST: never aria-live inside a note's node view).
      if (g.warned) announce(warning)
      if (!g.run) return false
    }
    update((m) => pressed(m, line))
    start()?.then(
      (r) => update((m) => settled(started(m, line, r), last.current)),
      () => update((m) => refused(m, line)),
    )
    return true
  }
  return { mark, warn, run }
}
