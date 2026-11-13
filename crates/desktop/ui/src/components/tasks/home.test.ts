import { expect, onTestFinished, test, vi } from 'vitest'
import type { AgentFile } from "@/rpc/bindings/method.roster.output"
import type { EngineAgent } from "@/rpc/bindings/method.roster.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { Task } from "@/rpc/bindings/method.roster.output"
import { agent as agentOwner, USER } from './helpers'
import { agentWhere, agentsCount, askSecs, clock, engines, homeChip, homeEngine, homeLine, homeTargets, isMuted, itemHome, machineUnknown, movedToast, mutedTitle, offlineCallout, onMachine, openOn, remoteAgents, routineMachines } from './home'

const now = Date.UTC(2026, 10, 8, 12, 0)
const peer = (id: string, name: string, reachable: boolean, last_seen_ms: number) =>
  ({ id, name, addrs: [], reachable, last_seen_ms, hub_pref: 'eligible' }) as unknown as MeshStatus['peers'][number]
const mesh = { self_id: 'A', self_name: 'A', peers: [peer('X', 'xeus', true, now), peer('M', 'mini', false, now - 5 * 60_000)] }
const task = (t: Partial<Task>): Task =>
  ({ id: 't1', title: 't', owner: USER, state: 'todo', home: 'A', checkout: null, blockers: [] , ...t }) as unknown as Task
const file = (name: string, extra: Partial<AgentFile> = {}): AgentFile => ({ name, title: name, paused: false, proposed_by: null, ...extra }) as unknown as AgentFile
const roster: EngineAgent[] = [
  { engine: 'A', file: file('theuth') },
  { engine: 'X', file: file('theuth') },
  { engine: 'A', file: file('hans') },
  { engine: 'X', file: file('ops', { paused: true }) },
  { engine: 'M', file: file('ops') },
]

test('board chip: `on <name>`, none for a legacy row; foreign homes are muted; machine filter by home', () => {
  expect(homeChip(task({ home: 'X' }), mesh)).toBe('on xeus')
  expect(homeChip(task({ home: 'A' }), mesh)).toBe('on A')
  expect(homeChip(task({ home: 'Z0123456789' }), mesh)).toBe('on an unknown machine')
  expect(homeChip(task({ home: 'X' }), undefined), 'nothing while the mesh loads').toBeNull()
  expect(homeEngine(task({ home: 'X' }), undefined)).toBeNull()
  expect(engines({ ...mesh, peers: [peer('N', '', true, now)] })[1]?.name, 'a nameless peer is never a raw id').toBe('another machine')
  expect(homeEngine(task({ home: 'X' }), mesh)).toEqual({ name: 'xeus', here: false, online: true })
  expect(homeEngine(task({ home: 'M' }), mesh)?.online).toBe(false)
  expect(mutedTitle(task({ home: 'X' }), mesh)).toBe('Homed on xeus — read-only here')
  expect(mutedTitle(task({ home: 'A' }), mesh)).toBeUndefined()
  expect(homeChip(task({ home: null }), mesh)).toBeNull()
  expect(isMuted(task({ home: 'X' }), mesh)).toBe(true)
  expect(isMuted(task({ home: 'A' }), mesh)).toBe(false)
  expect(isMuted(task({ home: null }), mesh)).toBe(false)
  const ts = [task({ home: 'A' }), task({ home: 'X' }), task({ home: null })]
  expect(ts.filter((t) => onMachine(t, undefined))).toHaveLength(3)
  expect(ts.filter((t) => onMachine(t, 'X'))).toEqual([ts[1]])
  expect(engines(mesh).map((e) => e.name)).toEqual(['A', 'xeus', 'mini'])
})

test('Move to ▸: self first, current home excluded, enabled only online + owner agent there', () => {
  const theuth = homeTargets(task({ owner: agentOwner('theuth'), home: 'X' }), mesh, roster, [], now)
  expect(theuth.map((r) => [r.name, r.enabled, r.detail])).toEqual([
    ['A', true, ''],
    ['mini', false, 'offline 5m'],
  ])
  const hans = homeTargets(task({ owner: agentOwner('hans') }), mesh, roster, [], now)
  expect(hans.map((r) => [r.name, r.enabled, r.detail])).toEqual([
    ['xeus', false, 'no hans agent there'],
    ['mini', false, 'offline 5m'],
  ])
  // theuth owns it and A is home: xeus takes it.
  expect(homeTargets(task({ owner: agentOwner('theuth') }), mesh, roster, [], now)[0]).toMatchObject({ name: 'xeus', enabled: true })
  // user-owned: no agent needed; ops exists on X and M only.
  expect(homeTargets(task({ owner: USER }), mesh, roster, [], now).map((r) => r.enabled)).toEqual([true, false])
  expect(homeTargets(task({ owner: agentOwner('ops') }), mesh, roster, [], now).map((r) => [r.name, r.enabled])).toEqual([
    ['xeus', true],
    ['mini', false],
  ])
})

test('Move to ▸: "Its session moves too" only for a live, owned-here checkout that is no desk or harness session; none for a done task', () => {
  const co = (session: string, engine = 'A', orphaned = false) => ({ session, engine, epoch: 1, progress_at_ms: 0, orphaned, taken: false }) as Task['checkout']
  const sub = (t: Task, fixed: string[] = []) => homeTargets(t, mesh, roster, fixed, now)[0]?.detail
  expect(sub(task({ checkout: co('s1') }))).toBe('Its session moves too')
  expect(sub(task({ checkout: co('desk1') }), ['desk1'])).toBe('')
  expect(sub(task({ checkout: co('harness1') }), ['desk1', 'harness1'])).toBe('')
  expect(sub(task({ checkout: co('s1', 'A', true) }))).toBe('')
  // checked out on another machine: this engine cannot carry it.
  expect(sub(task({ checkout: co('s1', 'X'), home: 'X' }))).toBe('')
  // the subtitle goes on its own line; only the missing agent follows the name.
  expect(homeTargets(task({ checkout: co('s1') }), mesh, roster, [], now)[0]?.inline).toBe(false)
  expect(homeTargets(task({ owner: agentOwner('hans') }), mesh, roster, [], now)[0]?.inline).toBe(true)
  expect(homeTargets(task({ state: 'done' }), mesh, roster, [], now)).toEqual([])
  expect(homeTargets(task({ state: 'cancelled' }), mesh, roster, [], now)).toEqual([])
})

test('offline home: Home line, `<name> is offline (since HH:MM)`, callout names the machine; online home has none', () => {
  // The same day as "last seen", so the line reads a bare HH:MM whatever today is.
  vi.useFakeTimers({ toFake: ['Date'] })
  vi.setSystemTime(new Date(2026, 10, 8, 22, 0))
  onTestFinished(() => {
    vi.useRealTimers()
  })
  const seen = new Date(2026, 10, 8, 21, 40).getTime()
  const m = { ...mesh, peers: [peer('M', 'mini', false, seen), peer('X', 'xeus', true, now)] }
  const off = homeLine(task({ owner: agentOwner('hans'), home: 'M' }), m)
  expect(off?.text).toBe('Home: hans on mini')
  expect(off?.offline).toBe('mini is offline (since 21:40)')
  expect(off?.callout).toBe(offlineCallout('mini'))
  expect(off?.callout).toContain('Nothing runs elsewhere while mini is off.')
  // a finished task keeps its Home line but loses the offline line and the callout.
  expect(homeLine(task({ owner: agentOwner('hans'), home: 'M', state: 'done' }), m)).toEqual({ text: 'Home: hans on mini', offline: null, callout: null })
  expect(homeLine(task({ home: 'M', state: 'cancelled' }), m)?.callout).toBeNull()
  // the wait comes from the engine's setting; 5 minutes by default.
  expect(off?.callout).toContain('after 5 minutes')
  expect(homeLine(task({ home: 'M' }), m, 600)?.callout).toContain('after 10 minutes')
  expect(askSecs({ home_offline_ask_secs: 120 })).toBe(120)
  expect(askSecs({})).toBe(300)
  expect(askSecs(null)).toBe(300)
  // an unknown home is never a raw id, and it is offline for this engine: the line and the callout.
  const ghost = homeLine(task({ home: 'deadbeefdeadbeef' }), m)
  expect(ghost?.text).toBe('Home: you on an unknown machine')
  expect(ghost?.offline).toBe('an unknown machine is offline')
  expect(ghost?.callout).toContain('an unknown machine is off')
  expect(homeLine(task({ home: 'X' }), undefined), 'nothing while the mesh loads').toBeNull()
  expect(homeLine(task({ owner: USER, home: 'X' }), m)).toEqual({ text: 'Home: you on xeus', offline: null, callout: null })
  expect(homeLine(task({ home: null }), m)).toBeNull()
})

test('Agents page: theuth on two engines is two rows, hans only on A; remote status and open counts', () => {
  const tasks = [
    task({ owner: agentOwner('theuth'), home: 'X', state: 'todo' }),
    task({ owner: agentOwner('theuth'), home: 'A', state: 'todo' }),
    task({ owner: agentOwner('theuth'), home: 'X', state: 'done' }),
    task({ owner: agentOwner('ops'), home: 'X', state: 'doing' }),
  ]
  const rows = remoteAgents(roster, mesh, tasks)
  expect(rows.map((r) => [r.key, r.engineName, r.status, r.open, r.subtitle])).toEqual([
    ['X/theuth', 'xeus', 'idle', 1, 'built-in · on every machine'],
    ['X/ops', 'xeus', 'paused', 1, 'agents/ops.md · on xeus, mini'],
    ['M/ops', 'mini', 'offline', 0, 'agents/ops.md · on xeus, mini'],
  ])
  expect(agentWhere('hans', roster, mesh)).toBe('agents/hans.md · only on A')
  expect(agentWhere('theuth', roster, mesh)).toBe('built-in · on every machine')
  expect(agentWhere('solo', [], mesh)).toBe('agents/solo.md · only on A')
})

test('offline since: HH:MM within a day, date and time after', () => {
  const at = new Date(2026, 10, 7, 21, 40).getTime()
  expect(clock(at, at + 3_600_000)).toBe('21:40')
  expect(clock(at, at + 25 * 3_600_000)).toBe('Nov 7, 21:40')
})

test('attention toast: title says what moved, refusals go in the description; nothing waiting is no error', () => {
  expect(movedToast({ moved: ['a', 'b'], refused: [] }, 'B')).toEqual({ title: 'Moved 2 tasks to B', description: undefined, error: false })
  expect(movedToast({ moved: ['a'], refused: ['Notes — no hans agent here', 'X — done'] }, 'B')).toEqual({ title: 'Moved 1 task to B', description: 'Notes — no hans agent here\nX — done', error: false })
  expect(movedToast({ moved: [], refused: ['x'] }, 'B')).toEqual({ title: 'Nothing moved to B', description: 'x', error: true })
  expect(movedToast({ moved: [], refused: [] }, 'B')).toEqual({ title: 'No tasks were waiting', error: false })
})

test('Agents counts by home: a task homed on xeus counts there, not on A; legacy rows count here; header counts rows and machines', () => {
  const ts = [task({ owner: agentOwner('hans'), home: 'A' }), task({ owner: agentOwner('hans'), home: 'X' }), task({ owner: agentOwner('hans'), home: null }), task({ owner: agentOwner('hans'), home: 'A', state: 'done' })]
  expect(openOn(ts, mesh, 'A', 'hans')).toBe(2)
  expect(openOn(ts, mesh, 'X', 'hans')).toBe(1)
  expect(agentsCount(3, 2)).toBe('3 agents on 2 machines')
  expect(agentsCount(1, 1)).toBe('1 agent')
  expect(agentsCount(2, 1)).toBe('2 agents')
})

test('attention: "on <machine>" for an item whose task is homed elsewhere', () => {
  const ts = [task({ id: 'a', home: 'X' }), task({ id: 'b', home: 'A' })]
  expect(itemHome('a', ts, mesh)).toBe('on xeus')
  expect(itemHome('b', ts, mesh)).toBeNull()
  expect(itemHome(null, ts, mesh)).toBeNull()
  expect(itemHome('zz', ts, mesh)).toBeNull()
})

test('routines: machine options and the unknown-machine chip', () => {
  expect(routineMachines(mesh).map((o) => o.label)).toEqual(['A (this machine)', 'xeus', 'mini'])
  expect(routineMachines(mesh, 'ghost').at(-1)).toEqual({ value: 'ghost', label: 'machine unknown' })
  expect(routineMachines(mesh, 'X')).toHaveLength(3)
  expect(machineUnknown('ghost', mesh)).toBe(true)
  expect(machineUnknown('X', mesh)).toBe(false)
  expect(machineUnknown(undefined, mesh)).toBe(false)
  expect(machineUnknown('ghost', undefined)).toBe(false)
})
