// 171F4 T2-6: the gutter's answer to a line or selection ▶. While the run is in flight the pressed line shows it is
// running and a repeat press of that line is ignored; a run that ends with no output shows ✓ (exit 0) or ✗ for
// FEEDBACK_MS, then the normal ►. A run with output answers in the output pane: the mark just clears.
import { useEffect, useRef, useState } from 'react'
import type { CellRun } from '@/rpc/bindings/CellRun'
import { ended } from './store'

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

/**
 * The gutter mark of one cell. `run(line, start)` presses `line` (`start` sends it) unless that line's run is in
 * flight; returns whether it pressed.
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
  const run = (line: number, start: () => Promise<CellRun> | undefined): boolean => {
    if (busy(now.current, line)) return false
    update((m) => pressed(m, line))
    start()?.then(
      (r) => update((m) => settled(started(m, line, r), last.current)),
      () => update((m) => refused(m, line)),
    )
    return true
  }
  return { mark, run }
}
