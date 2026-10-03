import { expect, test } from 'vitest'
import type { ProviderInfo } from '@/rpc/extra'
import { BUILTIN_LABEL, builtinKeyNote, splitProviders } from './providers'

const row = (name: string, builtin: boolean, configured: boolean): ProviderInfo => ({
  name,
  base_url: `https://${name}/v1/`,
  models: 1,
  builtin,
  configured,
})

test('built-ins go to their own table, gateways keep engine order', () => {
  const { builtins, gateways } = splitProviders([
    row('opencode', true, false),
    row('openrouter', true, true),
    row('a', false, true),
    row('rwx', false, true),
  ])
  expect(builtins.map((p) => p.name)).toEqual(['opencode', 'openrouter'])
  expect(builtins.map((p) => p.configured)).toEqual([false, true])
  expect(gateways.map((p) => p.name)).toEqual(['a', 'rwx'])
  expect(splitProviders(undefined)).toEqual({ builtins: [], gateways: [] })
})

test('built-in labels and key notes', () => {
  expect(BUILTIN_LABEL.opencode).toBe('OpenCode Zen')
  expect(builtinKeyNote('openrouter')).toBe('OpenRouter key (Settings > Connections)')
  expect(builtinKeyNote('zen')).toBe('zen key (Settings > Connections)')
})
