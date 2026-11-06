import { expect, test } from 'vitest'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { contextLabel, favoriteTarget, ModelPicker, type View, viewFor, withExtra } from './ModelPicker'

const model = (provider: string, id: string, name: string, extra: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  name,
  provider,
  context: 0,
  reasoning: false,
  image: false,
  efforts: [],
  budget_min: null,
  budget_max: null,
  local: false,
  ...extra,
})

const models = [
  model('anthropic', 'anthropic/claude-opus-4-1', 'Claude Opus 4.1', { context: 200_000 }),
  model('anthropic', 'anthropic/claude-sonnet-4-5', 'Claude Sonnet 4.5', { context: 200_000, image: true }),
  model('openai', 'openai/gpt-5.4', 'GPT-5.4', { context: 400_000 }),
  model('openai', 'openai/gpt-5.4-mini', 'GPT-5.4 mini'),
]
const values = (v: View) => v.rows.map((r) => r.value)

test('providers level: Current row, then providers A→Z with counts', () => {
  const v = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '' })
  expect(v.mode).toBe('providers')
  expect(values(v)).toEqual(['current:openai/gpt-5.4', 'provider:anthropic', 'provider:openai'])
  expect(v.rows[0]).toMatchObject({ kind: 'current', name: 'GPT-5.4', provider: 'openai' })
  expect(v.rows[1]).toMatchObject({ kind: 'provider', count: 2, current: false })
  expect(v.rows[2]).toMatchObject({ kind: 'provider', count: 2, current: true })
  // No current model: no Current row; an unknown current id shows its raw id.
  expect(values(viewFor({ models, model: undefined, provider: null, query: '' }))).toEqual(['provider:anthropic', 'provider:openai'])
  expect(viewFor({ models, model: 'gone/x', provider: null, query: '' }).rows[0]).toMatchObject({ kind: 'current', name: 'gone/x', provider: undefined })
})

test('models level: back row, the provider’s models by name, the current one checked', () => {
  const v = viewFor({ models, model: 'openai/gpt-5.4-mini', provider: 'openai', query: '' })
  expect(v.mode).toBe('models')
  expect(v.provider).toBe('openai')
  expect(values(v)).toEqual(['back', 'model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  expect(v.rows[2]).toMatchObject({ kind: 'model', current: true })
})

test('search mode: Current row first even when it does not match, never duplicated', () => {
  const v = viewFor({ models, model: 'anthropic/claude-opus-4-1', provider: 'openai', query: 'gpt' })
  expect(v.mode).toBe('search')
  expect(values(v)).toEqual(['current:anthropic/claude-opus-4-1', 'model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  // The current model matches: it stays the Current row only.
  const self = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: 'gpt-5.4' })
  expect(values(self)).toEqual(['current:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  // Nothing matches: the empty row.
  const none = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: 'xyz' })
  expect(values(none)).toEqual(['current:openai/gpt-5.4', 'empty'])
  // Whitespace only is browse mode; the remembered provider level comes back.
  expect(viewFor({ models, model: 'openai/gpt-5.4', provider: 'openai', query: '  ' }).mode).toBe('models')
})

test('§3.5 single provider: opens on its models with no back row', () => {
  const one = models.filter((m) => m.provider === 'openai')
  const v = viewFor({ models: one, model: 'openai/gpt-5.4', provider: null, query: '' })
  expect(v.mode).toBe('models')
  expect(v.provider).toBe('openai')
  expect(values(v)).toEqual(['model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  expect(viewFor({ models: one, model: undefined, provider: null, query: 'mini' }).mode).toBe('search')
  expect(viewFor({ models: [], model: undefined, provider: null, query: '' })).toEqual({ mode: 'providers', provider: null, rows: [] })
})

test('forms: the none row sits under Current and is checked when nothing is set; extra ids live under `other`', () => {
  const v = viewFor({ models, model: undefined, provider: null, query: '', none: { label: 'Engine default' }, extra: ['legacy/x', 'openai/gpt-5.4'] })
  expect(values(v)).toEqual(['none', 'provider:anthropic', 'provider:openai', 'provider:other'])
  expect(v.rows[0]).toMatchObject({ kind: 'none', label: 'Engine default', current: true })
  expect(v.rows[3]).toMatchObject({ kind: 'provider', id: 'other', count: 1 })
  const other = viewFor({ models, model: 'legacy/x', provider: 'other', query: '', extra: ['legacy/x'] })
  expect(values(other)).toEqual(['back', 'model:legacy/x'])
  expect(other.rows[1]).toMatchObject({ kind: 'model', current: true, m: { name: 'legacy/x', provider: 'other' } })
  const set = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '', none: { label: 'Off' } })
  expect(set.rows[1]).toMatchObject({ kind: 'none', current: false })
  // Search ranks the extra ids too.
  expect(values(viewFor({ models, model: undefined, provider: null, query: 'legacy', extra: ['legacy/x'] }))).toEqual(['model:legacy/x'])
})

test('127 favorites: first, in starred order, catalog-only; the Current row only when it is not a favorite', () => {
  const favorites = ['openai/gpt-5.4-mini', 'gone/x', 'anthropic/claude-opus-4-1']
  // Current is a favorite: it shows once, as its favorite row (checked), no Current lead row.
  const inFav = viewFor({ models, model: 'anthropic/claude-opus-4-1', provider: null, query: '', favorites })
  expect(values(inFav)).toEqual(['fav:openai/gpt-5.4-mini', 'fav:anthropic/claude-opus-4-1', 'provider:anthropic', 'provider:openai'])
  expect(inFav.rows[0]).toMatchObject({ kind: 'favorite', current: false, m: { name: 'GPT-5.4 mini' } })
  expect(inFav.rows[1]).toMatchObject({ kind: 'favorite', current: true })
  // Current is not a favorite: the Current row follows the favorites, before `none` and the providers.
  const notFav = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '', favorites, none: { label: 'Off' } })
  expect(values(notFav)).toEqual(['fav:openai/gpt-5.4-mini', 'fav:anthropic/claude-opus-4-1', 'current:openai/gpt-5.4', 'none', 'provider:anthropic', 'provider:openai'])
  // No favorites / none in the catalog: 124's layout.
  expect(values(viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '', favorites: ['gone/x'] }))).toEqual(['current:openai/gpt-5.4', 'provider:anthropic', 'provider:openai'])
  // Models level and search rows are marked, never re-ranked; no favorite rows there.
  const level = viewFor({ models, model: undefined, provider: 'openai', query: '', favorites })
  expect(level.rows.map((r) => (r.kind === 'model' ? r.favorite : r.kind))).toEqual(['back', false, true])
  const search = viewFor({ models, model: undefined, provider: null, query: 'claude', favorites })
  expect(search.rows.map((r) => (r.kind === 'model' ? [r.m.id, r.favorite] : r.kind))).toEqual([
    ['anthropic/claude-opus-4-1', true],
    ['anthropic/claude-sonnet-4-5', false],
  ])
})

test('favoriteTarget: Alt+Enter toggles model and favorite rows only', () => {
  expect(favoriteTarget('model:openai/gpt-5.4')).toBe('openai/gpt-5.4')
  expect(favoriteTarget('fav:anthropic/claude-opus-4-1')).toBe('anthropic/claude-opus-4-1')
  expect(favoriteTarget('provider:openai')).toBeNull()
  expect(favoriteTarget('effort:high')).toBeNull()
  expect(favoriteTarget('')).toBeNull()
})

test('contextLabel: thousands, empty when unknown', () => {
  expect(contextLabel(200_000)).toBe('200k')
  expect(contextLabel(1_047_576)).toBe('1048k')
  expect(contextLabel(0)).toBe('')
})

test('177 U4: local models and their providers carry the "local" tag; `localFirst` (summary pickers) lists local providers and local search hits first', () => {
  const local = [...models, model('mlx', 'mlx/LFM3-1.7B-MLX-bf16', 'LFM3 1.7B', { local: true }), model('ollama', 'ollama/gpt-oss:20b', 'gpt-oss 20B', { local: true })]
  expect(values(viewFor({ models: local, model: undefined, provider: null, query: '' }))).toEqual(['provider:anthropic', 'provider:mlx', 'provider:ollama', 'provider:openai'])
  expect(values(viewFor({ models: local, model: undefined, provider: null, query: '', localFirst: true }))).toEqual(['provider:mlx', 'provider:ollama', 'provider:anthropic', 'provider:openai'])
  const hits = (localFirst: boolean) => values(viewFor({ models: local, model: undefined, provider: null, query: 'gpt', localFirst }))
  expect(hits(true)[0]).toBe('model:ollama/gpt-oss:20b')
  expect(hits(true).slice(1).sort()).toEqual(hits(false).filter((v) => v !== 'model:ollama/gpt-oss:20b').sort())
  // An `extra` (stale on-file) id is never local.
  expect(withExtra([], ['x/gone'])[0]!.local).toBe(false)
  const tags = (html: string) => html.match(/>local</g)?.length ?? 0
  const providers = renderToStaticMarkup(createElement(ModelPicker, { models: local, model: undefined, onSelect() {} }))
  expect(tags(providers)).toBe(2)
  expect(providers).toMatch(/>mlx<\/span><span[^>]*>local</)
  // One provider opens on its models: each local row is tagged, a cloud one is not.
  expect(tags(renderToStaticMarkup(createElement(ModelPicker, { models: local.filter((m) => m.provider === 'mlx'), model: undefined, onSelect() {} })))).toBe(1)
  expect(tags(renderToStaticMarkup(createElement(ModelPicker, { models: models.filter((m) => m.provider === 'openai'), model: undefined, onSelect() {} })))).toBe(0)
  // R1-10: the Current row and a favourite row tag a local model too (plus the two local provider rows).
  const lead = renderToStaticMarkup(createElement(ModelPicker, { models: local, model: 'mlx/LFM3-1.7B-MLX-bf16', favorites: ['ollama/gpt-oss:20b'], onToggleFavorite() {}, onSelect() {} }))
  expect(lead).toMatch(/>LFM3 1\.7B<\/span><span[^>]*>local</)
  expect(lead).toMatch(/>gpt-oss 20B<\/span><span[^>]*>local</)
  expect(tags(lead)).toBe(4)
})
