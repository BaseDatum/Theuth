import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchResponse } from '@/rpc/bindings/SearchResponse'
import { DEBOUNCE_MS, SUPERSEDED, SearchQueue } from './search-queue'

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

type FakeApi = { sent: string[]; search(query: string): Promise<SearchResponse>; answer(query: string): void }

/** A `search` RPC the test settles by hand, in the order it wants. */
function fakeApi(): FakeApi {
  const sent: string[] = []
  const open: { query: string; resolve(r: SearchResponse): void; reject(e: Error): void }[] = []
  const search = (query: string) =>
    new Promise<SearchResponse>((resolve, reject) => {
      sent.push(query)
      open.push({ query, resolve, reject })
    })
  const answer = (query: string) => {
    const i = open.findIndex((o) => o.query === query)
    if (i < 0) throw new Error(`no open request for ${query}`)
    open.splice(i, 1)[0]?.resolve({ groups: [{ category: 'notes', page: { state: 'ready', coverage: 'complete', hits: [], total: 0, marker: query } }] } as unknown as SearchResponse)
  }
  return { sent, search, answer }
}

const flush = () => vi.advanceTimersByTimeAsync(0)

test('debounce restarts on every keystroke and settles once', async () => {
  const q = new SearchQueue()
  const settled: string[] = []
  for (const n of [1, 2, 3, 4, 5, 6, 7, 8]) {
    q.type('aquarium'.slice(0, n), (s) => settled.push(s))
    await vi.advanceTimersByTimeAsync(30)
  }
  expect(settled).toEqual([])
  await vi.advanceTimersByTimeAsync(DEBOUNCE_MS)
  expect(settled).toEqual(['aquarium'])
  // A cancelled debounce never fires; a cleared queue neither.
  q.type('x', (s) => settled.push(s))
  q.cancel()
  q.type('y', (s) => settled.push(s))
  q.clear()
  await vi.advanceTimersByTimeAsync(DEBOUNCE_MS * 2)
  expect(settled).toEqual(['aquarium'])
})

test('one request in flight; same-query requests queue FIFO; a newer query drops the waiting one', async () => {
  const api = fakeApi()
  const q = new SearchQueue()
  const a = q.run('a', () => api.search('a'))
  const aMore = q.run('a', () => api.search('a'))
  expect(api.sent).toEqual(['a'])
  const ab = q.run('ab', () => api.search('ab'))
  // `ab` replaced the waiting Show-more page of `a`, not the in-flight base page.
  await expect(aMore).rejects.toBe(SUPERSEDED)
  const abc = q.run('abc', () => api.search('abc'))
  await expect(ab).rejects.toBe(SUPERSEDED)
  expect(api.sent).toEqual(['a'])
  expect(q.busy).toBe(true)
  api.answer('a')
  await expect(a).resolves.toMatchObject({ groups: [{ page: { marker: 'a' } }] })
  await flush()
  // The in-flight answer freed the slot; only the latest query went out.
  expect(api.sent).toEqual(['a', 'abc'])
  api.answer('abc')
  await expect(abc).resolves.toMatchObject({ groups: [{ page: { marker: 'abc' } }] })
  expect(q.busy).toBe(false)
  // A failed send frees the slot too.
  const bad = q.run('x', () => Promise.reject(new Error('boom')))
  const next = q.run('x', () => api.search('x'))
  await expect(bad).rejects.toThrow('boom')
  await flush()
  expect(api.sent).toEqual(['a', 'abc', 'x'])
  q.clear()
  api.answer('x')
  await expect(next).resolves.toBeTruthy()
})

test('clear drops what waits, not what is in flight', async () => {
  const api = fakeApi()
  const q = new SearchQueue()
  const a = q.run('a', () => api.search('a'))
  const b = q.run('b', () => api.search('b'))
  q.clear()
  await expect(b).rejects.toBe(SUPERSEDED)
  api.answer('a')
  await expect(a).resolves.toBeTruthy()
  expect(api.sent).toEqual(['a'])
  expect(q.busy).toBe(false)
})

/**
 * The dialog's request pipeline without the DOM: `typed` → `SearchQueue.type` → `debounced` → a React Query
 * observer keyed by the settled query whose `queryFn` goes through `SearchQueue.run` (what `useQuery` does in
 * `SearchDialog`/`AttachPicker`). Rendered results are whatever the observer of the current key holds.
 */
function surface(api: FakeApi) {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } })
  const queue = new SearchQueue()
  const cats: SearchCategory[] = ['notes']
  const rendered: string[] = []
  let debounced = ''
  const options = (q: string) => ({
    queryKey: queryKeys.search(q, cats, 10, 1),
    queryFn: () => queue.run(q, () => api.search(q)),
    enabled: q !== '',
  })
  const observer = new QueryObserver<SearchResponse>(qc, options(''))
  observer.subscribe((r) => {
    const marker = (r.data?.groups[0]?.page as { marker?: string } | undefined)?.marker
    if (marker && marker === debounced) rendered.push(marker)
  })
  const settle = (q: string) => {
    debounced = q
    observer.setOptions(options(q))
  }
  return {
    queue,
    rendered,
    type(q: string) {
      if (q === '') {
        queue.clear()
        settle('')
      } else queue.type(q, settle)
    },
    /** Every search key the cache ever held a page for. */
    cached: () => qc.getQueryCache().findAll({ queryKey: queryKeys.searchAll }).filter((q) => q.state.data).map((q) => q.queryKey[1]),
    data: () => observer.getCurrentResult().data,
  }
}

test('typing aquarium at 30 ms per letter sends exactly one request and renders only its result', async () => {
  const api = fakeApi()
  const s = surface(api)
  for (const n of [1, 2, 3, 4, 5, 6, 7, 8]) {
    s.type('aquarium'.slice(0, n))
    await vi.advanceTimersByTimeAsync(30)
  }
  expect(api.sent).toEqual([])
  await vi.advanceTimersByTimeAsync(DEBOUNCE_MS)
  expect(api.sent).toEqual(['aquarium'])
  api.answer('aquarium')
  await flush()
  expect(api.sent).toEqual(['aquarium'])
  expect(s.rendered).toEqual(['aquarium'])
  expect(s.cached()).toEqual(['aquarium'])
})

test('slow typing: one request per settle, an older in-flight answer is never rendered, latest query wins', async () => {
  const api = fakeApi()
  const s = surface(api)
  // Each pause is longer than the debounce, so every prefix settles; the server is slow, so they overlap.
  for (const n of [1, 2, 3, 4]) {
    s.type('aqua'.slice(0, n))
    await vi.advanceTimersByTimeAsync(DEBOUNCE_MS + 50)
  }
  // Only `a` went out; `aq` and `aqu` were dropped in favour of `aqua`, which waits for `a`.
  expect(api.sent).toEqual(['a'])
  api.answer('a')
  await flush()
  expect(api.sent).toEqual(['a', 'aqua'])
  // The stale answer for `a` sits under its own key; the list (keyed by `aqua`) shows nothing from it.
  expect(s.rendered).toEqual([])
  expect(s.data()).toBeUndefined()
  api.answer('aqua')
  await flush()
  expect(s.rendered).toEqual(['aqua'])
  // Clearing the box while a request waits drops it and empties the list.
  s.type('aquar')
  await vi.advanceTimersByTimeAsync(DEBOUNCE_MS)
  s.type('aquari')
  await vi.advanceTimersByTimeAsync(DEBOUNCE_MS)
  expect(api.sent).toEqual(['a', 'aqua', 'aquar'])
  s.type('')
  await flush()
  expect(s.data()).toBeUndefined()
  api.answer('aquar')
  await vi.advanceTimersByTimeAsync(2_000)
  expect(api.sent).toEqual(['a', 'aqua', 'aquar'])
  expect(s.rendered).toEqual(['aqua'])
})
