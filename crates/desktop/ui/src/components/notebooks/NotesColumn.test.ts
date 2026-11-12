import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as NbRpc from '@/lib/notebooks-rpc'
import { notebookKeys } from '@/lib/notebooks-rpc'
import type { NotebookItem } from "@/rpc/bindings/method.agent.output"
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { ContextEstimate } from "@/rpc/bindings/method.agent.output"
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import type * as Hooks from './useNotebook'
import { NotesColumn } from './NotesColumn'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, search }: { children?: unknown; to: string; search?: Record<string, unknown> }) => createElement('a', { href: to, 'data-search': JSON.stringify(search ?? {}) }, children as never),
  useNavigate: () => vi.fn(),
}))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/store/connection', () => ({ useConnection: (pick: (s: unknown) => unknown) => pick({ client: {} , discovery: { phase: 'legacy' } }) }))
vi.mock('@/lib/popout', () => ({ canPopout: () => false, noteOpenMode: () => 'navigate', openPopout: vi.fn() }))
vi.mock('@/lib/citations', () => ({ provenanceOf: (md: string) => (md.includes('*Answer from [') ? 'chat' : null) }))
vi.mock('@/components/chat/Citations', () => ({ citationCount: () => 2, CitationLink: () => null }))
vi.mock('@/components/notebooks/NotebookContext', () => ({ useNotebookSession: () => 'sess1', useEnsureNotebookSession: () => async () => 'sess1' }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ doc_markdown: async () => '' }),
}))
vi.mock('@/lib/notebooks-rpc', async (original) => ({
  ...(await original<typeof NbRpc>()),
  useNotebooksApi: () => ({}),
  useContextApi: () => ({}),
}))
vi.mock('./useNotebook', async (original) => ({
  ...(await original<typeof Hooks>()),
  useContextEstimate: () => ({ data: estimate }),
}))

const NB = 'nb1'
const estimate: ContextEstimate = {
  items: [
    { id: 'n1', kind: 'note', title: 'Drill timeline', level: 'full', pinned: true, tokens: 1200, status: 'available' },
    { id: 'n2', kind: 'note', title: 'RTO vs measured', level: 'off', pinned: false, tokens: 0, status: 'omitted' },
  ],
  total_tokens: 1200,
  injected: false,
  pending_changes: 0,
  omitted: 1,
  empty: false,
}
const item = (id: string, added: number): NotebookItem => ({ notebook: NB, kind: 'note', item: id, added_ms: added, added_by: 'owner', removed_ms: null })
const note = (id: string, title: string): DocSummary => ({ id, kind: 'note', title, binding: 'note', created_ms: 1, updated_ms: Date.now() - 7_200_000, bytes: 0 })

const render = () => {
  const qc = new QueryClient()
  qc.setQueryData(notebookKeys.items(NB), [item('n1', 1), item('n2', 2)])
  qc.setQueryData(queryKeys.notes, [note('n1', 'Drill timeline'), note('n2', 'RTO vs measured'), note('n3', 'Not in this notebook')])
  qc.setQueryData(['doc', 'markdown', 'n1'], '# Drill timeline\n\nbody')
  qc.setQueryData(['doc', 'markdown', 'n2'], 'answer [1](theuth://sources/s1#b1)\n\n*Answer from [chat](theuth://sessions/x#3) · m · d*')
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(NotesColumn, { notebook: NB }) }))
}

test("the column lists the notebook's notes newest-added first, with the AI line on a saved answer", () => {
  const html = render()
  expect(html).toContain('Notes ')
  expect(html).toContain('· 2')
  expect(html.indexOf('RTO vs measured')).toBeLessThan(html.indexOf('Drill timeline'))
  expect(html).not.toContain('Not in this notebook')
  expect(html).toContain('2 citations kept')
  expect(html).toContain('edited 2h')
})

test('144 §4: every row link carries ?nb so the note page keeps the way back', () => {
  const html = render()
  const links = [...html.matchAll(/<a href="\/notes\/\$id" data-search="([^"]*)"/g)].map((m) => JSON.parse(m[1]!.replace(/&quot;/g, '"')) as Record<string, string>)
  expect(links).toHaveLength(2)
  expect(links.every((s) => s.nb === NB)).toBe(true)
})

test('wave 2 §5.1: a dial per row from the estimate (pinned full shows the dot, an off row says omitted) and the bulk row with ~tokens', () => {
  const html = render()
  expect(html).toContain('all notes:')
  expect(html).toContain('~1.2k tokens')
  expect(html).toContain('id="item-n1"')
  expect(html).toMatch(/aria-checked="true" data-pinned="true"[^>]*>full/)
  expect(html).toMatch(/aria-checked="true"[^>]*>off/)
  expect(html).toContain('>omitted<')
  expect(html).not.toContain('title="wave 2"')
})
