import { QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import type { DocumentChanged } from "@/rpc/bindings/event.document_2echanged.output"
import { dispatch } from '@/store/sessions'
import { createNote } from './create'

vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))

const row = (id: string, title = 'Old'): DocSummary => ({ id, kind: 'note', title, binding: 'note', created_ms: 1, updated_ms: 1, bytes: 0 })
const changed = (doc: string, over: Partial<DocumentChanged> = {}): DocumentChanged => ({
  type: 'DocumentChanged',
  doc,
  kind: 'note',
  title: 'Untitled',
  updated_ms: 5,
  deleted: false,
  ...over,
})

/** What New note does with the id: the list's `onSuccess` navigates to `/notes/$id?new`. */
const navigate = vi.fn()
const open = (n: DocSummary) => navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } })

let qc: QueryClient
let reply: PromiseWithResolvers<DocSummary>
const api = { doc_create: vi.fn(() => reply.promise) }

beforeEach(() => {
  qc = new QueryClient()
  qc.setQueryData(queryKeys.notes, [row('old')])
  reply = Promise.withResolvers<DocSummary>()
  navigate.mockClear()
  vi.mocked(toast.error).mockClear()
})

test('the bus event opens the note before the body import replies, and lists it at once', async () => {
  const p = createNote(api, qc, qc.getQueryData(queryKeys.notes)!).then(open)
  expect(api.doc_create).toHaveBeenCalledWith('Untitled', null)
  dispatch([{key: 'document.changed@1', event: changed('old', { title: 'Old' })}]) // an export of a listed note is not ours
  dispatch([{key: 'document.changed@1', event: changed('agent', { title: 'Plan' })}]) // nor a note the agent titled
  dispatch([{key: 'document.changed@1', event: changed('fresh')}])
  await p
  expect(navigate).toHaveBeenCalledWith({ to: '/notes/$id', params: { id: 'fresh' }, search: { new: true } })
  expect(qc.getQueryData<DocSummary[]>(queryKeys.notes)!.map((n) => n.id)).toEqual(['fresh', 'old'])
  // The reply failing afterwards (the helper) reports the import but leaves the note open. `createNote`'s
  // catch was attached first, so it has run once ours does.
  reply.reject(new Error('documents helper not found'))
  await reply.promise.catch(() => undefined)
  expect(toast.error).toHaveBeenCalledWith('documents helper not found')
  expect(navigate).toHaveBeenCalledTimes(1)
})

test('the reply alone opens the note when no event arrived', async () => {
  const p = createNote(api, qc, [row('old')]).then(open)
  reply.resolve(row('fresh', 'Untitled'))
  await p
  expect(navigate).toHaveBeenCalledWith({ to: '/notes/$id', params: { id: 'fresh' }, search: { new: true } })
  expect(qc.getQueryData<DocSummary[]>(queryKeys.notes)!.map((n) => n.id)).toEqual(['fresh', 'old'])
  // A later event for the same note does not add a second row or re-open it.
  dispatch([{key: 'document.changed@1', event: changed('fresh')}])
  expect(qc.getQueryData<DocSummary[]>(queryKeys.notes)).toHaveLength(2)
  expect(navigate).toHaveBeenCalledTimes(1)
})

test('a create that fails before the note exists rejects and opens nothing', async () => {
  const p = createNote(api, qc, [row('old')]).then(open)
  reply.reject(new Error('not connected'))
  await expect(p).rejects.toThrow('not connected')
  expect(navigate).not.toHaveBeenCalled()
  expect(toast.error).not.toHaveBeenCalled()
  expect(qc.getQueryData<DocSummary[]>(queryKeys.notes)).toHaveLength(1)
})
