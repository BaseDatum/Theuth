// The local startup screen's state (25 §4.3, 18): what the screen can honestly know — the shell has the connection
// (`started`, with `owned` once asked), the socket's state, `info()` answered — into the title and the three steps of
// the shutdown card's look. Pure: `StartupScreen.tsx` feeds it, the tests drive it.

import type { ConnState } from '@/rpc/transport'
import type { LaunchStep } from './LaunchCard'

/** The engine step still active this long (a login shell rc waiting for input, 18 §4): the launch log opens itself. */
export const STUCK_MS = 8_000

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
}

export type StartupView = {
  title: string
  failed: boolean
  steps: LaunchStep[]
  /** Relaunch is offered: the start failed or the engine went away (as before the steps). */
  relaunch: boolean
  /** The launch log's disclosure opens by itself (until the user toggles it). */
  logOpen: boolean
}

// CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ST`), two-byte escapes, then any other C0 control or DEL.
const ESCAPES = /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)?|\u001b[@-_]|[\u0000-\u0008\u000b-\u001f\u007f]/g

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

export function startupView(i: StartupInput): StartupView {
  const open = i.started && i.state === 'open'
  const engine: LaunchStep = i.started
    ? { id: 'engine', label: 'Starting the engine', status: 'done', detail: i.owned === false ? 'Using the engine that is already running' : undefined }
    : { id: 'engine', label: 'Starting the engine', status: 'active', detail: lastLogLine(i.log), clip: true }
  const connecting: LaunchStep = { id: 'connecting', label: 'Connecting', status: !i.started ? 'pending' : open ? 'done' : 'active' }
  const loading: LaunchStep = { id: 'loading', label: 'Loading', status: !open ? 'pending' : i.info ? 'done' : 'active' }
  const steps = [engine, connecting, loading]
  const failed = i.error !== undefined
  if (failed) {
    // The step that was active fails with the error; a failure before the shell answered is the engine's.
    const at = Math.max(0, steps.findIndex((s) => s.status === 'active'))
    steps[at] = { ...steps[at]!, status: 'failed', detail: i.error, clip: false }
  }
  const reconnecting = i.started && i.state === 'reconnecting' && i.info
  return {
    title: failed ? 'theuth could not start' : reconnecting ? 'Reconnecting to theuth' : 'Starting theuth',
    failed,
    steps,
    relaunch: failed || i.state === 'reconnecting',
    logOpen: failed || i.waited >= STUCK_MS,
  }
}
