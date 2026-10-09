import { expect, test, vi } from 'vitest'
import type { Notebook } from '@/rpc/bindings/Notebook'
import { scopeOrder } from './AskDialog'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/store/connection', () => ({ useConnection: () => null }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))

const nb = (id: string): Notebook => ({ doc: { id, title: id, kind: 'notebook', binding: 'notebook', created_ms: 1n, updated_ms: 1n, bytes: 0n }, project: null, archived_ms: null, sources: 0, notes: 0, chats: 0 })

test('the scope lists the current notebook first; without one (Library, /ask outside a notebook) the list order stands', () => {
  const all = [nb('a'), nb('b'), nb('c')]
  expect(scopeOrder(all, 'b').map((n) => n.doc.id)).toEqual(['b', 'a', 'c'])
  expect(scopeOrder(all, null).map((n) => n.doc.id)).toEqual(['a', 'b', 'c'])
  expect(scopeOrder(all, 'gone').map((n) => n.doc.id)).toEqual(['a', 'b', 'c'])
})
