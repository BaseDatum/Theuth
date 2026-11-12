import '../settings/test-dom'
import { afterEach, expect, test, vi } from 'vitest'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { contextLabel, type Effort, favoriteTarget, ModelPicker, type ModelPickerProps, type View, viewFor, withExtra } from './ModelPicker'

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

test('177 U4: local models and their providers carry the "local" tag; 194 D5: one ordering for every picker, local providers in their alphabetical place', () => {
  const local = [...models, model('mlx', 'mlx/LFM3-1.7B-MLX-bf16', 'LFM3 1.7B', { local: true }), model('ollama', 'ollama/gpt-oss:20b', 'gpt-oss 20B', { local: true })]
  expect(values(viewFor({ models: local, model: undefined, provider: null, query: '' }))).toEqual(['provider:anthropic', 'provider:mlx', 'provider:ollama', 'provider:openai'])
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

// ── 2026-11-11: the effort chips under the selected model's row ──────────────────────────────────────────────────

window.HTMLElement.prototype.scrollIntoView = () => {}
afterEach(() => {
  document.body.innerHTML = ''
})

const withEfforts = models.map((m) => (m.id === 'openai/gpt-5.4' ? { ...m, efforts: ['low', 'medium', 'high'] } : m))
const effortOn = (current: string | null, extra: Omit<Partial<Effort>, 'onSelect'> = {}) => ({ levels: ['low', 'medium', 'high'], current, onSelect: vi.fn(), ...extra })

async function mount(props: Partial<ModelPickerProps>) {
  const host = document.createElement('div')
  document.body.append(host)
  const onSelect = vi.fn()
  await act(async () => createRoot(host).render(createElement(ModelPicker, { models: withEfforts, model: 'openai/gpt-5.4', onSelect, ...props })))
  const item = (value: string) => host.querySelector<HTMLElement>(`[cmdk-item][data-value="${value}"]`)
  /** The chip row directly under `row` (its next sibling) as labels, the active one starred; `null` when none follows. */
  const chipsAfter = (row: Element | null | undefined) => {
    const g = row?.nextElementSibling
    return g?.getAttribute('aria-label') === 'Effort' ? [...g.children].map((c) => `${c.textContent}${c.querySelector('svg') ? '*' : ''}`) : null
  }
  const chip = (label: string) => [...host.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')].find((c) => c.textContent === label)!
  const lead = (text: string) => [...host.querySelectorAll('button')].find((b) => b.textContent === text)
  const values = () => [...host.querySelectorAll('[cmdk-item]')].map((e) => e.getAttribute('data-value'))
  const selected = () => host.querySelector('[cmdk-item][data-selected="true"]')?.getAttribute('data-value')
  const input = host.querySelector('input')!
  const key = (k: string) => act(async () => void input.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true })))
  const type = (text: string) =>
    act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, text)
      input.dispatchEvent(new Event('input', { bubbles: true }))
    })
  const click = (el: HTMLElement) => act(async () => void el.click())
  return { host, onSelect, item, chipsAfter, chip, lead, values, selected, key, type, click }
}

test('chips: one row directly under the Current row (plain buttons), Default first, the active level marked; no bottom Effort group; the highlight still starts on the first provider', async () => {
  const effort = effortOn('high')
  const p = await mount({ effort })
  expect(p.chipsAfter(p.lead('GPT-5.4openai'))).toEqual(['Default', 'low', 'medium', 'high*'])
  expect(p.host.querySelectorAll('[aria-label="Effort"]')).toHaveLength(1)
  expect([...p.host.querySelectorAll('[cmdk-group-heading]')].map((h) => h.textContent)).toEqual(['Providers'])
  expect(p.host.textContent).not.toContain('clear')
  expect(p.values()).toEqual(['provider:anthropic', 'provider:openai'])
  expect(p.selected()).toBe('provider:anthropic')
  expect(p.chip('high').getAttribute('aria-pressed')).toBe('true')
  await p.click(p.chip('medium'))
  await p.click(p.chip('Default'))
  expect(effort.onSelect.mock.calls).toEqual([['medium'], [null]])
  expect(p.onSelect).not.toHaveBeenCalled()
})

test('chips under a favorite row are cmdk items in DOM order right after it: ↓ reaches them, Enter applies', async () => {
  const effort = effortOn(null)
  const p = await mount({ effort, favorites: ['openai/gpt-5.4-mini', 'openai/gpt-5.4'], onToggleFavorite() {} })
  expect(p.chipsAfter(p.item('fav:openai/gpt-5.4'))).toEqual(['Default*', 'low', 'medium', 'high'])
  expect(p.chipsAfter(p.item('fav:openai/gpt-5.4-mini'))).toBeNull()
  expect(p.values()).toEqual(['fav:openai/gpt-5.4-mini', 'fav:openai/gpt-5.4', 'effort-default', 'effort:low', 'effort:medium', 'effort:high', 'provider:anthropic', 'provider:openai'])
  expect(p.selected()).toBe('fav:openai/gpt-5.4-mini')
  await p.key('ArrowDown')
  await p.key('ArrowDown')
  await p.key('ArrowDown')
  expect(p.selected()).toBe('effort:low')
  await p.key('Enter')
  expect(effort.onSelect).toHaveBeenCalledWith('low')
  expect(p.host.querySelectorAll('[aria-label="Effort"]')).toHaveLength(1)
})

test('chips inside the provider’s model list: under the current model’s row, before the next model', async () => {
  const effort = effortOn('medium')
  const p = await mount({ effort })
  await p.key('ArrowDown')
  await p.key('Enter') // provider:openai
  expect(p.chipsAfter(p.item('model:openai/gpt-5.4'))).toEqual(['Default', 'low', 'medium*', 'high'])
  expect(p.chipsAfter(p.item('model:openai/gpt-5.4-mini'))).toBeNull()
  expect(p.values()).toEqual(['model:openai/gpt-5.4', 'effort-default', 'effort:low', 'effort:medium', 'effort:high', 'model:openai/gpt-5.4-mini'])
  // Another provider's list has no selected row, so no chips.
  await p.key('Backspace')
  await p.click(p.item('provider:anthropic')!)
  expect(p.values()).toEqual(['model:anthropic/claude-opus-4-1', 'model:anthropic/claude-sonnet-4-5'])
  expect(p.host.querySelector('[aria-label="Effort"]')).toBeNull()
})

test('chips in search: under the Current row that leads the results; the highlight starts on the first result and Enter picks it', async () => {
  const effort = effortOn('low')
  const p = await mount({ effort })
  await p.type('mini')
  expect(p.chipsAfter(p.lead('GPT-5.4openai'))).toEqual(['Default', 'low*', 'medium', 'high'])
  expect(p.values()).toEqual(['model:openai/gpt-5.4-mini'])
  expect(p.selected()).toBe('model:openai/gpt-5.4-mini')
  await p.key('Enter')
  expect(p.onSelect).toHaveBeenCalledWith('openai/gpt-5.4-mini')
  expect(effort.onSelect).not.toHaveBeenCalled()
})

test('no chips without levels; `required` drops Default and `labels` name the levels; a selected none row carries them', async () => {
  expect((await mount({ effort: effortOn(null, { levels: [] }) })).host.querySelector('[aria-label="Effort"]')).toBeNull()
  expect((await mount({ model: 'openai/gpt-5.4-mini' })).host.querySelector('[aria-label="Effort"]')).toBeNull()
  const harness = await mount({ effort: effortOn('high', { levels: ['off', 'high', 'xhigh'], labels: { off: 'Off', high: 'High', xhigh: 'Extra high' }, required: true }) })
  expect(harness.chipsAfter(harness.lead('GPT-5.4openai'))).toEqual(['Off', 'High*', 'Extra high'])
  const none = await mount({ model: undefined, none: { value: '__none__', label: 'Node default' }, effort: effortOn('high') })
  expect(none.chipsAfter(none.lead('Node default'))).toEqual(['Default', 'low', 'medium', 'high*'])
})
