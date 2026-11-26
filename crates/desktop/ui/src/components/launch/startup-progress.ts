// The local startup screen's state (25 §4.3, 18, 222): what the screen can honestly know — the shell has the connection
// (`started`, with `owned` once asked), the socket's state, `info()` answered, and the engine's own `startup.json` as
// the shell tails it — into the title and the steps of the shutdown card's look. With a `startup.json` the engine's
// phases are the steps (the finished ones with their duration, the active one with its n/of and a running timer); an
// older engine without one gets the three steps. Pure: `StartupScreen.tsx` feeds it, the tests drive it.

import type { StartupProgress, StartupStatus } from '@/lib/tauri'
import type { ConnState } from '@/rpc/transport'
import type { LaunchStep } from './LaunchCard'

/** The engine step still active this long (a login shell rc waiting for input, 18 §4): the launch log opens itself. */
export const STUCK_MS = 8_000
/** 222: the shell's answer to a plain relaunch while the engine is still making progress: `starting: <label>`. */
export const STARTING = 'starting:'
/** 222: the shell's `stalled` (90 s without progress), in words. */
export const STALLED = 'Taking longer than usual — still working'
/** Finished phases listed one by one; earlier ones fold into one row. */
export const SHOWN_DONE = 6

export type StartupInput = {
  /** The shell answered `connection` (the client exists). */
  started: boolean
  /** `Connection.owned`, once read; `null` before. */
  owned: boolean | null
  state: ConnState
  /** `info()` answered. */
  info: boolean
  error?: string
  /** The pty transcript so far. */
  log: string
  /** How long the screen has waited with the engine step active (ms), as of the last tick; it keeps that value once the
   * engine answers, so a log that opened itself stays open. */
  waited: number
  /** The last `startup-progress` payload; `null` before the first. */
  progress: StartupProgress | null
  /** Wall clock (ms since the epoch) of the last tick: the running timers. */
  now: number
}

export type StartupStep = LaunchStep & {
  /** The active phase's `n` of `of` (a migration, a plugin): the bar. */
  progress?: { n: number; of: number }
}

export type StartupView = {
  title: string
  failed: boolean
  steps: StartupStep[]
  /** Relaunch is offered: the start failed, the engine exited or stalled, or it went away later. */
  relaunch: boolean
  /** The launch log's disclosure opens by itself (until the user toggles it). */
  logOpen: boolean
  /** A plain relaunch was refused because the engine is still at this (`starting: <label>`): ask before forcing it. */
  interrupt?: string
  /** Under the steps: `STALLED`. */
  note?: string
  /** The engine start's total time so far, while it runs. */
  elapsed?: string
}

// CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ST`, never past a line end), other escapes (`ESC` intermediates final:
// `ESC 7`, `ESC ( B`), then any other C0 control but `\t` and `\n` (so a lone `\r` too), and DEL.
const ESCAPES = /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b\n]*(?:\u0007|\u001b\\)?|\u001b[ -/]*[0-~]|[\u0000-\u0008\u000b-\u001f\u007f]/g

/** The launch log as plain text for the log pane: `\r\n` → `\n`, escape sequences and other controls dropped. */
export const cleanLog = (log: string): string => log.replaceAll('\r\n', '\n').replace(ESCAPES, '')

/** The last non-empty line of the launch log, escape sequences stripped; a `\r` redraw keeps only what was drawn last. */
export function lastLogLine(log: string): string | undefined {
  const lines = log.split('\n')
  for (let i = lines.length - 1; i >= 0; i--) {
    const redrawn = lines[i]!.split('\r').map((part) => part.replace(ESCAPES, '').trim())
    const line = redrawn.findLast((part) => part !== '')
    // ponytail: capped at 200 chars, the row ellipsises the rest; the full text is in the log.
    if (line) return line.slice(0, 200)
  }
  return undefined
}

/** A finished phase's duration: `12 ms`, `1.4 s`, `42 s`, `3:07`, `1:02:05`. */
export function formatDuration(ms: number): string {
  if (ms < 1_000) return `${Math.max(0, Math.round(ms))} ms`
  if (ms < 10_000) return `${(Math.floor(ms / 100) / 10).toFixed(1)} s`
  return formatTimer(ms)
}

/** A running timer, whole seconds: `7 s`, `3:07`, `1:02:05`. */
export function formatTimer(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1_000))
  if (s < 60) return `${s} s`
  const pad = (n: number) => String(n).padStart(2, '0')
  const [h, m] = [Math.floor(s / 3_600), Math.floor((s % 3_600) / 60)]
  return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`
}

export function startupView(i: StartupInput): StartupView {
  // A refused plain relaunch is a question, not a failure: the engine is still starting.
  const interrupt = i.error?.startsWith(STARTING) ? i.error.slice(STARTING.length).trim() : undefined
  const error = interrupt === undefined ? i.error : undefined
  const status = i.progress?.status ?? null
  // A reused engine's `startup.json` (if any) is an earlier boot's: nothing to show from it.
  const view = status && i.owned !== false ? phaseView(i, i.progress!, status, error) : threeSteps(i, error)
  return interrupt === undefined ? view : { ...view, interrupt, relaunch: false }
}

/** An older engine (no `startup.json`): starting the engine, connecting, loading. */
function threeSteps(i: StartupInput, error: string | undefined): StartupView {
  const engine: LaunchStep = i.started
    ? { id: 'engine', label: 'Starting the engine', status: 'done', detail: i.owned === false ? 'Using the engine that is already running' : undefined }
    : { id: 'engine', label: 'Starting the engine', status: 'active', detail: lastLogLine(i.log), clip: true }
  const steps = [engine, ...afterEngine(i)]
  const failed = error !== undefined
  if (failed) {
    // The step that was active fails with the error; a failure before the shell answered is the engine's.
    const at = Math.max(0, steps.findIndex((s) => s.status === 'active'))
    steps[at] = { ...steps[at]!, status: 'failed', detail: error, clip: false }
  }
  return {
    title: title(i, failed),
    failed,
    steps,
    relaunch: failed || i.state === 'reconnecting',
    logOpen: failed || i.waited >= STUCK_MS,
  }
}

/** 222: the engine's phases from `startup.json`, then connecting and loading. */
function phaseView(i: StartupInput, p: StartupProgress, s: StartupStatus, error: string | undefined): StartupView {
  const done = s.done ?? []
  const shown = done.slice(-SHOWN_DONE)
  const folded = done.slice(0, done.length - shown.length)
  const steps: StartupStep[] = []
  if (folded.length > 0) {
    const ms = folded.reduce((sum, d) => sum + d.ms, 0)
    steps.push({ id: 'earlier', label: `${folded.length} earlier steps`, status: 'done', meta: formatDuration(ms) })
  }
  shown.forEach((d, k) => steps.push({ id: `done-${folded.length + k}`, label: d.label, status: 'done', meta: formatDuration(d.ms) }))

  // The engine's own part is over once it is ready or the shell answered; until then its phase is the active step.
  const running = !i.started && s.state !== 'ready'
  if (s.state === 'failed') {
    steps.push({ id: 'phase', label: s.label, status: 'failed', detail: s.error ?? error })
  } else if (running) {
    // The phase began when the last one finished: the boot's start plus the finished durations (gaps between phases
    // and a `done` list cut at 200 make it an estimate).
    const since = s.started_ms + done.reduce((sum, d) => sum + d.ms, 0)
    const step = s.step && s.step.of > 0 ? { n: Math.min(s.step.n, s.step.of), of: s.step.of } : undefined
    steps.push({ id: 'phase', label: s.label, status: 'active', meta: formatTimer(i.now - since), progress: step })
  }
  steps.push(...afterEngine(i, s.state === 'ready'))

  const failed = error !== undefined || s.state === 'failed'
  if (error !== undefined && s.state !== 'failed') {
    // The step that was active fails with the error (the engine exited, stalled for good, or the socket failed).
    const at = steps.findIndex((st) => st.status === 'active')
    const k = at >= 0 ? at : steps.length - 1
    steps[k] = { ...steps[k]!, status: 'failed', detail: error, progress: undefined }
  }
  const stalled = running && p.stalled && !failed
  return {
    title: title(i, failed),
    failed,
    steps,
    relaunch: failed || stalled || !p.alive || i.state === 'reconnecting',
    logOpen: failed || stalled,
    note: stalled ? STALLED : undefined,
    elapsed: running && !failed ? formatTimer(i.now - s.started_ms) : undefined,
  }
}

/** Connecting and loading; connecting is active once the shell answered, or (222) once the engine says ready. */
function afterEngine(i: StartupInput, ready = false): LaunchStep[] {
  const open = i.started && i.state === 'open'
  return [
    { id: 'connecting', label: 'Connecting', status: open ? 'done' : i.started || ready ? 'active' : 'pending' },
    { id: 'loading', label: 'Loading', status: !open ? 'pending' : i.info ? 'done' : 'active' },
  ]
}

const title = (i: StartupInput, failed: boolean) =>
  failed ? 'theuth could not start' : i.started && i.state === 'reconnecting' && i.info ? 'Reconnecting to theuth' : 'Starting theuth'
