// 144 §4 / §4.3: the source page's way back lands on the notebook's Sources tab; the Insights tab reads the
// `## Insights` sections from the document's blocks (pure `insightSections`).
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DocBlock } from '@/components/board/blocks'
import type { NotebookBack } from '@/components/editor/DocumentPage'
import type * as NbRpc from '@/lib/notebooks-rpc'
import { SourcePage, insightSections } from './SourcePage'

vi.mock('@tanstack/react-router', () => ({
  getRouteApi: () => ({ useParams: () => ({ id: 's1' }), useSearch: () => ({ nb: 'nb1', tab: 'insights' }) }),
  Link: ({ children }: { children?: ReactNode }) => createElement('a', null, children),
  useNavigate: () => vi.fn(),
}))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/store/connection', () => ({ useConnection: (pick: (s: unknown) => unknown) => pick({ client: {} , discovery: { phase: 'legacy' } }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/editor/useDocument', () => ({ useDocument: () => ({ provider: { doc: { getXmlFragment: () => fragment } }, status: { kind: 'live' }, error: null }) }))
vi.mock('@/components/board/blocks', () => ({
  useBlocks: () => blocks,
  plainText: (content: unknown) => (Array.isArray(content) ? content.map((c: { text: string }) => c.text).join('') : ''),
}))
vi.mock('@/components/notebooks/useNotebook', () => ({
  useSource: () => ({ data: undefined, isError: false }),
  useItemNotebooks: () => [],
  useNotebook: () => ({ data: { doc: { id: 'nb1', title: 'DR failover' } } }),
}))
vi.mock('@/lib/notebooks-rpc', async (original) => ({
  ...(await original<typeof NbRpc>()),
  useNotebooksApi: () => ({}),
  useContextApi: () => ({ insight_skills: async () => ['key_insights', 'toc', 'open_questions', 'risk-register'] }),
}))
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  DropdownMenuTrigger: ({ children }: { children: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuItem: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menuitem' }, children),
  DropdownMenuLabel: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  DropdownMenuSeparator: () => null,
}))
// The frame is DocumentPage's own test; here it echoes what the page passes it.
vi.mock('@/components/editor/DocumentPage', () => ({
  DocumentPage: ({ back, title, fallback }: { back?: NotebookBack; title: ReactNode; fallback?: ReactNode }) =>
    createElement('div', { 'data-back': JSON.stringify(back ?? null) }, title, fallback),
  useNotebookBack: (kind: string, _id: string, nb?: string) => ({
    back: nb ? { to: '/notebooks/$id', params: { id: nb }, search: { tab: kind === 'note' ? 'notes' : 'sources' }, label: 'Back to DR failover', always: true } : undefined,
    chip: null,
  }),
}))

const fragment = { length: 0, observeDeep() {}, unobserveDeep() {} }
const heading = (id: string, level: number, text: string): DocBlock => ({ id, type: 'heading', props: { level }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock
const para = (id: string, text: string, type = 'paragraph'): DocBlock => ({ id, type, props: {}, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock

let blocks: DocBlock[] = [
  heading('t', 1, 'Runbook'),
  para('p0', 'Body text that is not an insight.'),
  heading('h2', 2, 'Insights'),
  heading('i1', 3, 'Dense summary · claude-haiku · 2026-10-08'),
  para('i1a', 'Failover takes 42 minutes.'),
  para('i1b', 'DNS is the long pole.', 'bulletListItem'),
  heading('i2', 3, 'Open questions · gpt-5-mini · 2026-10-09'),
  para('i2a', 'Who owns the DNS change?'),
  heading('h2b', 2, 'Appendix'),
  para('ap', 'Not an insight either.'),
]

test('insightSections: the ### sections under ## Insights with name · model · date and their blocks; nothing outside', () => {
  const s = insightSections(blocks)
  expect(s.map((x) => [x.id, x.name, x.model, x.date, x.blocks.map((b) => b.id)])).toEqual([
    ['i1', 'Dense summary', 'claude-haiku', '2026-10-08', ['i1a', 'i1b']],
    ['i2', 'Open questions', 'gpt-5-mini', '2026-10-09', ['i2a']],
  ])
  expect(insightSections([heading('t', 1, 'Runbook'), para('p', 'x')])).toEqual([])
  expect(insightSections([heading('h2', 2, 'Insights'), para('stray', 'before any ###'), heading('i', 3, 'Key insights')])).toEqual([{ id: 'i', name: 'Key insights', model: null, date: null, blocks: [] }])
})

const render = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(SourcePage) }))

test('the page passes the way back to /notebooks/$nb?tab=sources and renders the Insights tab: sections, Promote to note, Run ▾ with the builtins', () => {
  const html = render()
  expect(html).toContain('data-back="{&quot;to&quot;:&quot;/notebooks/$id&quot;,&quot;params&quot;:{&quot;id&quot;:&quot;nb1&quot;},&quot;search&quot;:{&quot;tab&quot;:&quot;sources&quot;}')
  expect(html).toContain('Insights · 2')
  expect(html).toContain('Dense summary')
  expect(html).toContain('claude-haiku · 2026-10-08')
  expect(html).toContain('Failover takes 42 minutes.')
  expect(html).toContain('Who owns the DNS change?')
  expect(html).not.toContain('Not an insight')
  expect(html.match(/Promote to note/g)).toHaveLength(2)
  expect(html).toMatch(/role="menu".*Key insights.*Table of contents.*Open questions/s)
  expect(html).not.toContain('Insights land in wave 2')
})

test('no insights: the empty state names Run ▾', () => {
  blocks = [heading('t', 1, 'Runbook'), para('p', 'x')]
  expect(render()).toContain('No insights yet — Run ▾ generates one.')
})
