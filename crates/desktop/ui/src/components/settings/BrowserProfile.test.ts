import './test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { WebProfileSite } from '@/lib/web-profile-wire'
import { BrowserProfile, modeText, siteLines, writerLabel } from './BrowserProfile'

const NOW = 1_795_000_000_000
const MIN = 60_000

const engine = vi.hoisted(() => ({
  sites: [] as WebProfileSite[] | Error,
  web_profile_sites: vi.fn(),
  web_profile_forget: vi.fn(),
  web_profile_clear: vi.fn(),
  web_profile_status: vi.fn(),
  changed: null as null | (() => void),
  signedOut: vi.fn(),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    web_profile_sites: engine.web_profile_sites,
    web_profile_forget: engine.web_profile_forget,
    web_profile_clear: engine.web_profile_clear,
    web_profile_status: engine.web_profile_status,
  }),
}))
const support = vi.hoisted(() => ({ status: true }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: (methods: string[]) => support.status || !methods.includes('web_profile_status') }))
vi.mock('@/store/sessions', () => ({
  on: (_topic: string, _type: string, cb: () => void) => {
    engine.changed = cb
    return () => (engine.changed = null)
  },
}))
vi.mock('@/lib/web-profile', () => ({
  profileDevice: () => 'iPad-7f3a2b',
  signedOutEverywhere: engine.signedOut,
  useWebProfileMode: (pick: (s: { engine: string | null; mode: 'shared' | 'off' | null }) => unknown) => pick({ engine: 'e1', mode: 'shared' }),
}))
vi.mock('@/link/engines', () => ({ useEngines: (pick: (s: { active: string }) => unknown) => pick({ active: 'e1' }) }))
// React Query notifies observers on a macrotask by default; on a microtask they settle inside one `act`.
notifyManager.setScheduler(queueMicrotask)

const site = (s: string, updated_ms: number, writer: string, cookies = 2, origins = 0): WebProfileSite => ({ site: s, cookies, origins, updated_ms, writer })

beforeEach(() => {
  engine.sites = [site('example.com', NOW - 5 * MIN, 'tile:iPad-7f3a2b'), site('github.com', NOW - MIN, 'obscura:studio', 1, 1)]
  engine.web_profile_sites.mockReset().mockImplementation(async () => {
    if (engine.sites instanceof Error) throw engine.sites
    return engine.sites
  })
  engine.web_profile_forget.mockReset().mockImplementation(async (s: string) => {
    if (Array.isArray(engine.sites)) engine.sites = engine.sites.filter((x) => x.site !== s)
    return {}
  })
  engine.web_profile_clear.mockReset().mockImplementation(async () => {
    engine.sites = []
    return { epoch: 7 }
  })
  engine.web_profile_status.mockReset().mockResolvedValue({ profile: 'off', epoch: 1, sites: 2, now_ms: NOW })
  support.status = true
  engine.signedOut.mockReset()
  vi.spyOn(Date, 'now').mockReturnValue(NOW)
})
afterEach(() => {
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

const mount = async () => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc }, createElement(BrowserProfile))))
  return { host, unmount: () => act(async () => root.unmount()) }
}
const run = (fn: () => void) => act(async () => fn())
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!

test('writers read as this device, an agent on its engine, or another device', () => {
  expect(writerLabel('tile:iPad-7f3a2b', 'iPad-7f3a2b')).toBe('this device')
  // Another iPad is another install: the whole device string decides.
  expect(writerLabel('tile:iPad-0c91de', 'iPad-7f3a2b')).toBe('another device')
  expect(writerLabel('tile:iPad', 'iPad-7f3a2b')).toBe('another device')
  expect(writerLabel('tile:Mac', 'iPad')).toBe('another device')
  expect(writerLabel('obscura:studio', 'iPad')).toBe('agent on studio')
  expect(writerLabel('something-else', 'iPad')).toBe('another device')
})

test('the list is newest change first (then by site), with counts, relative time and writer; never a value', () => {
  const lines = siteLines(
    [site('b.org', NOW - 3 * 3_600_000, 'tile:Mac', 1), site('example.com', NOW - 5 * MIN, 'tile:iPad-7f3a2b', 3, 2), site('a.org', NOW - 3 * 3_600_000, 'obscura:studio', 4, 1)],
    'iPad-7f3a2b',
    NOW,
  )
  expect(lines).toEqual([
    { site: 'example.com', detail: '3 cookies · storage of 2 origins · changed 5 min ago by this device' },
    { site: 'a.org', detail: '4 cookies · storage of 1 origin · changed 3 h ago by agent on studio' },
    { site: 'b.org', detail: '1 cookie · changed 3 h ago by another device' },
  ])
  expect(modeText(null, false)).toContain('Not known yet')
  expect(modeText('off', false)).toContain('As the engine last reported to this device')
  expect(modeText('off', true)).not.toContain('last reported')
})

test('Forget calls web_profile_forget for its site and the list refreshes; WebProfileChanged refreshes it too', async () => {
  const { host, unmount } = await mount()
  expect([...host.querySelectorAll('[data-settings-row]')].map((r) => r.textContent)).toEqual([
    expect.stringContaining('Sharing'),
    expect.stringContaining('github.com1 cookie · storage of 1 origin · changed 1 min ago by agent on studio'),
    expect.stringContaining('example.com2 cookies · changed 5 min ago by this device'),
    expect.stringContaining('Sign out everywhere'),
  ])
  await run(() => button('Forget example.com and sign out of it everywhere').click())
  expect(engine.web_profile_forget).toHaveBeenCalledWith('example.com')
  await vi.waitFor(() => expect(engine.web_profile_sites).toHaveBeenCalledTimes(2))
  await run(() => undefined)
  expect(host.textContent).not.toContain('example.com')

  engine.sites = [site('new.org', NOW, 'tile:iPad')]
  await run(() => engine.changed!())
  await vi.waitFor(() => expect(host.textContent).toContain('new.org'))
  expect(engine.web_profile_sites).toHaveBeenCalledTimes(3)
  await unmount()
})

test('Sign out everywhere asks first; confirmed, it calls web_profile_clear, clears this device and refreshes', async () => {
  const { host, unmount } = await mount()
  await run(() => [...host.querySelectorAll('button')].find((b) => b.textContent === 'Sign out everywhere')!.click())
  expect(engine.web_profile_clear).not.toHaveBeenCalled()
  const dialog = document.querySelector('[role=alertdialog]')!
  expect(dialog.textContent).toContain('Sign out of every site everywhere?')
  await run(() => [...dialog.querySelectorAll('button')].find((b) => b.textContent === 'Sign out everywhere')!.click())
  expect(engine.web_profile_clear).toHaveBeenCalledTimes(1)
  await vi.waitFor(() => expect(engine.signedOut).toHaveBeenCalledWith(7))
  await vi.waitFor(() => expect(host.textContent).toContain('No signed-in sites'))
  expect(engine.web_profile_sites).toHaveBeenCalledTimes(2)
  await unmount()
})

test('a failed list says so', async () => {
  engine.sites = new Error('profile unavailable')
  const { host, unmount } = await mount()
  await vi.waitFor(() => expect(host.textContent).toContain('Could not list the signed-in sites: profile unavailable'))
  await unmount()
})

test('the Sharing row reads browser.profile from web_profile_status, else from what the sync last heard', async () => {
  const first = await mount()
  await vi.waitFor(() => expect(first.host.textContent).toContain('Off (browser.profile: off in config.yaml)'))
  expect(first.host.textContent).not.toContain('As the engine last reported')
  await first.unmount()
  document.body.innerHTML = ''

  support.status = false
  const second = await mount()
  expect(engine.web_profile_status).toHaveBeenCalledTimes(1)
  expect(second.host.textContent).toContain('On: sign-ins in your web views')
  expect(second.host.textContent).toContain('As the engine last reported to this device')
  await second.unmount()
})

test('relative times move on every minute without anything else re-rendering', async () => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
  try {
    const { host, unmount } = await mount()
    await vi.waitFor(() => expect(host.textContent).toContain('changed 5 min ago by this device'))
    vi.mocked(Date.now).mockReturnValue(NOW + 2 * MIN)
    await act(async () => vi.advanceTimersByTime(60_000))
    expect(host.textContent).toContain('changed 7 min ago by this device')
    await unmount()
  } finally {
    vi.useRealTimers()
  }
})
