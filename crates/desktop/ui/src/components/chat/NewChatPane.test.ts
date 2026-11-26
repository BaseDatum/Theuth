import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { openDraftSession, useDraftTarget, useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import type { CreateSessionOptions } from '@/rpc/bindings/method.sessionsync.input'
import type { EngineStats } from "@/rpc/bindings/method.nodes.output"
import type { Peer } from "@/rpc/bindings/method.nodes.output"
import { PLAIN_DRAFT, createOptions } from './Composer'
import { CreateOptions, machineRows, pickMachine, vanishedTarget } from './NewChatPane'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ list_sessions: async () => [], title_model: async () => null }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

const render = (props: Partial<Parameters<typeof CreateOptions>[0]>) =>
  renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(
        TooltipProvider,
        null,
        createElement(CreateOptions, {
          priv: false,
          onPriv() {},
          project: null,
          onProject() {},
          endless: false,
          onEndless() {},
          settings: undefined,
          ...props,
        }) as ReactNode,
      ),
    }),
  )

test('the ∞ switch sits in the options row after Private; off, no notice', () => {
  const html = render({})
  expect(html).toMatch(/aria-label="Private session".*aria-checked="false"[^>]*aria-label="Fractal context".*>Project</s)
  expect(html).toMatch(/aria-label="Fractal context"[^]*?>∞<\/span>/)
  expect(html).not.toContain('Every turn starts from a compressed view')
})

test('the Private switch is icon-only like ∞: the incognito hat-and-glasses glyph, no visible word', () => {
  const html = render({})
  expect(html).toMatch(/aria-label="Private session"[^]*?lucide-hat-glasses/)
  expect(html).not.toMatch(/<span>Private<\/span>/)
})

test('on: the neutral notice with the three sentences and the compactor line; the model name when one is set', () => {
  const html = render({ endless: true })
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Fractal context"/)
  expect(html).toMatch(/border-border-subtle bg-muted\/40[^"]*"><p class="font-medium">Fractal context<\/p>/)
  expect(html).not.toMatch(/bg-warning-bg.*Fractal context/s)
  expect(html).not.toMatch(/endless|memory/i)
  expect(html).toContain('Every turn starts from a compressed view of the whole chat; nothing is deleted, the distant past just gets coarser.')
  expect(html).toContain('A background summariser keeps the view fresh; the first answer after a long turn may wait a few seconds for it.')
  expect(html).toContain('zoom and recall bring any past message back word for word.')
  expect(html).toContain('Compactor: Same as the session')
  const named = render({
    endless: true,
    settings: receiveComponent('method.agent.output.EndlessConfig', { default_chat: true, default_desk: true, default_meeting: false, view_bytes: 128000, meeting_view_bytes: 32000, node_bytes: 512, slack: 0.125, settle_secs: 45, jobs: 8, tries: 5, retry_secs: 10, cap_chars: 30000, backfill_max_units: 2000, model: 'anthropic/claude-haiku-4-5' }),
  })
  expect(named).toContain('Compactor: anthropic/claude-haiku-4-5')
})

test('createOptions: memory is endless only when the switch is on; private and project ride along unchanged', () => {
  expect(createOptions({ private: false, project: null, endless: true })).toMatchObject({ retention: 'durable', project: null, view: null, memory: 'endless' })
  expect(createOptions({ private: true, project: 'lab', endless: false })).toMatchObject({ retention: 'private', project: 'lab', memory: undefined })
  expect(createOptions({ private: true, project: null, endless: false }).view).not.toBeNull()
})

test('148 R1: the draft pick rides into create_session as this session\'s model/effort; without one both stay null (the engine default)', () => {
  expect(createOptions(PLAIN_DRAFT)).toMatchObject({ retention: 'durable', model: null, effort: null, memory: undefined })
  expect(createOptions(PLAIN_DRAFT, { model: 'openai/gpt-5.4', effort: 'high' })).toMatchObject({ model: 'openai/gpt-5.4', effort: 'high' })
  expect(createOptions({ private: true, project: 'lab', endless: false }, { model: null, effort: 'low' })).toMatchObject({ retention: 'private', project: 'lab', model: null, effort: 'low' })
})

const H = 3_600_000
const NOW = Date.UTC(2026, 10, 8, 12)
const stat = (id: string, os_name: string, cls: EngineStats['class'], repos: string[] = []): EngineStats =>
  ({ id, uptime_since_ms: 0, class: cls, availability_30d: 1, harnesses: [], repos, display: false, gpu: false, os: '', os_name, arch: '', host: '' })
const peer = (id: string, name: string, reachable: boolean, path: Peer['path'], last_seen_ms = NOW): Peer => ({ id, name, addrs: [], reachable, last_seen_ms, hub_pref: 'eligible', path, rtt_ms: null })
const MESH = {
  self_id: 'mac',
  self_name: "Sky's MacBook",
  peers: [peer('xeus', 'xeus', true, 'lan'), peer('cloud', 'cloud-1', true, 'relay'), peer('studio', 'studio', false, 'offline', NOW - 3 * H)],
  stats: [stat('mac', 'macOS', 'laptop'), stat('xeus', 'Arch Linux', 'desktop', ['/home/sky/Develop/theuth']), stat('cloud', 'Ubuntu', 'server')],
}

test('183 mock panel 5: this machine first; LAN / via relay / offline with its age, an offline one not pickable', () => {
  expect(machineRows(MESH, NOW)).toEqual([
    { id: null, name: "Sky's MacBook", detail: 'macOS · laptop', status: 'this machine', online: true, repos: [] },
    { id: 'xeus', name: 'xeus', detail: 'Arch Linux · desktop', status: 'LAN', online: true, repos: ['/home/sky/Develop/theuth'] },
    { id: 'cloud', name: 'cloud-1', detail: 'Ubuntu · server', status: 'via relay', online: true, repos: [] },
    { id: 'studio', name: 'studio', detail: '', status: 'offline 3h', online: false, repos: [] },
  ])
  // A lone engine (or one whose only peer row is itself) shows no picker.
  expect(machineRows({ ...MESH, peers: [] }, NOW)).toEqual([])
  expect(machineRows(undefined, NOW)).toEqual([])
})

// The P50 case: a peer's stats row whose newer columns never replicated (all empty) says nothing about its version.
const EMPTY_XEUS = { ...MESH, stats: [...MESH.stats.filter((s) => s.id !== 'xeus'), { ...stat('xeus', '', 'desktop'), class: '' as EngineStats['class'] }] }

test('a peer whose stats row is empty is an ordinary, pickable machine with what is known as its line', () => {
  const rows = machineRows(EMPTY_XEUS, NOW)
  expect(rows[1]).toEqual({ id: 'xeus', name: 'xeus', detail: '', status: 'LAN', online: true, repos: [] })
  expect(machineRows({ ...MESH, stats: [...MESH.stats.filter((s) => s.id !== 'xeus'), stat('xeus', '', 'desktop')] }, NOW)[1]?.detail).toBe('desktop')
})

test('183 W-10: the chosen machine and folder ride into create_session as engine / cwd, then reset to this machine', async () => {
  const calls: CreateSessionOptions[] = []
  const api = { create_session: async (o: CreateSessionOptions) => (calls.push(o), { session: 'new' }), new_session: async () => 'plain' } as unknown as Api
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/home/sky/Develop/theuth' })
  expect((await openDraftSession(api)).id).toBe('new')
  expect(calls[0]).toMatchObject({ engine: 'xeus', cwd: '/home/sky/Develop/theuth', model: null })
  expect(useDraftTarget.getState()).toMatchObject({ engine: null, cwd: null })
  // This machine, nothing picked: still the plain `new_session()`.
  expect((await openDraftSession(api)).id).toBe('plain')
  // A New Chat switch builds the options; the machine joins them.
  useDraftTarget.getState().set({ engine: 'cloud' })
  await openDraftSession(api, createOptions({ private: false, project: 'lab', endless: false }))
  expect(calls[1]).toMatchObject({ project: 'lab', engine: 'cloud', cwd: null })
})

test('183 R1: picking another machine drops the draft\'s model/effort pick (its models differ); re-picking the same keeps it', () => {
  useDraftTarget.getState().set({ engine: null, cwd: '~/x' })
  useDrafts.getState().setPick({ model: 'local/m', effort: 'high' })
  pickMachine(null)
  expect(useDrafts.getState().pick).toMatchObject({ model: 'local/m', effort: 'high' })
  pickMachine('peer-b')
  expect(useDrafts.getState().pick.model ?? null).toBeNull()
  expect(useDrafts.getState().pick.effort ?? null).toBeNull()
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'peer-b', cwd: null })
})

test('183 R1: a target that left the mesh is cleared back to this machine; a present one, this machine and an unloaded roster are kept', () => {
  const mesh = { self_id: 'me', self_name: 'mac', peers: [peer('b', 'b', true, 'lan')], stats: [] }
  const rows = machineRows(mesh)
  expect(vanishedTarget(rows, true, 'b')).toBe(false)
  expect(vanishedTarget(rows, true, null)).toBe(false)
  expect(vanishedTarget(rows, true, 'gone')).toBe(true)
  // The last peer unpaired: no rows at all, the pick would fail the first send.
  expect(vanishedTarget(machineRows({ ...mesh, peers: [] }), true, 'b')).toBe(true)
  // The roster has not loaded yet: nothing is known to have vanished.
  expect(vanishedTarget([], false, 'b')).toBe(false)
})
