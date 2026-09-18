import { expect, test } from 'vitest'
import type { PageState } from '@/rpc/bindings/PageState'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchGroup } from '@/rpc/bindings/SearchGroup'
import { DEFAULT_LIMITS, indexing, patch } from './search'

const group = (category: SearchCategory, state: PageState): SearchGroup =>
  ({ category, page: { state, hits: [], has_more: false, coverage: 'complete', notices: [] } }) as SearchGroup

test('indexing: only selected, still-building groups at the base limit are re-asked', () => {
  const groups = [
    group('sessions', 'ready'),
    group('artifacts', 'indexing'),
    group('knowledge', 'error'),
    group('knowledge_history', 'indexing'),
  ]
  const all: SearchCategory[] = ['sessions', 'artifacts', 'knowledge', 'knowledge_history']
  // Ready and failed groups are never re-matched; unavailable stops too.
  expect(indexing(all, groups, DEFAULT_LIMITS)).toEqual(['artifacts', 'knowledge_history'])
  expect(indexing(all, [group('artifacts', 'unavailable')], DEFAULT_LIMITS)).toEqual([])
  // A deselected category is not polled even if its old group still sits in the page.
  expect(indexing(['sessions', 'knowledge_history'], groups, DEFAULT_LIMITS)).toEqual(['knowledge_history'])
  // A category with its own Show more page polls that page, not the base.
  expect(indexing(all, groups, { ...DEFAULT_LIMITS, artifacts: 30 })).toEqual(['knowledge_history'])
  // Before the first answer there is nothing to poll.
  expect(indexing(all, undefined, DEFAULT_LIMITS)).toEqual([])
})

test('patch: a poll answer replaces exactly the groups it re-asked', () => {
  const old = [group('sessions', 'ready'), group('artifacts', 'indexing'), group('knowledge_history', 'indexing')]
  const fresh = [group('artifacts', 'ready'), group('knowledge', 'ready')]
  const out = patch(old, fresh)
  expect(out.map((g) => [g.category, g.page.state])).toEqual([
    ['sessions', 'ready'],
    ['artifacts', 'ready'],
    ['knowledge_history', 'indexing'],
  ])
  // Untouched groups keep identity (structural sharing, no spurious re-render); a reply for a category the page
  // no longer holds is dropped rather than appended.
  expect(out[0]).toBe(old[0])
  expect(out.some((g) => g.category === 'knowledge')).toBe(false)
})
