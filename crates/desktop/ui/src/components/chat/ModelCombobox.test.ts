import '../settings/test-dom'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { ModelCombobox, pickerSurface, triggerLabel } from './ModelCombobox'

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

test('148 R2: a coarse pointer opens the picker in the top-of-band dialog, a fine one in the trigger-anchored popover', () => {
  expect(pickerSurface(true)).toBe('dialog')
  expect(pickerSurface(false)).toBe('popover')
})

test('2026-11-11: the open picker shows `effort` as chips under the selected model; a chip applies the level and closes, like a model pick', async () => {
  window.HTMLElement.prototype.scrollIntoView = () => {}
  const host = document.createElement('div')
  document.body.append(host)
  const onSelect = vi.fn()
  await act(async () => createRoot(host).render(createElement(ModelCombobox, { models: [sonnet], value: sonnet.id, onChange() {}, effort: { levels: sonnet.efforts, current: 'high', onSelect }, 'aria-label': 'New sessions model' })))
  await act(async () => void host.querySelector('button')!.click())
  const chips = () => [...document.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')]
  expect(chips().map((c) => c.textContent)).toEqual(['Default', 'low', 'medium', 'high'])
  await act(async () => void chips()[1]!.click())
  expect(onSelect).toHaveBeenCalledWith('low')
  expect(chips()).toEqual([])
})
