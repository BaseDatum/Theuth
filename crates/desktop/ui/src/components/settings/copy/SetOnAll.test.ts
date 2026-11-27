import '../test-dom'
import { QueryClient } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { setterField } from '@/components/settings/copy/fields'
import { type Choice, choices, readPending } from '@/components/settings/copy/set-on-all'
import { SetOnAllSheet } from '@/components/settings/copy/SetOnAllSheet'
import type { SettingsMachines } from '@/components/settings/machine-target'
import type { FullApi, NodeResult, WireEngineCaps } from '@/lib/engine-call.types'
import type { Machine } from '@/lib/mesh-machines'

const caps = (c: Partial<WireEngineCaps>): WireEngineCaps => ({ engine: '', name: '', platform: 'linux', display: true, mic: true, interactive_user: true, headless: false, hub: false, ...c })
const machine = (id: string, name: string, m: Partial<Machine> = {}): Machine => ({ id, name, kind: 'peer', connected: false, hub: false, online: true, sealed: false, path: 'direct', ...m })

const ipad = machine('ipad', 'This iPad', { kind: 'local', connected: true, path: null })
const mbp = machine('mbp', '333 · MacBook Pro', { kind: 'hub', hub: true })
const p50 = machine('p50', 'P50 · ThinkPad', { caps: caps({ mic: false, display: false }) })
const nyc = machine('nyc', 'cloud-nyc1', { kind: 'cloud', sealed: true })
const xeus = machine('xeus', 'xeus', { online: false, path: null })
const sfo = machine('sfo', 'cloud-sfo2', { kind: 'cloud' })
const box = machine('box', 'box')
const MACHINES = [ipad, mbp, p50, nyc, xeus, sfo, box]

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
    { engine: 'ipad', name: 'This iPad', status: 'ok', restart_required: false },
    { engine: 'mbp', name: '333', status: 'ok', restart_required: true },
    { engine: 'nyc', name: 'cloud-nyc1', status: 'sealed', restart_required: false },
    { engine: 'xeus', name: 'xeus', status: 'offline', restart_required: false },
    { engine: 'sfo', name: 'cloud-sfo2', status: 'not_applicable', message: 'model not available there', restart_required: false },
    { engine: 'box', name: 'box', status: 'failed', message: 'disk full', restart_required: false },
  ]
  const engine_call_all = vi.fn(async (_m: string, _p: unknown, engines: string[]) => answers.filter((a) => engines.includes(a.engine)))
  const settings: SettingsMachines = { machines: MACHINES, target: p50, connected: ipad, api: { engine_call_all } as unknown as FullApi, qc: new QueryClient(), tab: null, open: () => {} }
  const field = setterField('max-tool-rounds', 'Max tool rounds', 200, 'set_max_tool_rounds')!
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(SetOnAllSheet, { request: { field, value: 200, tab: 'loop', needs: ['mic'] }, settings, onClose: () => {} })))
  // The sheet portals into the body. Queue the offline one too.
  const sheet = () => document.querySelector('[data-set-on-all]')!
  await act(async () => sheet().querySelector<HTMLInputElement>('[data-choice="xeus"] input')!.click())
  await act(async () => [...sheet().querySelectorAll('button')].find((b) => b.textContent?.startsWith('Apply to'))!.click())
  await vi.waitFor(() => expect(sheet().querySelector('[data-result="ok"]')).not.toBeNull())
  expect(engine_call_all).toHaveBeenCalledWith('set_max_tool_rounds', { rounds: 200 }, ['ipad', 'mbp', 'nyc', 'xeus', 'sfo', 'box'], null)
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
  expect(engine_call_all).toHaveBeenLastCalledWith('set_max_tool_rounds', { rounds: 200 }, ['nyc', 'xeus'], null)
  expect(rows()[3]).toBe('◷cloud-nyc1vault locked — applies after unlock')
  expect(readPending()).toMatchObject([{ engines: ['nyc'] }])
})
