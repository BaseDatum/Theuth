import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import { StudyList } from './StudyList'
import { studies } from './fixtures'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({}),
}))
// A static paint has no router: `Link` is a bare anchor, no open setup chat.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children?: unknown }) => createElement('a', { href: '#' }, children as never),
  useRouterState: () => null,
}))
const [user, meta] = studies()
vi.mock('./queries', () => ({
  useStudies: () => ({ data: list, isPending: false, isError: false }),
  useDrafts: () => ({ data: [] }),
  useDiscardDraft: () => ({ isPending: false, mutate: () => {} }),
}))
const list = [
  { ...user!.info, created_by: 'user' },
  { ...meta!.info, slug: 'inner-one', created_by: 'meta:tune-prompts/inner-one' },
]

function paint(showInner?: boolean): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(StudyList, { q: '', active: undefined, showInner }) }))
}

test('rail: an inner study (created_by meta:…) is hidden until the toggle shows it (159 W7 H7)', () => {
  const hidden = paint()
  expect(hidden).toContain(user!.info.slug)
  expect(hidden).not.toContain('inner-one')
  expect(hidden).toContain('Show 1 inner study')
  const shown = paint(true)
  expect(shown).toContain('inner-one')
  expect(shown).toContain('Hide inner studies')
})
