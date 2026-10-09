import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Popout from '@/lib/popout'
import type { Overview } from '@/rpc/bindings/Overview'
import { askKeys } from '@/lib/ask-rpc'
import { NotebookHeaderActions, popOutNotebook } from './NotebookHeaderActions'

const env = vi.hoisted(() => ({ mobile: false, popped: false, canPopout: true, openPopout: vi.fn(() => Promise.resolve()) }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => env.mobile }))
vi.mock('@/lib/popout', async (original) => ({
  ...(await original<typeof Popout>()),
  isPopout: () => env.popped,
  canPopout: () => env.canPopout,
  openPopout: env.openPopout,
}))
vi.mock('@/store/connection', () => ({ useConnection: () => ({ call: vi.fn() }) }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/components/notebooks/AskDialog', () => ({ AskDialog: ({ open }: { open: boolean }) => (open ? createElement('div', { 'data-ask': true }) : null) }))

const NB = 'nb1'
const overview = (kind: Overview['kind'], status: Overview['status']): Overview => ({ kind, artifact: null, name: `${kind}.md`, version: 1, created_ms: 1n, session: 's', status, note: null, error: null })
const render = (ask?: string | true) => {
  const qc = new QueryClient()
  qc.setQueryData(askKeys.overviews(NB), [overview('briefing', 'running')])
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(NotebookHeaderActions, { notebook: NB, ask, onAskChange: vi.fn() }) }))
}

test('desktop: Ask across notebooks, Overviews ▾ and Pop out sit in the header; the dialog follows ?ask', () => {
  const html = render()
  expect(html).toContain('Ask across notebooks')
  expect(html).toContain('Overviews')
  expect(html).toContain('aria-label="Pop out"')
  expect(html).not.toContain('aria-label="Notebook actions"')
  expect(html).not.toContain('data-ask')
  expect(render(true)).toContain('data-ask')
})

test('Sky\'s ruling: Pop out opens /notebooks/<id> in its own window through the shared helper; none inside a pop-out or where no window can open', async () => {
  await popOutNotebook(NB)
  expect(env.openPopout).toHaveBeenCalledWith({ kind: 'notebook', notebook: NB })
  env.popped = true
  expect(render()).not.toContain('aria-label="Pop out"')
  env.popped = false
  env.canPopout = false
  expect(render()).not.toContain('aria-label="Pop out"')
  env.canPopout = true
})

test('under 768 px the group collapses into one ⋯ menu', () => {
  env.mobile = true
  const html = render()
  expect(html).toContain('aria-label="Notebook actions"')
  expect(html).not.toContain('aria-label="Pop out"')
  expect(html).not.toContain('>Ask across notebooks<')
  env.mobile = false
})
