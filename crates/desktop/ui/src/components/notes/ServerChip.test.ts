// 184 §12.1: the note chip and banner. Static markup; `useMesh` is a mock.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { EngineProvider } from '@/lib/doc/provider'
import { ServerBanner, ServerChip, serverView } from './ServerChip'

const peer = (id: string, name: string) => ({ id, name, addrs: [], reachable: true, last_seen_ms: 0 }) as never
const mesh = { self_id: 'S', self_name: 'studio-mac', peers: [peer('X', 'xeus')] }
let current: typeof mesh | { self_id: string; self_name: string; peers: never[] } = mesh
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: current }) }))

const provider = (p: Partial<{ kind: string; home: string | null; server: string | null; dual: string | null; status: string }>) =>
  ({
    meta: { kind: p.kind ?? 'note', home: p.home === undefined ? 'X' : p.home },
    server: p.server === undefined ? 'X' : p.server,
    dual: p.dual ?? null,
    status: { kind: p.status ?? 'live' },
    onEvent: () => () => {},
    onStatus: () => () => {},
  }) as unknown as EngineProvider
const html = (p: EngineProvider) => renderToStaticMarkup(createElement('div', null, createElement(ServerChip, { provider: p }), createElement(ServerBanner, { provider: p })))

test('on its home: green chip, tooltip, no banner', () => {
  const h = html(provider({}))
  expect(h).toContain('bg-success')
  expect(h).toContain('On xeus')
  expect(h).toContain('title="This note lives on xeus; other machines edit it through it."')
  expect(h).not.toContain('role="status"')
})

test('unhomed: the chip names the holder', () => {
  expect(html(provider({ home: null, server: 'S' }))).toContain('On studio-mac')
})

test('home offline: grey chip with `· offline` and the warning banner', () => {
  const h = html(provider({ server: 'S' }))
  expect(h).toContain('bg-fg-tertiary')
  expect(h).toContain('On xeus · offline')
  expect(h).toContain('<strong>xeus is offline</strong> — editing on studio-mac for now. Your changes merge back when xeus returns.')
})

test('dual: green chip and the Also open on banner', () => {
  const h = html(provider({ dual: 'S' }))
  expect(h).toContain('bg-success')
  expect(h).toContain('<strong>Also open on studio-mac</strong> — edits merge within seconds.')
})

test('boards and meeting notes get the chip; other kinds, a closed document, a null server and a lone engine get nothing', () => {
  expect(html(provider({ kind: 'board' }))).toContain('On xeus')
  expect(html(provider({ kind: 'meeting_note' }))).toContain('On xeus')
  expect(html(provider({ kind: 'task_thread' }))).toBe('<div></div>')
  expect(html(provider({ status: 'closed' }))).toBe('<div></div>')
  expect(html(provider({ server: null }))).toBe('<div></div>')
  current = { self_id: 'S', self_name: 'studio-mac', peers: [] }
  expect(html(provider({ server: 'S' }))).toBe('<div></div>')
  current = mesh
})

test('names, never ids: an unknown id shows its first 8 characters', () => {
  const v = serverView(mesh, 'note', 'abcdef0123456789', 'S', null)
  expect(v?.name).toBe('abcdef01')
  expect(v?.banner?.bold).toBe('abcdef01 is offline')
})

test('a 60-char machine name truncates and the full name is in the title', () => {
  const long = 'm'.repeat(60)
  current = { self_id: 'S', self_name: 'studio-mac', peers: [peer('X', long)] }
  const h = html(provider({}))
  expect(h).toContain('truncate')
  expect(h).toContain(`title="This note lives on ${long}; other machines edit it through it."`)
  current = mesh
})
