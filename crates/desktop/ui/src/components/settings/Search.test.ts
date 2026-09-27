import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ProviderSettings } from '@/rpc/bindings/ProviderSettings'
import type { SearchSettings } from '@/rpc/bindings/SearchSettings'
import { Search } from './Search'
import { isDirty, moveProvider, providerState, strip, validate } from './search-settings'

const calls: string[] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...await original<typeof Rpc>(),
  useApi: () => ({
    search_settings_get: async () => {
      calls.push('search_settings_get')
      return settings
    },
    search_status: async () => {
      calls.push('search_status')
      return [
        { name: 'google', external: false, searches: true, ok: false, kind: 'rate_limited', detail: 'HTTP 429', last_ok_at: null, last_error_at: '2026-09-27T12:00:00Z' },
        { name: 'searxng', external: true, searches: true, ok: true, kind: '', detail: '12 rows', last_ok_at: '2026-09-27T12:00:01Z', last_error_at: null },
        { name: 'scrape', external: true, searches: false, ok: null, kind: '', detail: '', last_ok_at: null, last_error_at: null },
      ]
    },
  }),
}))

function provider(name: string, patch: Partial<ProviderSettings> = {}): ProviderSettings {
  return {
    name,
    enabled: false,
    base_url: name === 'brave_api' ? null : '',
    secret: name === 'searxng' ? null : name === 'brave_api' ? 'brave' : name,
    secret_set: false,
    timeout_ms: 6000,
    priority: 10,
    engines: name === 'openserp' ? ['google', 'bing'] : null,
    registered: false,
    ...patch,
  }
}

const settings: SearchSettings = {
  providers: [
    provider('searxng', { priority: 10, enabled: true, base_url: 'http://127.0.0.1:8089', registered: true }),
    provider('openserp', { priority: 20 }),
    provider('firecrawl', { priority: 30, enabled: true, base_url: 'https://api.firecrawl.dev' }),
    provider('brave_api', { priority: 40, secret_set: true }),
  ],
  defaults: { read_top: 5, read_budget_ms: 6000, render_cap_ms: 3000, recent_default: null, keyless_fallback: true, degraded_is_error: false },
}

test('moveProvider swaps neighbours and renumbers priorities; ends are no-ops', () => {
  const up = moveProvider(settings.providers, 'firecrawl', -1)
  expect(up.map((p) => p.name)).toEqual(['searxng', 'firecrawl', 'openserp', 'brave_api'])
  expect(up.map((p) => p.priority)).toEqual([10, 20, 30, 40])
  expect(moveProvider(settings.providers, 'searxng', -1).map((p) => p.name)).toEqual(['searxng', 'openserp', 'firecrawl', 'brave_api'])
  expect(moveProvider(settings.providers, 'brave_api', 1).map((p) => p.name)).toEqual(['searxng', 'openserp', 'firecrawl', 'brave_api'])
})

test('validate mirrors the daemon bounds and isDirty ignores read-only fields', () => {
  expect(validate(settings)).toBeNull()
  expect(validate({ ...settings, providers: [provider('searxng', { enabled: true })] })).toBe('SearXNG: enabled without an instance URL')
  expect(validate({ ...settings, providers: [provider('openserp', { base_url: 'ftp://x' })] })).toContain('http://')
  expect(validate({ ...settings, providers: [provider('brave_api', { timeout_ms: 10 })] })).toContain('timeout')
  expect(validate({ ...settings, defaults: { ...settings.defaults, read_top: 9 } })).toBe('Read count must be 0..8')
  expect(isDirty(settings, settings)).toBe(false)
  const flags = { ...settings, providers: settings.providers.map((p) => ({ ...p, secret_set: !p.secret_set, registered: !p.registered })) }
  expect(isDirty(settings, flags)).toBe(false)
  expect(isDirty(settings, { ...settings, defaults: { ...settings.defaults, read_top: 3 } })).toBe(true)
  expect(strip(flags).providers.every((p) => !p.secret_set && !p.registered)).toBe(true)
})

test('providerState explains why a card is not registered', () => {
  expect(providerState(provider('brave_api'))).toBe('Disabled')
  expect(providerState(provider('brave_api', { enabled: true }))).toBe('Key "brave" not set')
  expect(providerState(provider('searxng', { enabled: true }))).toBe('No instance URL')
  expect(providerState(provider('firecrawl', { enabled: true, secret_set: true }))).toBe('Not registered yet (save to apply)')
  expect(providerState(provider('searxng', { enabled: true, base_url: 'http://x', registered: true }))).toBe('Registered')
})

test('pane renders provider cards, key status and health from the RPCs without showing secret values', async () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const render = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Search) as ReactNode }))
  // Status mounts only once the settings answered: two render/fetch passes reach every query the pane registers.
  for (let pass = 0; pass < 2; pass++) {
    render()
    for (const query of qc.getQueryCache().findAll()) {
      await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
    }
  }
  const html = render()
  expect(calls).toContain('search_settings_get')
  expect(calls).toContain('search_status')
  expect(html).toContain('data-provider="searxng"')
  expect(html).toContain('Brave Search API')
  expect(html).toContain('Registered')
  expect(html).toContain('Key &quot;firecrawl&quot; not set')
  expect(html).toContain('Set key')
  expect(html).toContain('Replace key')
  expect(html).not.toContain('type="password"')
  expect(html).toContain('rate_limited')
  expect(html).toContain('12 rows')
  expect(html).not.toContain('</span>scrape')
  expect(html).toContain('</span>searxng')
  expect(html).toContain('Run a test search')
})
