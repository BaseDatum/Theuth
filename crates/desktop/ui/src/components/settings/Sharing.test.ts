import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { isNotOnEngine, namesApi, NotOnEngine } from '@/components/public-share/wire'
import { makeApi } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { usePublicShares } from '@/store/public-shares'
import { ApprovalsOffWarning, availabilityText, d22Text, MachinesSection, MeshIdRow, NameSection, nameProblem, publishArgs } from './Sharing'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn(), message: vi.fn() }) }))

const warning = (sharing: boolean, approvals: boolean | null) => renderToStaticMarkup(h(ApprovalsOffWarning, { sharing, approvals }))

test('Settings › Sharing warns when sharing is on and tools.approvals is off, only then', () => {
  expect(warning(true, false)).toContain('Approvals are off. Publishing still asks you, but other tools run without asking.')
  expect(warning(true, true)).toBe('')
  expect(warning(false, false)).toBe('')
  // Not loaded yet: no warning rather than a false one.
  expect(warning(true, null)).toBe('')
})

test('the publish form: a number is a port, anything else a folder; password access on request', () => {
  expect(publishArgs(' 5173 ', '8h', false)).toEqual({ source: { port: 5173 }, expiry: '8h', access: 'public' })
  expect(publishArgs('/srv/site', '1h', true)).toEqual({ source: { dir: '/srv/site' }, expiry: '1h', access: 'password' })
  expect(publishArgs('  ', '8h', false)).toBeNull()
})

test('§5.2 name rule, before asking the edge', () => {
  expect(nameProblem('sky')).toBeNull()
  expect(nameProblem('my-app-2')).toBeNull()
  expect(nameProblem('ab')).toBe('At least 3 characters')
  expect(nameProblem('a'.repeat(33))).toBe('At most 32 characters')
  expect(nameProblem('Sky')).toBe('Only a–z, 0–9 and -')
  expect(nameProblem('-sky')).toBe('No - at the start or end')
  expect(nameProblem('xn--abc')).toBe('No xn-- names')
  expect(nameProblem('admin')).toBe('Reserved by the edge')
  expect(nameProblem('constructor')).toBeNull()
})

test('§5.2 availability: free, taken, and a cooldown with the date it ends', () => {
  const now = Date.UTC(2026, 10, 23)
  expect(availabilityText('sky.tun.zip', { available: true, reason: null, reclaim_after_ms: null }, now)).toEqual({ ok: true, text: 'sky.tun.zip is free' })
  expect(availabilityText('sky.tun.zip', { available: false, reason: 'name_taken', reclaim_after_ms: null }, now)).toEqual({ ok: false, text: 'sky.tun.zip is taken' })
  const until = Date.UTC(2026, 11, 20)
  const cool = availabilityText('sky.tun.zip', { available: false, reason: 'cooldown', reclaim_after_ms: until }, now)
  expect(cool.ok).toBe(false)
  expect(cool.text).toContain(`free again on ${new Date(until).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })}`)
  // An edge that does not send `reclaim_after_ms`.
  expect(availabilityText('sky', { available: false, reason: 'cooldown', reclaim_after_ms: null }, now).text).toBe('sky was released recently and is cooling down')
})

test('D22 names the hub', () => {
  expect(d22Text('tun.zip', { self: 'e1', hub: 'e2', names: { e1: 'MacBook', e2: 'Studio' } })).toBe(
    'When a machine is removed from your mesh, the hub (Studio) contacts tun.zip to revoke it there, even if sharing is off on the hub.',
  )
  expect(d22Text('tun.zip', null)).toContain('the hub contacts tun.zip')
})

let root: Root | undefined
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
  usePublicShares.getState().reset()
  useConnection.setState({ client: null, api: null })
})

async function mount(el: Parameters<typeof h>[0]) {
  const div = document.createElement('div')
  document.body.append(div)
  root = createRoot(div)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(el as never))))
}

/** A client answering `share_names_*` from `answer` behind the generated `Api`; anything else is -32601 like an engine
 * without the method. */
function client(answer: Record<string, (params: unknown[]) => unknown>) {
  const calls: [string, unknown[]][] = []
  const c = {
    call: async (method: string, params: unknown[]) => {
      calls.push([method, params])
      const f = answer[method]
      if (!f) throw new RpcError(-32601, 'Method not found')
      return f(params)
    },
  }
  useConnection.setState({ client: c as never, api: makeApi(c as never) })
  return calls
}

const text = () => document.body.textContent ?? ''
const button = (label: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === label)

test('M2 names on an engine without the methods: "not available on this engine", not an error', async () => {
  client({})
  await mount(NameSection)
  await vi.waitFor(() => expect(text()).toContain('Your names: not available on this engine.'))
})

test('M2 names: the list, primary badge and state; Release asks once more, then calls share_names_release', async () => {
  const calls = client({
    share_names_list: () => [
      { name: 'sky', domain: 'tun.zip', kind: 'reserved', primary: true, state: 'valid' },
      { name: 'k3x7qa', domain: 'tun.zip', kind: 'random', primary: false, state: 'queued', queue_position: 3, queue_eta_s: 86_400 },
    ],
    share_names_release: () => null,
  })
  await mount(NameSection)
  await vi.waitFor(() => expect(text()).toContain('sky.tun.zipprimary'))
  expect(text()).toContain('Reserved · Ready')
  expect(text()).toContain('Random · Waiting for a certificate slot — position 3, about 24 h')

  const sky = document.querySelector('[data-held-name="sky"]')!
  await act(async () => [...sky.querySelectorAll('button')].find((b) => b.textContent === 'Release')!.click())
  expect(calls.some(([m]) => m === 'share_names_release')).toBe(false)
  expect(sky.textContent).toContain('Its shares stop on every machine')
  const confirm = [...sky.querySelectorAll<HTMLButtonElement>('[role="alert"] button')].find((b) => b.textContent === 'Release')!
  await act(async () => confirm.click())
  expect(calls.filter(([m]) => m === 'share_names_release')).toEqual([['share_names_release', ['sky']]])
})

test('M2 names: an action the engine lacks says so on its row', async () => {
  client({ share_names_list: () => [{ name: 'k3x7qa', domain: 'tun.zip', kind: 'random', primary: false, state: 'valid' }] })
  await mount(NameSection)
  await vi.waitFor(() => expect(button('Make primary')).toBeDefined())
  await act(async () => button('Make primary')!.click())
  await vi.waitFor(() => expect(text()).toContain('Not available on this engine.'))
})

test('M2 machines: every machine of the mesh with what it serves; the hub marked', async () => {
  usePublicShares.getState().setMachines({ self: 'e1', hub: 'e2', names: { e1: 'MacBook', e2: 'Studio', e3: 'Pi' } })
  const share = (id: string, engine: string, host: string, path = '/') =>
    ({ id, engine, host, path, state: 'live', url: `https://${host}${path}`, created_ms: 1 }) as never
  usePublicShares.getState().replace([share('a', 'e1', 'app.sky.tun.zip'), share('b', 'e2', 'sky.tun.zip', '/docs'), share('c', 'e2', 'api2.sky.tun.zip')])
  await mount(MachinesSection)
  const rows = text()
  expect(rows).toContain('MacBook (this machine)Serves app.sky.tun.zip')
  expect(rows).toContain('StudiohubServes api2.sky.tun.zip, sky.tun.zip/docs')
  expect(rows).toContain('PiNothing public')
})

test('M2 namesApi: typed calls with the generated shapes; a missing method is NotOnEngine', async () => {
  const calls = client({
    share_names_check: () => ({ available: false, reason: 'cooldown', reclaim_after_ms: 1_800_000_000_000 }),
    share_names_release: ([name]) => ({ name, reclaim_after_ms: 1_800_000_000_000 }),
    share_names_transfer: () => 1_790_000_000_000,
    share_names_rotate: () => 2,
  })
  expect(await namesApi.check('sky')).toEqual({ available: false, reason: 'cooldown', reclaim_after_ms: 1_800_000_000_000 })
  expect(await namesApi.release('sky')).toEqual({ name: 'sky', reclaim_after_ms: 1_800_000_000_000 })
  expect(await namesApi.transfer('sky', 'mesh-abc')).toBe(1_790_000_000_000)
  expect(await namesApi.rotate('sky')).toBe(2)
  expect(calls).toEqual([
    ['share_names_check', ['sky']],
    ['share_names_release', ['sky']],
    ['share_names_transfer', ['sky', 'mesh-abc']],
    ['share_names_rotate', ['sky']],
  ])
  const missing = await namesApi.primary('sky').catch((e: unknown) => e)
  expect(missing).toBeInstanceOf(NotOnEngine)
  expect(missing instanceof NotOnEngine && missing.method).toBe('share_names_primary')
  // Not connected: a plain error, not "not available on this engine".
  useConnection.setState({ client: null, api: null })
  const offline = await namesApi.list().catch((e: unknown) => e)
  expect(isNotOnEngine(offline)).toBe(false)
})

test('D20 mesh id: shown with a copy button for receiving a transfer; hidden while the engine has none', async () => {
  expect(renderToStaticMarkup(h(MeshIdRow, { meshId: null }))).toBe('')
  const writeText = vi.fn(async () => undefined)
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
  client({ share_names_list: () => [] })
  await mount(() => h(NameSection, { meshId: 'z6mkmesh0123456789' }))
  expect(text()).toContain('Your mesh id')
  expect(text()).toContain('Give this to the other mesh to receive a transfer.')
  expect(document.querySelector('[data-mesh-id]')?.textContent).toBe('z6mkmesh0123456789')
  await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Copy mesh id"]')!.click())
  expect(writeText).toHaveBeenCalledWith('z6mkmesh0123456789')
})

test('D20 mesh id: NameSection without one shows no mesh id row', async () => {
  client({ share_names_list: () => [] })
  await mount(NameSection)
  await vi.waitFor(() => expect(text()).toContain('No name yet.'))
  expect(text()).not.toContain('Your mesh id')
})
