import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import { Connections } from './Connections'
import { BUILTIN_LABEL, builtinKeyNote, orderProviders, refreshedText } from './providers'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))

const row = (name: string, builtin: boolean, configured: boolean, refreshable = false): ProviderInfo => ({
  name,
  base_url: `https://${name}/v1/`,
  models: 1,
  builtin,
  configured,
  refreshable,
})

const render = (providers: ProviderInfo[]) => {
  const qc = new QueryClient()
  qc.setQueryData(queryKeys.providers, providers)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(Connections)))
}

test('built-ins come first, custom providers after, engine order kept within each', () => {
  const list = orderProviders([row('a', false, true), row('opencode', true, false), row('rwx', false, true), row('openrouter', true, true)])
  expect(list.map((p) => p.name)).toEqual(['opencode', 'openrouter', 'a', 'rwx'])
  expect(orderProviders(undefined)).toEqual([])
})

test('refresh toast: counts on a change, "no change" when nothing moved', () => {
  const provider = { ...row('rwx', false, true, true), models: 14 }
  expect(refreshedText({ provider, added: ['m1', 'm2'], removed: ['m0'] })).toBe('rwx: 14 models (+2, −1)')
  expect(refreshedText({ provider, added: [], removed: ['m0'] })).toBe('rwx: 14 models (+0, −1)')
  expect(refreshedText({ provider, added: [], removed: [] })).toBe('rwx: 14 models, no change')
})

test('one Providers section: rows in order with tags, Refresh models only where refreshable, no "gateway"', () => {
  const html = render([row('opencode', true, true, true), row('openrouter', true, false, false), row('rwx', false, true, true)])
  const text = html.replace(/<[^>]+>/g, ' ')
  expect(text).toContain('Providers')
  expect(text.indexOf('OpenCode Zen')).toBeLessThan(text.indexOf('OpenRouter'))
  expect(text.indexOf('OpenRouter')).toBeLessThan(text.indexOf('rwx'))
  expect(text.match(/built-in/g)).toHaveLength(2)
  expect(text).toContain('custom')
  expect(text).toContain('https://rwx/v1/')
  expect(text).toContain('key set')
  expect(text).toContain('no key (secret `openrouter`)')
  // opencode and rwx are refreshable; openrouter (no key) is not.
  expect(text.match(/Refresh models/g)).toHaveLength(2)
  expect(html).toContain('aria-label="Remove rwx"')
  expect(html).not.toContain('aria-label="Remove opencode"')
  expect(html).toContain('aria-label="Custom provider name"')
  expect(text).not.toContain('No custom providers yet.')
  expect(html).not.toMatch(/gateway/i)
})

test('no custom provider: the muted empty line', () => {
  const html = render([row('opencode', true, false), row('openrouter', true, false)])
  expect(html).toContain('No custom providers yet.')
  expect(html).not.toContain('Refresh models')
  expect(html).not.toMatch(/gateway/i)
})

test('built-in labels and key notes', () => {
  expect(BUILTIN_LABEL.opencode).toBe('OpenCode Zen')
  expect(builtinKeyNote('openrouter')).toBe('OpenRouter key (Settings > Connections)')
  expect(builtinKeyNote('zen')).toBe('zen key (Settings > Connections)')
})
