import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { SurfaceContent } from '@/rpc/bindings/method.plugins.output'
import { PluginSurface } from './PluginSurface'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/components/apps/theme', () => ({ useDark: () => false }))

afterEach(() => {
  document.body.innerHTML = ''
})

const content = (over: Partial<SurfaceContent>): SurfaceContent => ({
  plugin: 'turns',
  placement: 'turns',
  revision: 1,
  state: 'ready',
  format: 'markdown',
  source: '',
  text: null,
  bindings: [],
  error: null,
  ...over,
})

async function mount(c: SurfaceContent | undefined) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(PluginSurface, { plugin: 'turns', placement: 'turns', session: null, content: c })))
  return { host, body: () => host.querySelector<HTMLElement>('[data-plugin-content]')!, unmount: () => act(async () => root.unmount()) }
}

test('the attribution strip reads "plugin · turns" and sits outside the content node', async () => {
  const { host, body, unmount } = await mount(content({ source: 'Turns today: **3**' }))
  const strip = host.querySelector<HTMLElement>('[data-plugin-attribution]')!
  expect(strip.textContent).toBe('plugin · turns')
  expect(body().contains(strip)).toBe(false)
  expect(body().textContent).toContain('Turns today: 3')
  expect(body().querySelector('strong, [data-streamdown="strong"]')?.textContent).toBe('3')
  await unmount()
})

test('markdown: raw HTML is not rendered — no input can pose as theuth asking for a secret', async () => {
  const { body, unmount } = await mount(content({ source: 'Your key: <input type="password" name="key"> and <b>bold</b>\n\n<form><input></form>' }))
  expect(body().querySelector('input')).toBeNull()
  expect(body().querySelector('form')).toBeNull()
  expect(body().querySelector('b')).toBeNull()
  expect(body().textContent).toContain('Your key:')
  await unmount()
})

test('json-render renders through the chat renderer', async () => {
  const spec = '{"root":"r","elements":{"r":{"type":"Heading","props":{"text":"Turns today","level":2}}}}'
  const { body, unmount } = await mount(content({ format: 'json-render', source: spec }))
  expect(body().textContent).toContain('Turns today')
  expect(body().querySelector('[data-streamdown="json-render"]')).not.toBeNull()
  await unmount()
})

test('loading and error are host-drawn frames; no source falls back to the text', async () => {
  let m = await mount(undefined)
  expect(m.body().querySelector('[role="status"]')?.textContent).toBe('Loading…')
  await m.unmount()
  m = await mount(content({ state: 'error', source: null, error: 'plugin did not render' }))
  expect(m.body().querySelector('[role="alert"]')?.textContent).toBe('plugin did not render')
  await m.unmount()
  m = await mount(content({ source: null, format: null, text: '3 turns' }))
  expect(m.body().textContent).toBe('3 turns')
  await m.unmount()
})
