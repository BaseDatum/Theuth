import { expect, test } from 'vitest'
import type { CostCells, CostCube, LoginInfo, ProviderInfo, ProviderLimits, UsageLimits } from '@/rpc/bindings/method.agent.output'
import { emptyCube, HOUR_MS, index } from '@/lib/costs/cube'
import { usd } from '@/lib/costs/format'
import { coveredLine, planName, subscriptionProviders, subscriptionValue } from './subscription-value'

// providers: 0 anthropic, 1 openai, 2 openrouter; bases: 0 known, 1 reported, 2 estimated, 3 unpriced.
type Cell = { at: string; provider: number; basis?: number; usd: number; calls?: number }
const BASIS = { known: 0, reported: 1, estimated: 2, unpriced: 3 } as const

function cube(cells: Cell[]): CostCube {
  const c: CostCells = { hour: [], root: [], model: [], provider: [], kind: [], via: [], basis: [], nanos: [], calls: [], failed: [], repriced: [], input: [], output: [], cache_read: [], cache_write: [], cache_saved_nanos: [], approx_nanos: [] }
  for (const x of cells) {
    c.hour.push(Math.floor(Date.parse(x.at) / HOUR_MS))
    c.root.push(0)
    c.model.push(x.provider)
    c.provider.push(x.provider)
    c.kind.push(0)
    c.via.push(0)
    c.basis.push(x.basis ?? BASIS.estimated)
    c.nanos.push(Math.round(x.usd * 1e9))
    c.calls.push(x.calls ?? 1)
    for (const k of ['failed', 'repriced', 'input', 'output', 'cache_read', 'cache_write', 'cache_saved_nanos', 'approx_nanos'] as const) c[k].push(0)
  }
  return {
    ...emptyCube(),
    epoch: 'e',
    rev: 1,
    providers: ['anthropic', 'openai', 'openrouter'],
    kinds: ['round'],
    vias: ['theuth'],
    models: [
      { id: 'anthropic/claude-sonnet-5', name: 'claude-sonnet-5', provider: 'anthropic' },
      { id: 'openai/gpt-6', name: 'gpt-6', provider: 'openai' },
      { id: 'openrouter/x', name: 'x', provider: 'openrouter' },
    ],
    roots: [],
    cells: c,
  }
}

const login = (name: string, logged_in = true, account: string | null = null): LoginInfo => ({ name, logged_in, account, expires_at: null })
const provider = (name: string, has_key: boolean): ProviderInfo =>
  ({ name, has_key, configured: has_key, builtin: false, local: false, models: 1, base_url: '', refreshable: false, runtime: null }) as ProviderInfo
const limitsRow = (provider: string, auth: string, over: Partial<ProviderLimits> = {}): ProviderLimits =>
  ({ id: `${provider}:${auth}`, provider, auth, label: provider, account: null, plan: null, status: 'ok', message: null, source: 'usage endpoint', observed_at_ms: null, windows: [], rates: [], balance: null, ...over }) as ProviderLimits
const win = (id: string, used: number) => ({ id, label: id, used_fraction: used, resets_at_ms: null, window_secs: null, history: [] })

const NOW = Date.parse('2026-11-24T15:30:00Z')

test('subscriptionProviders: logged-in logins without an own key, engine order', () => {
  const logins = [login('openai'), login('anthropic'), login('xai', false), login('cursor')]
  expect(subscriptionProviders(logins, [provider('anthropic', true), provider('openai', false)]).map((l) => l.name)).toEqual(['openai', 'cursor'])
  expect(subscriptionProviders(logins, undefined).map((l) => l.name)).toEqual(['openai', 'anthropic', 'cursor'])
  expect(subscriptionProviders([], [])).toEqual([])
})

test('sums estimated cells of the provider: this local month and the rolling 30 days', () => {
  const ix = index(
    cube([
      { at: '2026-11-24T10:00:00Z', provider: 0, usd: 2.5, calls: 3 }, // this month, in 30 d
      { at: '2026-11-01T00:00:00Z', provider: 0, usd: 1, calls: 1 }, // first hour of the month
      { at: '2026-10-31T23:00:00Z', provider: 0, usd: 4, calls: 2 }, // last month, in 30 d
      { at: '2026-10-20T12:00:00Z', provider: 0, usd: 8, calls: 5 }, // last month, outside 30 d
      { at: '2026-11-20T12:00:00Z', provider: 0, basis: BASIS.known, usd: 100 }, // key-billed: not counted
      { at: '2026-11-20T12:00:00Z', provider: 0, basis: BASIS.reported, usd: 50 },
      { at: '2026-11-20T12:00:00Z', provider: 0, basis: BASIS.unpriced, usd: 0, calls: 9 },
      { at: '2026-11-21T12:00:00Z', provider: 1, usd: 7 }, // other provider
      { at: '2026-11-25T12:00:00Z', provider: 0, usd: 1000 }, // after now (clock skew)
    ]),
    'UTC',
  )
  const [v] = subscriptionValue(ix, [login('anthropic', true, 'me@x')], [], undefined, NOW)
  expect(v).toMatchObject({ provider: 'anthropic', name: 'Claude', account: 'me@x', plan: null, windows: [] })
  expect(v!.month).toBeCloseTo(3.5, 9)
  expect(v!.last30).toBeCloseTo(7.5, 9)
  expect(v!.calls30).toBe(6)
})

test('the month boundary is local: a cell on 1 Nov UTC is still October in Los Angeles', () => {
  const ix = index(cube([{ at: '2026-11-01T03:00:00Z', provider: 0, usd: 5 }, { at: '2026-11-10T12:00:00Z', provider: 0, usd: 1 }]), 'America/Los_Angeles')
  const [v] = subscriptionValue(ix, [login('anthropic')], [], undefined, NOW)
  expect(v!.month).toBeCloseTo(1, 9)
  expect(v!.last30).toBeCloseTo(6, 9)
})

test('a provider with its own key is left out; one the cube never saw sums to zero', () => {
  const ix = index(cube([{ at: '2026-11-20T12:00:00Z', provider: 0, usd: 3 }]), 'UTC')
  const out = subscriptionValue(ix, [login('anthropic'), login('xai')], [provider('anthropic', true)], undefined, NOW)
  expect(out).toEqual([{ provider: 'xai', name: 'Grok', account: null, plan: null, month: 0, last30: 0, calls30: 0, windows: [] }])
})

test('an empty cube and no logins', () => {
  expect(subscriptionValue(index(emptyCube(), 'UTC'), [login('openai')], [], undefined, NOW)).toMatchObject([{ month: 0, last30: 0, calls30: 0 }])
  expect(subscriptionValue(index(cube([{ at: '2026-11-20T12:00:00Z', provider: 0, usd: 3 }]), 'UTC'), [], [], undefined, NOW)).toEqual([])
})

test('plan and windows come from the non-key usage_limits rows of that provider', () => {
  const limits: UsageLimits = {
    generated_at_ms: NOW,
    budgets: [],
    providers: [
      limitsRow('anthropic', 'api_key', { plan: 'tier 4', windows: [win('api', 0.1)] }),
      limitsRow('anthropic', 'oauth', { plan: 'max', windows: [win('five_hour', 0.4), win('seven_day', 0.2)] }),
      limitsRow('openai', 'oauth', { plan: 'pro', windows: [win('primary', 0.9)] }),
    ],
  }
  const [v] = subscriptionValue(index(emptyCube(), 'UTC'), [login('anthropic')], [], limits, NOW)
  expect(v!.plan).toBe('max')
  expect(v!.windows.map((w) => w.id)).toEqual(['five_hour', 'seven_day'])
})

test('wording', () => {
  expect(coveredLine({ name: 'Claude', month: 41.2 }, usd)).toBe('Your Claude subscription covered $41.20 of usage at API rates this month')
  expect(planName('openai')).toBe('ChatGPT')
  expect(planName('acme')).toBe('acme')
})
