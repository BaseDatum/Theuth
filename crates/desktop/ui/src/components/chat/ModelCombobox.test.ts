import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { ModelCombobox, triggerLabel } from './ModelCombobox'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({}),
}))

const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', efforts: ['low', 'medium', 'high'] } as ModelInfo

test('148: the trigger reads "name · effort" when an effort is set; the id, then `none`, when the catalog has no name', () => {
  expect(triggerLabel(sonnet, sonnet.id, undefined, null)).toBe('Claude Sonnet 4.5')
  expect(triggerLabel(sonnet, sonnet.id, undefined, 'high')).toBe('Claude Sonnet 4.5 · high')
  expect(triggerLabel(undefined, 'x/gone', { label: 'Off' }, 'low')).toBe('x/gone · low')
  expect(triggerLabel(undefined, null, { label: 'Same as new sessions' }, undefined)).toBe('Same as new sessions')
  expect(triggerLabel(undefined, undefined, undefined, undefined)).toBe('Model')
})

test('148: the rendered trigger shows the effort passed through `effort.current`; without the prop, the name alone (existing callers)', () => {
  const html = (effort?: { levels: string[]; current: string | null | undefined; onSelect(l: string | null): void }) =>
    renderToStaticMarkup(createElement(ModelCombobox, { models: [sonnet], value: sonnet.id, onChange() {}, effort, 'aria-label': 'New sessions model' }))
  expect(html({ levels: sonnet.efforts, current: 'high', onSelect() {} })).toContain('Claude Sonnet 4.5 · high</span>')
  expect(html()).toContain('Claude Sonnet 4.5</span>')
  expect(html()).not.toContain(' · ')
})
