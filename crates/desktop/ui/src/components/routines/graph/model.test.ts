import { receiveComponent } from '@/rpc/bindings/receive'
import { expect, test } from 'vitest'
import type { RunInfo, ScheduleFile, ScheduleInfo, SensorInfo } from '@/rpc/bindings/method.schedule.output'
import { type GraphNode, type RoutineGraph, routineGraph } from './model'

const NOW = Date.parse('2026-11-24T12:00:00Z')
const iso = (minsFromNow: number) => new Date(NOW + minsFromNow * 60_000).toISOString()

const info = (o: Partial<ScheduleInfo>): ScheduleInfo =>
  receiveComponent('method.schedule.output.ScheduleInfo', {
    name: 'x',
    when: 'cron 0 9 * * 1-5',
    describe: '',
    enabled: true,
    quiet: false,
    next: null,
    last: null,
    last_result: null,
    session: null,
    running: false,
    error: null,
    until: null,
    max_runs: null,
    runs: 0,
    approvals: true,
    cwd: null,
    agent: null,
    project: null,
    when_sensor: null,
    fire: null,
    mode: null,
    memory: null,
    expires: null,
    last_check: null,
    for: null,
    cooldown: null,
    timeout: null,
    model: null,
    run_timeout: null,
    created: null,
    sensor: null,
    ...o,
  })

const file = (o: Partial<ScheduleFile>): ScheduleFile =>
  receiveComponent('method.schedule.output.ScheduleFile', {
    name: 'x',
    cron: null,
    every: null,
    at: null,
    quiet: false,
    missed_run: false,
    model: null,
    enabled: true,
    until: null,
    max_runs: null,
    cwd: null,
    approvals: true,
    agent: null,
    project: null,
    when: null,
    fire: null,
    for: null,
    cooldown: null,
    mode: null,
    memory: null,
    expires: null,
    timeout: null,
    created: null,
    run_timeout: null,
    runtime: null,
    budget: null,
    cap: null,
    effort: null,
    engine: null,
    prompt: 'do it',
    ...o,
  })

const run = (mins: number, o: Partial<RunInfo>): RunInfo =>
  receiveComponent('method.schedule.output.RunInfo', { session: null, fired_at: iso(-mins), finished_at: iso(-mins + 1), result: 'end_turn', report: null, envelope: null, kind: 'run', ...o })

const PR_SENSOR = { name: 'pr_watch.py', description: 'A pull request moved.', usage: null, args: [{ name: 'repo', required: true, example: 'owner/name', help: '' }], version: '2', builtin: true, edited: false, shipped_version: '2', needs_cpython: false, cpython_available: true, hosts: [], has_probe: true, error: null } as SensorInfo

const node = (g: RoutineGraph, id: string): GraphNode => {
  const n = g.nodes.find((x) => x.id === id)
  if (!n) throw new Error(`no ${id} card`)
  return n
}
const rowText = (g: RoutineGraph, id: string) => node(g, id).data.rows.map((r) => `${r.label}: ${r.text}`)
const ids = (g: RoutineGraph) => g.nodes.map((n) => n.id).sort()
const edgeIds = (g: RoutineGraph) => g.edges.map((e) => `${e.source}>${e.target}:${e.kind}`).sort()

/** No two cards share any area: the hand layout's one hard promise. */
function expectNoOverlap(g: RoutineGraph) {
  for (const a of g.nodes)
    for (const b of g.nodes) {
      if (a === b) continue
      const apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
      expect(apart, `${a.id} overlaps ${b.id}`).toBe(true)
    }
}

test('clock: the main row, the prompt over Run, a loop-back under the row, no stop cards', () => {
  const g = routineGraph(
    info({ when: 'cron 0 9 * * 1-5', quiet: true, model: 'm', next: iso(14 * 60), session: 'ses-1', last: iso(-600), last_result: 'end_turn' }),
    file({ cron: '0 9 * * 1-5', quiet: true, missed_run: true, model: 'm', effort: 'high', cap: 'read_only', budget: '0.50 USD' }),
    [run(600, { session: 'ses-1' })],
    [],
    { now: NOW },
  )
  expect(ids(g)).toEqual(['deliver', 'gate', 'history', 'prompt', 'run', 'trigger'])
  expect(edgeIds(g)).toEqual(['deliver>trigger:loop', 'gate>run:flow', 'prompt>run:attach', 'run>deliver:flow', 'trigger>gate:flow'])
  expect(rowText(g, 'trigger')).toEqual(['cron: 0 9 * * 1-5', ': weekdays at 09:00', 'missed: runs at boot if missed', 'next: in 14h', 'by hand: Run now'])
  expect(node(g, 'trigger').data.badge).toBe('Clock')
  expect(rowText(g, 'gate')).toContain('cap: read_only')
  expect(rowText(g, 'gate')).toContain('budget: 0.50 USD per run')
  expect(rowText(g, 'run')).toContain('model: m · high')
  expect(node(g, 'run').data.rows[0]).toMatchObject({ label: 'session', session: 'ses-1' })
  expect(rowText(g, 'deliver')[0]).toBe('quiet: only message_user speaks')
  // Prompt sits above Run, the lane below every main-row card, History beside the row's end.
  const loop = g.edges.find((e) => e.kind === 'loop')!
  const row = g.nodes.filter((n) => ['trigger', 'gate', 'run', 'deliver'].includes(n.id))
  expect(node(g, 'prompt').y + node(g, 'prompt').h).toBeLessThan(node(g, 'run').y)
  expect(node(g, 'prompt').x).toBe(node(g, 'run').x)
  expect(loop.lane).toBeGreaterThan(Math.max(...row.map((n) => n.y + n.h)))
  expect(node(g, 'history').x).toBeGreaterThan(node(g, 'deliver').x + node(g, 'deliver').w)
  expect(node(g, 'history').y).toBe(node(g, 'run').y)
  expect(new Set(row.map((n) => n.y)).size).toBe(1)
  expectNoOverlap(g)
})

test('once: End follows Deliver on the row and nothing loops back', () => {
  const g = routineGraph(info({ when: 'at 2026-12-01T15:00', next: iso(60) }), file({ at: '2026-12-01T15:00' }), [], undefined, { now: NOW })
  expect(edgeIds(g)).toEqual(['deliver>end:flow', 'gate>run:flow', 'prompt>run:attach', 'run>deliver:flow', 'trigger>gate:flow'])
  expect(node(g, 'end').y).toBe(node(g, 'deliver').y)
  expect(rowText(g, 'end')).toEqual(['after: the one fire'])
  expect(node(g, 'history').data.runs).toEqual([])
  expectNoOverlap(g)
})

test('watcher: sensor and args on Trigger, the token as a vault name, End over Gate for max_runs/expires', () => {
  const g = routineGraph(
    info({ when: 'every 60s', when_sensor: 'pr_watch.py repo=o/r token=gh', sensor: 'pr_watch.py', fire: 'on_change', mode: 'queue', memory: 'clear', max_runs: 40, runs: 3, expires: '14d', created: iso(-2 * 24 * 60) }),
    file({ every: '60s', when: 'pr_watch.py repo=o/r token=gh', fire: 'on_change', mode: 'queue', cooldown: '5m', for: '0s', max_runs: 40, expires: '14d' }),
    [run(1, { kind: 'check', result: 'quiet', finished_at: null }), run(2, { kind: 'check', result: 'unchanged' }), run(3, { kind: 'run', session: 's' })],
    [PR_SENSOR],
    { now: NOW },
  )
  const trigger = rowText(g, 'trigger')
  expect(trigger.slice(0, 3)).toEqual(['sensor: pr_watch.py', 'repo: o/r', 'token: vault: gh'])
  expect(trigger).toContain('cooldown: 5m between fires')
  expect(trigger.some((r) => r.startsWith('for:'))).toBe(false)
  expect(node(g, 'trigger').data.danger).toBe(false)
  expect(rowText(g, 'gate')).toContain('busy: queue — waits for the run')
  expect(rowText(g, 'gate')).toContain('expires: 12d 0h left of 14d')
  expect(rowText(g, 'run')).toContain('memory: clear — fresh each fire')
  expect(g.edges.find((e) => e.kind === 'stop')).toMatchObject({ source: 'gate', target: 'end', label: 'max_runs · expired' })
  expect(node(g, 'end').x).toBe(node(g, 'gate').x)
  expect(node(g, 'end').y + node(g, 'end').h).toBeLessThan(node(g, 'gate').y)
  // Consecutive checks fold into one History line.
  expect(node(g, 'history').data.runs?.map((r) => [r.what, r.label])).toEqual([
    ['2 checks', 'quiet, unchanged'],
    ['run', 'ok'],
  ])
  expectNoOverlap(g)
})

test('watcher whose sensor is not installed is a danger card', () => {
  const g = routineGraph(info({ when: 'every 60s', when_sensor: 'gone.py', sensor: 'gone.py', expires: '1d' }), file({ every: '60s', when: 'gone.py', expires: '1d' }), [], [PR_SENSOR], { now: NOW })
  expect(node(g, 'trigger').data.danger).toBe(true)
  expect(rowText(g, 'trigger')).toContain(': sensor not installed')
})

test('loop with an agent: Until check on the row, End over it, task runs link the task', () => {
  const g = routineGraph(
    info({ when: 'every 0s', until: './check.sh', max_runs: 50, runs: 12, agent: 'bob', project: 'web', last: iso(-4), last_result: 'task 812' }),
    file({ every: '0s', until: './check.sh', max_runs: 50, agent: 'bob', project: 'web', prompt: Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n') }),
    [...Array.from({ length: 9 }, (_, i) => run(4 + 10 * i, { result: `task ${812 - i}` })), run(200, { kind: 'end', result: 'until broken: exit 2: boom', finished_at: null })],
    undefined,
    { now: NOW },
  )
  expect(node(g, 'trigger').data.badge).toBe('Loop')
  expect(edgeIds(g)).toEqual(['deliver>trigger:loop', 'gate>until:flow', 'prompt>run:attach', 'run>deliver:flow', 'trigger>gate:flow', 'until>end:stop', 'until>run:flow'])
  expect(g.edges.find((e) => e.id === 'until-run')?.label).toBe('exit 1')
  expect(g.edges.find((e) => e.kind === 'stop')?.label).toBe('exit 0 · max_runs')
  expect(g.edges.find((e) => e.kind === 'loop')?.label).toBe('again at once')
  expect(rowText(g, 'until')).toContain('count: 12 / 50 runs')
  expect(node(g, 'until').data.rows.at(-1)).toMatchObject({ label: 'last end', tone: 'danger' })
  expect(rowText(g, 'run').slice(0, 2)).toEqual(['creates: a task for bob', 'project: web'])
  expect(node(g, 'run').data.badge).toBe('task')
  expect(rowText(g, 'deliver')).toEqual(['result: on the task, by bob'])
  // Eight History lines at most; an agent's `task N` row is ok and names its task.
  const runs = node(g, 'history').data.runs!
  expect(runs).toHaveLength(8)
  expect(runs[0]).toMatchObject({ result: 'ok', label: 'task 812', task: '812', session: null })
  expect(rowText(g, 'history')[0]).toBe('last: 4m ago · task 812')
  // A long prompt shows twelve lines and keeps the full text for "Show all".
  expect(node(g, 'prompt').data.lines).toHaveLength(12)
  expect(node(g, 'prompt').data.full?.split('\n')).toHaveLength(20)
  expectNoOverlap(g)
})

test('a broken routine says so on Trigger, Gate and History, and nothing loops back', () => {
  const g = routineGraph(info({ when: 'cron nope', error: 'cron: invalid expression' }), file({ cron: 'nope' }), [], undefined, { now: NOW })
  expect(node(g, 'trigger').data.danger).toBe(true)
  expect(node(g, 'trigger').data.rows[0]).toMatchObject({ label: 'broken', tone: 'danger' })
  expect(node(g, 'gate').data.rows[0]).toMatchObject({ text: 'broken — never fires', tone: 'danger' })
  expect(node(g, 'history').data.rows[0]).toMatchObject({ label: 'error', tone: 'danger' })
  expect(g.edges.some((e) => e.kind === 'loop')).toBe(false)
})

test('paused routine on another machine', () => {
  const g = routineGraph(info({ enabled: false, engine: 'eng-b', approvals: false }), file({ enabled: false, engine: 'eng-b', approvals: false }), [], undefined, { now: NOW, machine: (id) => (id === 'eng-b' ? 'studio' : id) })
  expect(rowText(g, 'gate').slice(0, 2)).toEqual(['state: paused', 'machine: studio'])
  expect(node(g, 'gate').data.rows.find((r) => r.label === 'approvals')).toMatchObject({ text: 'off — never prompts', tone: 'warn' })
  expect(rowText(g, 'trigger')).toContain('next: paused')
})
