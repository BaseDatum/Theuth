import { expect, test } from 'vitest'
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import { moveBlocked, moveTargets, settingsRows } from './SessionMenu'

const now = Date.UTC(2026, 10, 7, 17, 20)
const peer = (id: string, name: string, reachable: boolean, last_seen_ms: number) =>
  ({ id, name, addrs: [], reachable, last_seen_ms, hub_pref: 'eligible' }) as unknown as MeshStatus['peers'][number]
const mesh = {
  self_id: 'me',
  peers: [peer('x1', 'xeus', true, now), peer('me', 'this mac', true, now), peer('l1', "Sky's laptop", false, now - 3 * 3_600_000), peer('n1', '', false, 0)],
}

test('181 D1: Move to ▸ lists every paired engine but this one; offline is disabled with its age; running jobs stay here', () => {
  expect(moveTargets(mesh, 0, now)).toEqual([
    { id: 'x1', name: 'xeus', online: true, detail: '' },
    { id: 'l1', name: "Sky's laptop", online: false, detail: 'offline 3h' },
    { id: 'n1', name: 'n1', online: false, detail: 'offline' },
  ])
  expect(moveTargets(mesh, 1, now)[0]?.detail).toBe('1 running job stays here')
  expect(moveTargets(mesh, 2, now).map((t) => t.detail)).toEqual(['2 running jobs stay here', 'offline 3h', 'offline'])
  // D6: no paired engines, no submenu.
  expect(moveTargets({ self_id: 'me', peers: [] }, 3, now)).toEqual([])
  expect(moveTargets(undefined, 0, now)).toEqual([])
})

test('181 fix round: a paused agent’s tree, a desk and a harness session say why they cannot move', () => {
  const rows = [
    { id: 'desk', parent: null, agent: null, desk: 'ops' },
    { id: 'child', parent: 'desk', agent: null, desk: null },
    { id: 'grand', parent: 'child', agent: null, desk: null },
    { id: 'acp', parent: null, agent: 'omp', desk: null },
    { id: 'chat', parent: null, agent: null, desk: null },
  ]
  const paused = [{ status: 'paused' as const, desk: 'desk', file: { name: 'ops' } }] as never[]
  const row = (id: string) => rows.find((r) => r.id === id)
  for (const id of ['desk', 'child', 'grand']) expect(moveBlocked(row(id), rows, paused)).toBe('ops is paused — un-pause it before moving')
  expect(moveBlocked(row('desk'), rows, [])).toBe("ops's desk stays on the engine that runs its agent")
  expect(moveBlocked(row('child'), rows, [])).toBeNull()
  expect(moveBlocked(row('acp'), rows, paused)).toBe('runs the omp harness as a process here, so it cannot move')
  expect(moveBlocked(row('chat'), rows, paused)).toBeNull()
  expect(moveBlocked(undefined, rows, paused)).toBeNull()
})

test('183 §12.2 / mock panel 3: a remote session\'s ⋯ menu — settings on its owner, then Continue here and Move to', () => {
  const rows = settingsRows({ owner: 'cloud-1', self: "Sky's MacBook", offline: false, approvals: { global: false, pin: true }, messaging: { effective: true }, model: 'Opus 5.5', effort: 'high' })
  expect(rows.map((r) => r.label)).toEqual([
    'Settings for this session on cloud-1',
    'Approvals: Ask before risky tools',
    'Agent messaging',
    // 2026-11-11: the effort rides on the model row; the picker it opens carries the effort chips (no Effort › submenu).
    'Model: Opus 5.5 · high',
    '',
    "Continue here (move to Sky's MacBook)",
    'Move to',
  ])
  expect(rows.find((r) => r.id === 'approvals')).toMatchObject({ value: 'on', disabled: false, options: [{ label: 'On' }, { label: 'Off' }, { label: 'cloud-1 default (off)' }] })
  expect(rows.find((r) => r.id === 'messaging')).toMatchObject({ checked: true, disabled: false })
  expect(rows.find((r) => r.id === 'model')).toMatchObject({ disabled: false })
  expect(settingsRows({ self: 'mac', offline: false, model: 'Opus 5.5', effort: null }).find((r) => r.id === 'model')?.label).toBe('Model: Opus 5.5')
  expect(rows.find((r) => r.id === 'continue')).toMatchObject({ disabled: false })
  // §4.4 / edge 13: owner offline — the line says so and every setter greys (cached readers too); Continue here stays.
  const off = settingsRows({ owner: 'cloud-1', self: 'mac', offline: true, approvals: { global: true, pin: true }, messaging: { effective: true }, model: 'm', effort: 'high' })
  expect(off[1]).toEqual({ id: 'offline', label: 'cloud-1 is offline · Continue here' })
  expect(off.filter((r) => 'disabled' in r && r.disabled).map((r) => r.id)).toEqual(['approvals', 'messaging', 'model'])
  expect(off.find((r) => r.id === 'continue')).toMatchObject({ disabled: false })
})

test('183 §12.2: a session owned here gets the same settings, no header, and Continue here greyed', () => {
  const rows = settingsRows({ self: 'mac', offline: true, approvals: { global: true }, messaging: { effective: false }, model: 'm' })
  expect(rows.map((r) => r.id)).toEqual(['approvals', 'messaging', 'model', 'separator', 'continue', 'move'])
  expect(rows.find((r) => r.id === 'approvals')).toMatchObject({ value: 'default', disabled: false, options: [{}, {}, { label: 'Machine default (on)' }] })
  expect(rows.find((r) => r.id === 'continue')).toEqual({ id: 'continue', label: 'Continue here', disabled: true })
  // W-4 tri-state from the row's own pin (E183: `session` is the effective value, ancestors included).
  const value = (approvals: { global: boolean; session: boolean; pin?: boolean }) => settingsRows({ self: 'mac', offline: false, approvals, model: 'm' }).find((r) => r.id === 'approvals')
  expect(value({ global: false, session: true, pin: true })).toMatchObject({ value: 'on' })
  expect(value({ global: true, session: false, pin: false })).toMatchObject({ value: 'off' })
  expect(value({ global: true, session: false })).toMatchObject({ value: 'default' })
  expect(settingsRows({ self: 'mac', offline: false, model: 'm' }).find((r) => r.id === 'approvals')).toMatchObject({ value: undefined, disabled: true })
})

test('183 R1 (U-M1): an engine that sends pin: null (or omits it) means inherit, never Off', () => {
  const row = (approvals: Parameters<typeof settingsRows>[0]['approvals']) => settingsRows({ self: 'mac', offline: false, approvals, model: 'm' }).find((r) => r.id === 'approvals')
  expect(row({ global: true, session: true, pin: null })).toMatchObject({ value: 'default' })
  expect(row({ global: true, session: false, pin: null })).toMatchObject({ value: 'default' })
  expect(row({ global: true, session: true, pin: false })).toMatchObject({ value: 'off' })
})

test('183 §12.2 (R1): the inherit option names whose default it is, with the effective value when the pin is absent', () => {
  const label = (o: Partial<Parameters<typeof settingsRows>[0]>) => (settingsRows({ self: 'mac', offline: false, model: 'm', ...o }).find((r) => r.id === 'approvals') as { options: { label: string }[] }).options[2]!.label
  // An ancestor's pin can make the session's effective value differ from the machine's switch.
  expect(label({ approvals: { global: true, session: false } })).toBe('Machine default (off)')
  expect(label({ owner: 'cloud-1', approvals: { global: false, session: true } })).toBe('cloud-1 default (on)')
  expect(label({ owner: 'cloud-1', approvals: { global: false, session: true, pin: true } })).toBe('cloud-1 default (off)')
})

test('183 R1 (S7): an owner from before 183 greys the approvals row and says it needs an update', () => {
  const row = settingsRows({ owner: 'old-box', self: 'mac', offline: false, oldOwner: true, model: 'm' }).find((r) => r.id === 'approvals')
  expect(row).toMatchObject({ disabled: true, note: 'old-box needs an update' })
})

test('219 §6.1: tools that always ask add one row under Approvals; none adds no row', () => {
  const rows = (alwaysAsk: number) => settingsRows({ self: 'mac', offline: false, approvals: { global: false }, alwaysAsk, model: 'm' }).map((r) => r.id)
  expect(rows(0)).toEqual(['approvals', 'messaging', 'model', 'separator', 'continue', 'move'])
  expect(rows(3)).toEqual(['approvals', 'always-ask', 'messaging', 'model', 'separator', 'continue', 'move'])
  expect(settingsRows({ self: 'mac', offline: false, alwaysAsk: 3, model: 'm' }).find((r) => r.id === 'always-ask')?.label).toBe('Always ask: 3 tools')
  expect(settingsRows({ self: 'mac', offline: false, alwaysAsk: 1, model: 'm' }).find((r) => r.id === 'always-ask')?.label).toBe('Always ask: 1 tool')
})
