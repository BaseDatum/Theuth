import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { CitationRow } from "@/rpc/bindings/method.autoresearch.output"
import type { Idea as StudyIdea } from "@/rpc/bindings/method.autoresearch.output"
import { LiteratureIdeas, host } from './LiteratureIdeas'

const idea = (id: number, source: StudyIdea['source'], extra: Partial<StudyIdea> = {}): StudyIdea => ({
  id,
  slug: 'toy-sort',
  text: `idea ${id}`,
  category: 'algorithm',
  source,
  priority: 1,
  status: 'open',
  tried_in: [],
  at: '2026-10-10T00:00:00Z',
  ...extra,
})

const citation = (idea_id: number, url: string, quote: string, title = ''): CitationRow => ({
  idea_id: idea_id,
  url,
  title,
  quote,
  verified: true,
  fetched_at: '2026-10-10T00:00:00Z',
})

test('literature ideas: nothing for a 140 study, a paper chip with link and quote per cited idea, the trial named', () => {
  const plain = [idea(1, 'user'), idea(2, 'model'), idea(3, 'advisor')]
  expect(renderToStaticMarkup(createElement(LiteratureIdeas, { ideas: plain }))).toBe('')

  const ideas = [...plain, idea(4, 'literature', { text: '[paper] Merge natural runs like Timsort', tried_in: [7, 9] }), idea(5, 'literature', { status: 'closed' })]
  const citations = [citation(4, 'https://www.example.org/timsort?x=1', 'runs that are already ordered', 'Timsort — the sort behind sorted()')]
  const html = renderToStaticMarkup(createElement(LiteratureIdeas, { ideas, citations }))
  expect(html).toContain('data-papers="2"')
  expect(html).toContain('2 cited ideas')
  expect(html).not.toContain('[paper] Merge')
  expect(html).toContain('Merge natural runs like Timsort')
  expect(html).toContain('href="https://www.example.org/timsort?x=1"')
  expect(html).toContain('paper · Timsort — the sort behind sorted()')
  expect(html).toContain('<q class="min-w-0 italic" title="Timsort — the sort behind sorted() — example.org">runs that are already ordered</q>')
  expect(html).toContain('tried in #7, #9')
  // Non-literature ideas stay out; a literature idea without a known citation keeps a bare chip.
  expect(html).not.toContain('idea 1')
  expect(html).toContain('line-through')
  expect((html.match(/>paper</g) ?? []).length).toBe(1)
  expect(host('https://www.example.org/a/b#c')).toBe('example.org')
  expect(host('example.org/x')).toBe('example.org')
})
