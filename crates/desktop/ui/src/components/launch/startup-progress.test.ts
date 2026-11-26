import { expect, test } from 'vitest'
import type { StartupProgress, StartupStatus } from '@/lib/tauri'
import { STALLED, STUCK_MS, type StartupInput, cleanLog, formatDuration, formatTimer, lastLogLine, startupView } from './startup-progress'

const BASE: StartupInput = { started: false, owned: null, state: 'connecting', info: false, log: '', waited: 0, progress: null, now: 0 }
const LOG = 'Last login: Tue Nov 17 on ttys004\r\n\u001b]0;theuth\u0007\u001b[1;32mtheuth\u001b[0m 2.31.0\r\nopening store\u001b[K\r\n\r\n'
const view = (over: Partial<StartupInput>) => startupView({ ...BASE, ...over })
type Row = [id: string, status: string, detail: string | undefined]
const rows = (over: Partial<StartupInput>): Row[] => view(over).steps.map((s) => [s.id, s.status, s.detail])

test.each<[string, Partial<StartupInput>, Row[], string, boolean]>([
  ['not started: the engine step, no log yet', {}, [['engine', 'active', undefined], ['connecting', 'pending', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['engine starting: its detail is the last log line', { log: LOG }, [['engine', 'active', 'opening store'], ['connecting', 'pending', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['the shell has the connection, owned not read yet', { started: true, log: LOG }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['reused engine', { started: true, owned: false }, [['engine', 'done', 'Using the engine that is already running'], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['socket connecting, own engine', { started: true, owned: true, log: LOG }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['info pending', { started: true, owned: true, state: 'open' }, [['engine', 'done', undefined], ['connecting', 'done', undefined], ['loading', 'active', undefined]], 'Starting theuth', false],
  ['reconnecting after the engine was up', { started: true, owned: true, state: 'reconnecting', info: true }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Reconnecting to theuth', true],
  ['a first socket retrying is still the start', { started: true, owned: true, state: 'reconnecting' }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', true],
])('%s', (_, over, steps, title, relaunch) => {
  const v = view(over)
  expect(rows(over)).toEqual(steps)
  expect([v.title, v.failed, v.relaunch, v.logOpen]).toEqual([title, false, relaunch, false])
})

test.each<[string, Partial<StartupInput>, string]>([
  ['the engine did not start', { log: LOG }, 'engine'],
  ['the socket failed', { started: true, owned: true }, 'connecting'],
  ['info failed (older engine)', { started: true, owned: true, state: 'open' }, 'loading'],
])('error: %s — the active step fails with the error, the log opens, Relaunch shows', (_, over, failedId) => {
  const v = view({ ...over, error: 'boom' })
  expect(v.title).toBe('theuth could not start')
  expect([v.failed, v.relaunch, v.logOpen]).toEqual([true, true, true])
  const failed = v.steps.filter((s) => s.status === 'failed')
  expect(failed).toMatchObject([{ id: failedId, detail: 'boom', clip: false }])
  // Steps before it stay done, after it pending.
  const at = v.steps.findIndex((s) => s.id === failedId)
  expect(v.steps.slice(0, at).every((s) => s.status === 'done')).toBe(true)
  expect(v.steps.slice(at + 1).every((s) => s.status === 'pending')).toBe(true)
})

test('the launch log opens itself once the engine step has been active for 8 s, and stays open after', () => {
  expect(view({ log: LOG, waited: STUCK_MS - 1 }).logOpen).toBe(false)
  expect(view({ log: LOG, waited: STUCK_MS }).logOpen).toBe(true)
  expect(view({ log: LOG, waited: STUCK_MS, started: true, owned: true }).logOpen).toBe(true)
})

test('the last log line: escapes and controls stripped, a \\r redraw keeps what was drawn last, capped', () => {
  expect(lastLogLine('')).toBeUndefined()
  expect(lastLogLine('\r\n  \r\n\u001b[0m\r\n')).toBeUndefined()
  expect(lastLogLine(LOG)).toBe('opening store')
  expect(lastLogLine('a\nfetching 10%\rfetching 55%\r')).toBe('fetching 55%')
  expect(lastLogLine('\u001b[1;34m[oh-my-zsh]\u001b[0m Would you like to update? [Y/n] ')).toBe('[oh-my-zsh] Would you like to update? [Y/n]')
  expect(lastLogLine('x'.repeat(500))).toHaveLength(200)
})

test('the log pane text: \\r\\n and lone \\r handled, CSI/OSC/two-byte escapes and controls dropped, \\t and \\n kept', () => {
  expect(cleanLog('Last login: Tue Nov 17 11:02:41 on ttys004\r\n\u001b[1;34m[oh-my-zsh]\u001b[0m Would you like to update? [Y/n] ')).toBe(
    'Last login: Tue Nov 17 11:02:41 on ttys004\n[oh-my-zsh] Would you like to update? [Y/n] ',
  )
  expect(cleanLog('\u001b]0;theuth\u0007a\tb\u001b7\u001b8\r\nc\rd\u0000\u007f\u0008\n')).toBe('a\tb\ncd\n')
  // An unterminated OSC stops at the line end instead of eating the rest of the log.
  expect(cleanLog('\u001b]0;title\nnext line')).toBe('\nnext line')
})

// ------------------------------------------------------------------------------------------- 222: `startup.json`

const T0 = 1_764_160_000_000
const DONE = [
  { phase: 'lock', label: 'Taking the lock', ms: 3 },
  { phase: 'config', label: 'Reading config', ms: 12 },
  { phase: 'runtime-init', label: 'Starting the runtime', ms: 1_450 },
]
const status = (over: Partial<StartupStatus> = {}): StartupStatus => ({
  v: 1,
  pid: 4242,
  started_ms: T0,
  seq: 9,
  hb_ms: T0,
  state: 'running',
  phase: 'migrate:agent',
  label: 'Migrating agent.db',
  step: { n: 3, of: 78 },
  done: DONE,
  error: null,
  ...over,
})
const progress = (over: Partial<StartupStatus> = {}, p: Partial<StartupProgress> = {}): StartupProgress => ({
  status: status(over),
  alive: true,
  stalled: false,
  waited_ms: 0,
  ...p,
})
// 7 s into the migration, 8 s into the boot.
const NOW = T0 + 1_465 + 7_000
type PhaseRow = [label: string, status: string, meta: string | undefined]
const phaseRows = (over: Partial<StartupInput>): PhaseRow[] => view({ now: NOW, ...over }).steps.map((s) => [s.label, s.status, s.meta])

test('migrating: the finished phases with durations, the active one with its n/of and timer, the total; no Relaunch', () => {
  const v = view({ now: NOW, progress: progress() })
  expect(phaseRows({ progress: progress() })).toEqual([
    ['Taking the lock', 'done', '3 ms'],
    ['Reading config', 'done', '12 ms'],
    ['Starting the runtime', 'done', '1.4 s'],
    ['Migrating agent.db', 'active', '7 s'],
    ['Connecting', 'pending', undefined],
    ['Loading', 'pending', undefined],
  ])
  expect(v.steps[3]!.progress).toEqual({ n: 3, of: 78 })
  expect([v.title, v.failed, v.relaunch, v.logOpen, v.elapsed, v.note]).toEqual(['Starting theuth', false, false, false, '8 s', undefined])
})

test('a long wait without stalling does not open the log (the phases say what happens)', () => {
  expect(view({ now: NOW, progress: progress(), waited: 10 * STUCK_MS }).logOpen).toBe(false)
})

test('a phase without a step has no bar; an unknown state reads as running', () => {
  const v = view({ now: NOW, progress: progress({ phase: 'plugin:mesh', label: 'Starting mesh', step: null, state: 'warming' }) })
  expect(v.steps[3]).toMatchObject({ label: 'Starting mesh', status: 'active', progress: undefined })
})

test('the active timer counts from `phase_ms` when the engine writes it (the estimate ignores gaps between phases)', () => {
  // The estimate says 7 s; the engine says the phase began 3 s ago (4 s went between phases).
  expect(view({ now: NOW, progress: progress({ phase_ms: NOW - 3_000 }) }).steps[3]!.meta).toBe('3 s')
  expect(view({ now: NOW, progress: progress({ phase_ms: null }) }).steps[3]!.meta).toBe('7 s')
})

test('more than six finished phases: the earlier ones fold into one row with their total', () => {
  const done = Array.from({ length: 10 }, (_, k) => ({ phase: `plugin:p${k}`, label: `Plugin ${k}`, ms: 100 * (k + 1) }))
  const rows = phaseRows({ progress: progress({ done }), now: T0 + 60_000 })
  expect(rows.slice(0, 7)).toEqual([
    ['4 earlier steps', 'done', '1.0 s'],
    ['Plugin 4', 'done', '500 ms'],
    ['Plugin 5', 'done', '600 ms'],
    ['Plugin 6', 'done', '700 ms'],
    ['Plugin 7', 'done', '800 ms'],
    ['Plugin 8', 'done', '900 ms'],
    ['Plugin 9', 'done', '1.0 s'],
  ])
})

test('stalled: the note, the log opens, Relaunch offered; still not a failure', () => {
  const v = view({ now: NOW, progress: progress({}, { stalled: true }) })
  expect([v.failed, v.relaunch, v.logOpen, v.note]).toEqual([false, true, true, STALLED])
  expect(v.steps[3]).toMatchObject({ status: 'active' })
})

test('the engine exited before the shell reported it: Relaunch offered', () => {
  expect(view({ now: NOW, progress: progress({}, { alive: false }) }).relaunch).toBe(true)
})

test('startup.json says failed: that phase fails with its error', () => {
  const v = view({ now: NOW, progress: progress({ state: 'failed', error: 'migration 12 of agent.db: disk full' }, { alive: false }) })
  expect(v.title).toBe('theuth could not start')
  expect([v.failed, v.relaunch, v.logOpen, v.elapsed]).toEqual([true, true, true, undefined])
  expect(v.steps[3]).toMatchObject({ label: 'Migrating agent.db', status: 'failed', detail: 'migration 12 of agent.db: disk full' })
})

test('the shell failed the start (exit, hard stall): the active phase fails with its error', () => {
  const v = view({ now: NOW, progress: progress({}, { alive: false }), error: 'The engine exited (code 101)' })
  expect(v.steps[3]).toMatchObject({ label: 'Migrating agent.db', status: 'failed', detail: 'The engine exited (code 101)', progress: undefined })
  expect([v.failed, v.relaunch]).toEqual([true, true])
})

test('ready: the engine phases are done and connecting is active until the socket opens', () => {
  const ready = progress({ state: 'ready', phase: 'ready', label: 'Ready', step: null })
  expect(phaseRows({ progress: ready }).slice(3)).toEqual([
    ['Connecting', 'active', undefined],
    ['Loading', 'pending', undefined],
  ])
  expect(phaseRows({ progress: ready, started: true, owned: true, state: 'open' }).slice(3)).toEqual([
    ['Connecting', 'done', undefined],
    ['Loading', 'active', undefined],
  ])
  expect(view({ progress: ready, started: true, owned: true, state: 'open' }).elapsed).toBeUndefined()
})

test('a reused engine ignores an earlier boot’s startup.json: the three steps', () => {
  expect(rows({ started: true, owned: false, progress: progress({ state: 'ready' }) })[0]).toEqual(['engine', 'done', 'Using the engine that is already running'])
})

test.each<[string, Partial<StartupInput>]>([
  ['with progress', { now: NOW, progress: progress() }],
  ['without (older engine)', {}],
])('a refused relaunch (`starting:`, %s) asks instead of failing', (_, over) => {
  const v = view({ ...over, error: 'starting: Migrating agent.db (3/78)' })
  expect([v.interrupt, v.failed, v.relaunch, v.title]).toEqual(['Migrating agent.db (3/78)', false, false, 'Starting theuth'])
  expect(v.steps.some((s) => s.status === 'failed')).toBe(false)
})

test('durations and timers', () => {
  expect([0, 12, 999, 1_000, 1_449, 9_999, 42_500, 187_000, 3_725_000].map(formatDuration)).toEqual([
    '0 ms', '12 ms', '999 ms', '1.0 s', '1.4 s', '9.9 s', '42 s', '3:07', '1:02:05',
  ])
  expect([-5, 0, 7_900, 60_000, 3_600_000].map(formatTimer)).toEqual(['0 s', '0 s', '7 s', '1:00', '1:00:00'])
})
