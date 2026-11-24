// 184 §12.4: Move to ▸. Rows and refusals are pure; the mutation runs through a MutationObserver.
import { MutationObserver, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type { EngineProvider } from '@/lib/doc/provider'
import { MoveNote, moveOptions, moveRefusal, moveRows } from './MoveNote'

const toasts: [string, string][] = []
vi.mock('@/lib/notify', () => ({ notify: Object.assign((m: string) => toasts.push(['ok', m]), { error: (m: string) => toasts.push(['error', m]) }) }))
const now = Date.UTC(2026, 10, 8, 12, 0)
const peer = (id: string, name: string, reachable: boolean, last_seen_ms: number) => ({ id, name, addrs: [], reachable, last_seen_ms }) as never
const mesh = { self_id: 'S', self_name: 'studio-mac', docs_homes_on: true, peers: [peer('X', 'xeus', true, now), peer('M', 'mini', false, now - 5 * 60_000)] }
let current = mesh
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: current, refetch: () => {} }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  DropdownMenuTrigger: ({ children }: { children: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuItem: ({ children, disabled }: { children: ReactNode; disabled?: boolean }) => createElement('div', { role: 'menuitem', 'data-disabled': disabled }, children),
}))
beforeEach(() => {
  toasts.length = 0
  current = mesh
})

test('rows: the home lives here, a reachable machine is enabled, an unreachable one is disabled with its age', () => {
  const rows = moveRows(mesh, 'X', null, now)
  expect(rows.map((r) => [r.name, r.enabled, r.detail])).toEqual([
    ['studio-mac', true, ''],
    ['xeus', false, 'lives here'],
    ['mini', false, 'offline 5m'],
  ])
})

test('pending: every row is disabled and the target says Moving…', () => {
  const rows = moveRows(mesh, 'S', 'X', now)
  expect(rows.every((r) => !r.enabled)).toBe(true)
  expect(rows.find((r) => r.id === 'X')?.detail).toBe('Moving…')
})

test('refusals: boards, meeting notes, homes off; task threads hide; a note may move', () => {
  expect(moveRefusal('board', true)).toBe('Boards stay on the machine whose folder they point at (root:).')
  expect(moveRefusal('meeting_note', true)).toBe('Meeting notes stay on the machine that recorded them.')
  expect(moveRefusal('note', false)).toBe('Notes can move once every machine runs this version.')
  expect(moveRefusal('task_thread', true)).toBe('hide')
  expect(moveRefusal('note', true)).toBeNull()
})

test('choosing a row calls doc_move_home(doc, to) and toasts Moved to <machine>', async () => {
  const calls: unknown[][] = []
  const api = { doc_move_home: async (...a: unknown[]) => void calls.push(a) } as never
  await new MutationObserver(new QueryClient(), moveOptions(api, 'd1')).mutate({ id: 'X', name: 'xeus' })
  expect(calls).toEqual([['d1', 'X']])
  expect(toasts).toEqual([['ok', 'Moved to xeus']])
})

test('an error toasts its text verbatim', async () => {
  const api = { doc_move_home: async () => Promise.reject(new Error('target not reachable')) } as never
  await new MutationObserver(new QueryClient(), moveOptions(api, 'd1')).mutate({ id: 'X', name: 'xeus' }).catch(() => {})
  expect(toasts).toEqual([['error', 'target not reachable']])
})

const html = (kind: string, home: string | null = 'X') =>
  renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient() }, createElement(MoveNote, { provider: { id: 'd1', meta: { kind, home }, server: 'X', dual: null, status: { kind: 'live' }, onEvent: () => () => {}, onStatus: () => () => {} } as unknown as EngineProvider })))

test('markup: a note lists machines; a board is a disabled trigger with the tooltip; a lone engine and task threads show nothing', () => {
  const h = html('note')
  expect(h).toContain('aria-label="Move to"')
  expect(h).not.toContain('disabled=""')
  expect(h).toMatch(/xeus.*lives here.*mini.*offline/s)
  const board = html('board')
  expect(board).toContain('title="Boards stay on the machine whose folder they point at (root:)."')
  expect(board).toContain('disabled=""')
  expect(html('task_thread')).toBe('')
  current = { ...mesh, peers: [] }
  expect(html('note')).toBe('')
})

test('markup: the disabled trigger is described by the refusal text', () => {
  const reason = 'Boards stay on the machine whose folder they point at (root:).'
  const h = html('board')
  expect(h).toContain('aria-describedby="move-refusal"')
  expect(h).toContain(`<span id="move-refusal" class="sr-only">${reason}</span>`)
  expect(html('note')).not.toContain('aria-describedby')
})
