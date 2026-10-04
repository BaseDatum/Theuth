import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { SectionTabs } from './SectionTabs'

type State = { location: { pathname: string } }
const router = vi.hoisted(() => ({ pathname: '/agents' }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => () => Promise.resolve(),
  useRouterState: ({ select }: { select: (s: State) => string }) => select({ location: { pathname: router.pathname } }),
}))

const active = (html: string) => [...html.matchAll(/<button[^>]*data-state="active"[^>]*aria-label="([^"]+)"/g)].map((m) => m[1])

test('agents group: Tasks active under /tasks/abc, labels fold below 20rem', () => {
  router.pathname = '/tasks/abc'
  const html = renderToStaticMarkup(createElement(SectionTabs, { group: "agents" }))
  expect(active(html)).toEqual(['Tasks'])
  expect(html.match(/<button/g)?.length).toBe(3)
  expect(html).toContain('hidden @min-[20rem]:inline')
  expect(html).toContain('@container')
})

test('library group: Meetings active under /meetings/briefs, seven icon-only tabs below 40rem', () => {
  router.pathname = '/meetings/briefs'
  const html = renderToStaticMarkup(createElement(SectionTabs, { group: "library" }))
  expect(active(html)).toEqual(['Meetings'])
  expect(html.match(/<button/g)?.length).toBe(7)
  expect(html).toContain('hidden @min-[40rem]:inline')
  expect(html).toContain('title="Artifacts"')
})

test('no tab active on a foreign path', () => {
  router.pathname = '/settings/general'
  expect(active(renderToStaticMarkup(createElement(SectionTabs, { group: "library" })))).toEqual([])
})
