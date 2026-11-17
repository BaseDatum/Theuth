import '../settings/test-dom'
import { act, createElement, useState } from 'react'
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

// ── 2026-11-17 (Sky): picking a model with levels keeps the picker open on its chips ─────────────────────────────────

const entry = (id: string, name: string, efforts: string[]): ModelInfo => ({ id, name, provider: id.split('/')[0]!, context: 0, reasoning: efforts.length > 0, image: false, efforts, budget_min: null, budget_max: null, local: false })
const catalog = [entry('anthropic/claude-sonnet-4-5', 'Claude Sonnet 4.5', ['low', 'medium', 'high']), entry('openai/gpt-4.1', 'GPT-4.1', [])]

/** A form row over `catalog` that starts on GPT-4.1 (no levels), opened on its providers level. */
async function form() {
  const onChange = vi.fn()
  const onLevel = vi.fn()
  function Form() {
    const [value, setValue] = useState<string | null>('openai/gpt-4.1')
    const [effort, setEffort] = useState<string | null>(null)
    const levels = catalog.find((m) => m.id === value)?.efforts ?? []
    return createElement(ModelCombobox, {
      models: catalog,
      value,
      onChange: (id) => (onChange(id), setValue(id)),
      effort: levels.length ? { levels, current: effort, onSelect: (l) => (onLevel(l), setEffort(l)) } : undefined,
      'aria-label': 'Model',
    })
  }
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(createElement(Form)))
  const trigger = host.querySelector<HTMLButtonElement>('button[aria-label="Model"]')!
  const click = (sel: string) => act(async () => void document.querySelector<HTMLElement>(sel)!.click())
  const pick = async (id: string) => (await click(`[data-value="provider:${id.split('/')[0]}"]`), await click(`[data-value="model:${id}"]`))
  await act(async () => void trigger.click())
  return { onChange, onLevel, trigger, click, pick, isOpen: () => document.querySelector('[cmdk-root]') !== null }
}
const chipRow = () => [...document.querySelectorAll('[aria-label="Effort"] > *')].map((c) => c.textContent)

test('ModelCombobox: a model with levels keeps the popover open, its chips shown and the active one focused; a chip sets the effort and closes', async () => {
  const f = await form()
  await f.pick('anthropic/claude-sonnet-4-5')
  expect(f.onChange.mock.calls).toEqual([['anthropic/claude-sonnet-4-5']])
  expect(f.isOpen()).toBe(true)
  expect(chipRow()).toEqual(['Default', 'low', 'medium', 'high'])
  expect(document.activeElement?.textContent).toBe('Default')
  await f.click('[aria-label="Effort"] [data-value="effort:high"]')
  expect(f.onLevel.mock.calls).toEqual([['high']])
  expect(f.isOpen()).toBe(false)
  expect(f.trigger.textContent).toBe('Claude Sonnet 4.5 · high')
  expect(f.onChange).toHaveBeenCalledTimes(1)
})

test('ModelCombobox: Escape or a click outside closes it after a pick and keeps the model, no effort applied; a model without levels closes at once', async () => {
  const esc = await form()
  await esc.pick('anthropic/claude-sonnet-4-5')
  await act(async () => void document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
  expect(esc.isOpen()).toBe(false)
  expect([esc.onChange.mock.calls, esc.onLevel.mock.calls, esc.trigger.textContent]).toEqual([[['anthropic/claude-sonnet-4-5']], [], 'Claude Sonnet 4.5'])
  document.body.innerHTML = ''
  const outside = await form()
  await outside.pick('anthropic/claude-sonnet-4-5')
  // Radix arms its outside-pointer listener a tick after opening (retry until armed), and dismisses on the click that follows.
  await vi.waitFor(async () => {
    await act(async () => void ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'].forEach((type) => document.body.dispatchEvent(new MouseEvent(type, { bubbles: true }))))
    expect(outside.isOpen()).toBe(false)
  })
  expect([outside.onChange.mock.calls, outside.onLevel.mock.calls]).toEqual([[['anthropic/claude-sonnet-4-5']], []])
  document.body.innerHTML = ''
  const plain = await form()
  await plain.pick('anthropic/claude-sonnet-4-5')
  // Changing one's mind on the chips: back to the providers, a model without levels.
  await act(async () => void [...document.querySelectorAll('button')].find((b) => b.textContent === 'All providers')!.click())
  await plain.pick('openai/gpt-4.1')
  expect(plain.onChange.mock.calls).toEqual([['anthropic/claude-sonnet-4-5'], ['openai/gpt-4.1']])
  expect(plain.isOpen()).toBe(false)
})
