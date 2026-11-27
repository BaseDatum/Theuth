import '../test-dom'
import { QueryClient } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { setterField } from '@/components/settings/copy/fields'
import { type Choice, choices, readPending, retryPending, settlePending } from '@/components/settings/copy/set-on-all'
import { SetOnAllSheet } from '@/components/settings/copy/SetOnAllSheet'
import { type SettingsMachines, usePickedMachine } from '@/components/settings/machine-target'
import type { Api } from '@/rpc/bindings/client'
import type { EngineCapsResult, NodeResult } from '@/rpc/bindings/method.agent.output'
import type { Machine } from '@/lib/mesh-machines'
import { LOCAL } from '@/link/engines'

const caps = (c: Partial<EngineCapsResult>): EngineCapsResult => ({ engine: '', name: '', platform: 'linux', display: true, mic: true, interactive_user: true, headless: false, hub: false, ...c })
/** A member of the hub's mesh, reached through the hub row's client (`engine_call`). */
const machine = (id: string, name: string, m: Partial<Machine> = {}): Machine => ({ id, name, kind: 'peer', connected: false, client: 'hub-row', forwarded: true, hub: false, online: true, sealed: false, path: 'direct', ...m })

/** This iPad's engine, a member of the mesh (case a): reached through its own client. */
const ipad = machine('ipad', 'This iPad', { kind: 'local', connected: true, client: LOCAL, forwarded: false, path: null })
const mbp = machine('mbp', '333 · MacBook Pro', { kind: 'hub', hub: true, forwarded: false, path: null })
const p50 = machine('p50', 'P50 · ThinkPad', { caps: caps({ mic: false, display: false }) })
const nyc = machine('nyc', 'cloud-nyc1', { kind: 'cloud', sealed: true })
const xeus = machine('xeus', 'xeus', { online: false, path: null })
const sfo = machine('sfo', 'cloud-sfo2', { kind: 'cloud' })
const box = machine('box', 'box')
const MACHINES = [ipad, mbp, p50, nyc, xeus, sfo, box]

function settingsOf(machines: Machine[], engine_call_all: unknown, local: Record<string, unknown>): SettingsMachines {
  const connected = machines.find((m) => m.connected) ?? null
  return { machines, target: p50, connected, api: { engine_call_all } as unknown as Api, local: local as unknown as Api, meshClient: 'hub-row', qc: new QueryClient(), tab: null, open: () => {} }
}

afterEach(() => {
  document.body.innerHTML = ''
  localStorage.clear()
})

test('227: the sheet ticks every reachable machine, this device included; offline unticked; n/a and hub-only disabled with the reason', () => {
  const byId = (cs: Choice[]) => Object.fromEntries(cs.map((c) => [c.machine.id, [c.ticked, c.disabled, c.note]]))
  expect(byId(choices(MACHINES, { needs: ['mic'] }))).toEqual({
    ipad: [true, null, null],
    mbp: [true, null, null],
    p50: [false, 'n/a: no microphone', null],
    nyc: [true, null, 'vault locked: applies after unlock'],
    xeus: [false, null, 'offline: queued until it reconnects'],
    sfo: [true, null, null],
    box: [true, null, null],
  })
  const hubOnly = byId(choices(MACHINES, { hubOnly: true }))
  expect(hubOnly.mbp).toEqual([true, null, null])
  expect(hubOnly.ipad).toEqual([false, 'hub only', null])
})

test('227: results per machine as the mock: applied, offline/locked pending, skipped, failed, restart chip; Retry pending re-runs only the pending', async () => {
  const answers: NodeResult[] = [
    // The hub answers for this iPad's engine too (it is a member): dropped, its own client's result counts once.
    { engine: 'ipad', name: 'This iPad', status: 'failed', message: 'must be ignored', restart_required: false },
    { engine: 'mbp', name: '333', status: 'ok', restart_required: true },
    { engine: 'nyc', name: 'cloud-nyc1', status: 'sealed', restart_required: false },
    { engine: 'xeus', name: 'xeus', status: 'offline', restart_required: false },
    { engine: 'sfo', name: 'cloud-sfo2', status: 'not_applicable', message: 'model not available there', restart_required: false },
    { engine: 'box', name: 'box', status: 'failed', message: 'disk full', restart_required: false },
  ]
  const engine_call_all = vi.fn(async (_m: string, _p: unknown, engines: string[]) => answers.filter((a) => engines.includes(a.engine)))
  const set_max_tool_rounds = vi.fn(async () => null)
  const settings = settingsOf(MACHINES, engine_call_all, { set_max_tool_rounds })
  const field = setterField('max-tool-rounds', 'Max tool rounds', 200, 'set_max_tool_rounds')!
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(SetOnAllSheet, { request: { field, value: 200, tab: 'loop', needs: ['mic'] }, settings, onClose: () => {} })))
  // The sheet portals into the body. Queue the offline one too.
  const sheet = () => document.querySelector('[data-set-on-all]')!
  await act(async () => sheet().querySelector<HTMLInputElement>('[data-choice="xeus"] input')!.click())
  await act(async () => [...sheet().querySelectorAll('button')].find((b) => b.textContent?.startsWith('Apply to'))!.click())
  await vi.waitFor(() => expect(sheet().querySelector('[data-result="ok"]')).not.toBeNull())
  // This iPad's engine directly, the mesh through engine_call_all (without it), in parallel; one merged list.
  expect(set_max_tool_rounds).toHaveBeenCalledWith(200)
  expect(engine_call_all).toHaveBeenCalledWith('set_max_tool_rounds', { rounds: 200 }, ['mbp', 'nyc', 'xeus', 'sfo', 'box'])
  const rows = () => [...sheet().querySelectorAll('[data-result]')].map((li) => li.textContent)
  await vi.waitFor(() =>
    expect(rows()).toEqual([
      '✓This iPadapplied',
      '✓333 · MacBook Prorestart neededapplied',
      '–P50 · ThinkPadskipped — n/a: no microphone',
      '◷cloud-nyc1vault locked — applies after unlock',
      '◷xeusoffline — applies when it reconnects',
      '–cloud-sfo2skipped — model not available there',
      '✗boxfailed: disk full',
    ]),
  )
  expect(document.querySelector('[data-slot="sheet-title"]')?.textContent).toBe('Max tool rounds → 200')
  expect(readPending()).toMatchObject([{ id: 'loop/max-tool-rounds', engines: ['nyc', 'xeus'], plan: { kind: 'calls', calls: [{ method: 'set_max_tool_rounds', args: [200] }] } }])
  // xeus came back meanwhile; the vault of cloud-nyc1 is still locked.
  answers[3] = { ...answers[3]!, status: 'ok' }
  await act(async () => [...sheet().querySelectorAll('button')].find((b) => b.textContent === 'Retry pending')!.click())
  await vi.waitFor(() => expect(rows()[4]).toBe('✓xeusapplied'))
  expect(engine_call_all).toHaveBeenLastCalledWith('set_max_tool_rounds', { rounds: 200 }, ['nyc', 'xeus'])
  expect(rows()[3]).toBe('◷cloud-nyc1vault locked — applies after unlock')
  expect(readPending()).toMatchObject([{ engines: ['nyc'] }])
})

test('227 (b): this device\'s engine outside the mesh is ticked by default and gets its own result row', async () => {
  const alone = machine(LOCAL, 'This iPad', { kind: 'local', connected: true, client: LOCAL, forwarded: false, path: null, note: 'not in this mesh' })
  expect(choices([alone, mbp], {}).map((c) => [c.machine.id, c.ticked, c.disabled])).toEqual([[LOCAL, true, null], ['mbp', true, null]])
  const engine_call_all = vi.fn(async (_m: string, _p: unknown, engines: string[]) => engines.map((engine): NodeResult => ({ engine, name: engine, status: 'ok', restart_required: false })))
  const set_max_tool_rounds = vi.fn(async () => {
    throw new Error('disk full')
  })
  const settings = settingsOf([alone, mbp], engine_call_all, { set_max_tool_rounds })
  const field = setterField('max-tool-rounds', 'Max tool rounds', 150, 'set_max_tool_rounds')!
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(SetOnAllSheet, { request: { field, value: 150, tab: 'loop' }, settings, onClose: () => {} })))
  const sheet = () => document.querySelector('[data-set-on-all]')!
  await act(async () => [...sheet().querySelectorAll('button')].find((b) => b.textContent?.startsWith('Apply to 2'))!.click())
  const rows = () => [...sheet().querySelectorAll('[data-result]')].map((li) => li.textContent)
  await vi.waitFor(() => expect(rows()).toEqual(['✗This iPadfailed: disk full', '✓333 · MacBook Proapplied']))
  expect(engine_call_all).toHaveBeenCalledWith('set_max_tool_rounds', { rounds: 150 }, ['mbp'])
})

test('227: needs_press means "press it on that machine": Open on <machine> picks it when this app reaches it with its own client, else open theuth there', async () => {
  const engine_call_all = vi.fn(async (_m: string, _p: unknown, engines: string[]) =>
    engines.map((engine): NodeResult => ({ engine, name: engine, status: engine === 'mbp' || engine === 'box' ? 'needs_press' : 'ok', restart_required: false })),
  )
  const settings = settingsOf([ipad, mbp, box], engine_call_all, { set_max_tool_rounds: vi.fn(async () => null) })
  const field = setterField('max-tool-rounds', 'Max tool rounds', 90, 'set_max_tool_rounds')!
  const onClose = vi.fn()
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(SetOnAllSheet, { request: { field, value: 90, tab: 'loop' }, settings, onClose })))
  const sheet = () => document.querySelector('[data-set-on-all]')!
  await act(async () => [...sheet().querySelectorAll('button')].find((b) => b.textContent?.startsWith('Apply to'))!.click())
  const rows = () => [...sheet().querySelectorAll('[data-result]')].map((li) => li.textContent)
  await vi.waitFor(() =>
    expect(rows()).toEqual([
      '✓This iPadapplied',
      '◷333 · MacBook Proneeds your press on 333 · MacBook ProOpen on 333 · MacBook Pro',
      '◷boxneeds your press on boxopen theuth on box to finish',
    ]),
  )
  // Never queued for the automatic retries.
  expect(readPending()).toEqual([])
  await act(async () => sheet().querySelector<HTMLButtonElement>('[data-open-on="mbp"]')!.click())
  expect(usePickedMachine.getState().picked).toBe('mbp')
  expect(onClose).toHaveBeenCalled()
  usePickedMachine.setState({ picked: null })
})

test('227: an automatic retry that meets needs_press drops it from the queue (never retried again, never marked failed) and reports it', async () => {
  const plan = { kind: 'calls' as const, calls: [{ method: 'set_max_tool_rounds', args: [70] }] }
  settlePending({ id: 'loop/max-tool-rounds', label: 'Max tool rounds', display: '70', plan }, [{ engine: 'mbp', name: 'mbp', status: 'offline', restart_required: false }], true)
  const engine_call_all = vi.fn(async (_m: string, _p: unknown, engines: string[]) => engines.map((engine): NodeResult => ({ engine, name: engine, status: 'needs_press', restart_required: false })))
  const settings = settingsOf([ipad, mbp], engine_call_all, {})
  expect(await retryPending({ mesh: settings.api, local: settings.local, self: ipad }, settings.machines)).toEqual({ ran: 1, needsPress: ['333 · MacBook Pro (Max tool rounds)'] })
  expect(engine_call_all).toHaveBeenCalledWith('set_max_tool_rounds', { rounds: 70 }, ['mbp'])
  expect(readPending()).toEqual([])
})
