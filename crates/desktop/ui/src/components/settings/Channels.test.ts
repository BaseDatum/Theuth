import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { TelegramEvent } from '@/rpc/bindings/TelegramEvent'
import type { TelegramStatus } from '@/rpc/bindings/TelegramStatus'
import { Channels, mmss, secondsLeft, telegramId, telegramState } from './Channels'

let telegram: TelegramStatus
let telegramError: Error | null = null
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    signal_status: async () => ({ state: null, registered: false, number: null, owner: '', allowed: [] }),
    slack_status: async () => ({ state: null, bot_user: null, team: null, owner: '', allowed: [], channels: [] }),
    telegram_status: async () => {
      if (telegramError) throw telegramError
      return telegram
    },
  }),
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Channels) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

const base = (state: TelegramEvent | null, polling = true, bot = true): TelegramStatus => ({
  state,
  bot: bot ? { id: 7n, username: 'omp_bot', topics: true, users_may_create_topics: true } : null,
  polling,
  owner: null,
  allowed: [],
  groups: [],
  threads: [],
  open: 0,
  calls: {},
  dropped_updates: 0n,
})

test('telegramState: one phrase per variant (98 §10.5)', () => {
  const cases: [TelegramEvent, string][] = [
    [{ type: 'Unconfigured' }, 'no bot token'],
    [{ type: 'Identity', bot: 'omp_bot', topics: false }, 'connected as @omp_bot · topics off'],
    [{ type: 'Connected', bot: 'omp_bot' }, 'connected as @omp_bot · topics on'],
    [{ type: 'Disconnected', reason: 'timeout' }, 'disconnected: timeout'],
    [{ type: 'Conflict' }, 'another poller holds this token'],
    [{ type: 'RateLimited', method: 'sendMessage', retry_after_secs: 5n }, 'rate limited on sendMessage, retry in 5s'],
    [{ type: 'StreamFallback', session: 's1', reason: 'draft failed' }, 'connected'],
    [{ type: 'Unreachable', chat: -100n, reason: 'kicked' }, 'chat -100 unreachable: kicked'],
    [{ type: 'PairingLink', url: 'https://t.me/omp_bot?start=x', expires_at_ms: 0n }, 'waiting for the owner to tap Start'],
    [{ type: 'Paired', user: 42n, name: 'Sky' }, 'paired with Sky'],
    [{ type: 'DaemonOnly' }, 'daemon only'],
  ]
  for (const [state, phrase] of cases) expect(telegramState(base(state))).toBe(phrase)
  expect(telegramState(base(null))).toBe('unknown')
})

test('telegramState: idle suffix only for a configured bot that is not polling', () => {
  expect(telegramState(base({ type: 'Connected', bot: 'omp_bot' }, false))).toBe('connected as @omp_bot · topics on · idle (hub elsewhere)')
  expect(telegramState(base({ type: 'Connected', bot: 'omp_bot' }, false, false))).toBe('connected as @omp_bot')
  expect(telegramState(base({ type: 'Unconfigured' }, false, false))).toBe('no bot token')
})

test('telegramId: digits only; a leading minus only for chats', () => {
  expect(telegramId(' 123456789 ', 'user')).toBe(123456789)
  expect(telegramId('-1001234567890', 'chat')).toBe(-1001234567890)
  for (const bad of ['', '-1', '12a', '@sky', '+1 555', '1.5']) expect(telegramId(bad, 'user')).toBeNull()
  for (const bad of ['', '-', '--100', '1e3']) expect(telegramId(bad, 'chat')).toBeNull()
})

test('countdown: whole seconds, floored at 0, rendered m:ss', () => {
  expect(secondsLeft(10_000, 0)).toBe(10)
  expect(secondsLeft(10_001, 0)).toBe(11)
  expect(secondsLeft(5_000, 9_000)).toBe(0)
  expect(mmss(600)).toBe('10:00')
  expect(mmss(65)).toBe('1:05')
  expect(mmss(0)).toBe('0:00')
})

test('card: no token → "not set", a password input, Pair disabled; no link block', async () => {
  telegram = base({ type: 'Unconfigured' }, false, false)
  const html = await render()
  expect(html).toContain('no bot token')
  expect(html).toContain('not set')
  expect(html).toMatch(/<input[^>]*type="password"/)
  expect(html).toMatch(/<button[^>]*disabled[^>]*>Pair</)
  expect(html).not.toContain('expires in')
})

test('card: live pairing link → QR, URL, countdown; owner and lists render as text', async () => {
  const url = 'https://t.me/omp_bot?start=abcdefgh12345678'
  telegram = {
    ...base({ type: 'PairingLink', url, expires_at_ms: BigInt(Date.now() + 9 * 60_000 + 30_000) }),
    owner: 42n,
    allowed: [1n, 2n],
    groups: [-1001234567890n],
  }
  const html = await render()
  expect(html).toContain('waiting for the owner to tap Start')
  expect(html).toContain('>set<')
  expect(html).toContain('Replace…')
  expect(html).toContain('Re-pair')
  expect(html).toContain('>42<')
  expect(html).toMatch(/expires in 9:(29|30)/)
  expect(html).toContain('<svg')
  expect(html).toContain(url)
  for (const id of ['1', '2', '-1001234567890']) expect(html).toContain(`Remove ${id}`)
})

test('card: while telegram_status is pending the section shows "loading…", never nothing', () => {
  telegram = base({ type: 'Unconfigured' }, false, false)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Channels) as ReactNode }))
  expect(html).toContain('Telegram')
  expect(html).toContain('loading…')
})

test('card: a failing telegram_status renders the error and a Retry button', async () => {
  telegramError = new Error('telegram actor is not running')
  try {
    // `retryOnMount: false`: a fresh server-render observer would otherwise show the errored query as pending again.
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false, retryOnMount: false } } })
    const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Channels) as ReactNode }))
    paint()
    for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey }).catch(() => undefined)
    const html = paint()
    expect(html).toContain('status unavailable: telegram actor is not running')
    expect(html).toMatch(/<button[^>]*>Retry</)
  } finally {
    telegramError = null
  }
})
