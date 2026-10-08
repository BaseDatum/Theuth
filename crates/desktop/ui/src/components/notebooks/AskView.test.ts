import { type ReactNode, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { type AskState, EMPTY_ASK, applyAskEvent } from '@/lib/ask-rpc'
import type { Notebook } from '@/rpc/bindings/Notebook'
import { AskStructure, hitsMarkdown, planSteps } from './AskView'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children?: unknown }) => createElement('a', null, children as never),
  getRouteApi: () => ({ useParams: () => ({ session: 's1' }) }),
  useNavigate: () => vi.fn(),
}))
vi.mock('@/store/connection', () => ({ useConnection: () => ({ call: vi.fn() }) }))
vi.mock('@/store/sessions', () => ({ on: () => () => {}, useSessions: Object.assign(() => false, { getState: () => ({}) }) }))
vi.mock('@/components/chat/Transcript', () => ({ Transcript: () => null }))
vi.mock('@/components/chat/Composer', () => ({ Composer: () => null }))
// The chat renderer's pieces: the markdown is echoed so the chip hrefs can be asserted.
vi.mock('streamdown', () => ({ Streamdown: ({ children }: { children: string }) => createElement('div', { 'data-md': true }, children) }))
vi.mock('@/components/chat/Citations', () => ({ CitationsProvider: ({ children }: { children: ReactNode }) => children, References: () => null }))
vi.mock('@/components/chat/MessageAssistant', () => ({ markdownComponents: {}, plugins: {}, rehypePlugins: [], shikiTheme: ['a', 'b'] }))
vi.mock('@/components/notebooks/useNotebook', () => ({ useSources: () => ({ data: [] }) }))
vi.mock('@/components/notebooks/useNotebooks', () => ({ useNotebooks: () => ({ data: [] }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))

const nb = (id: string, title: string): Notebook => ({ doc: { id, title, kind: 'notebook', binding: 'notebook', created_ms: 1n, updated_ms: 1n, bytes: 0n }, project: null, archived_ms: null, sources: 0, notes: 0, chats: 0 })
const NOTEBOOKS = [nb('a', 'DR failover'), nb('b', 'Onboarding')]

const render = (state: AskState, extra: Partial<Parameters<typeof AskStructure>[0]> = {}) =>
  renderToStaticMarkup(createElement(AskStructure, { state, notebooks: NOTEBOOKS, sourceIds: new Set(['src1']), answered: false, live: state.done === null, retry: vi.fn(), retrying: false, ...extra }))

test('planSteps names each branch\'s notebooks and query; hitsMarkdown writes the §6.1 chips (sources vs notes by id)', () => {
  expect(planSteps([{ n: 1, notebooks: ['a', 'b'], query: 'RTO', instructions: '' }, { n: 2, notebooks: ['zzz'], query: 'who', instructions: '' }], NOTEBOOKS)).toEqual([
    { title: 'DR failover · Onboarding', detail: 'RTO' },
    { title: 'Untitled', detail: 'who' },
  ])
  expect(hitsMarkdown([{ id: 'src1', block: 'b1', title: 'Runbook [v2]', score: 1 }, { id: 'note1', block: null, title: '', score: 0 }], new Set(['src1']))).toBe('[Runbook  v2 ](theuth://sources/src1#b1) · [note1](theuth://notes/note1)')
})

test('nothing before the plan; then steps, one collapsible row per branch with its hits and partial, the answer, then a destructive alert with Retry on error', () => {
  expect(render(EMPTY_ASK)).toBe('')
  let s = applyAskEvent(EMPTY_ASK, { type: 'plan', branches: [{ n: 1, notebooks: ['a'], query: 'RTO target', instructions: '' }, { n: 2, notebooks: ['b'], query: 'owners', instructions: '' }] })
  let html = render(s)
  expect(html).toContain('aria-label="Plan"')
  expect(html).toContain('DR failover')
  expect(html).toContain('Branch 1 · RTO target')
  expect(html).toContain('Branch 2 · owners')
  expect(html).toContain('searching…')
  expect(html).not.toContain('role="alert"')
  s = applyAskEvent(s, { type: 'branch', n: 1, hits: [{ id: 'src1', block: 'b9', title: 'Runbook', score: 0.9 }] })
  s = applyAskEvent(s, { type: 'partial', n: 1, answer: 'RTO is 4 h [1](theuth://sources/src1#b9)' })
  html = render(s)
  expect(html).toContain('1 hit')
  expect(html).toContain('[Runbook](theuth://sources/src1#b9)')
  expect(html).toContain('RTO is 4 h')
  s = applyAskEvent(s, { type: 'answer', markdown: '**Synthesis** with [1](theuth://sources/src1#b9)' })
  expect(render(s)).toContain('**Synthesis**')
  // Once the session's own assistant turn is in the transcript the structure stops repeating the answer.
  expect(render(s, { answered: true })).not.toContain('**Synthesis**')
  s = applyAskEvent(s, { type: 'done', ok: false, error: 'branch 2 failed twice' })
  html = render(s)
  expect(html).toContain('role="alert"')
  expect(html).toContain('branch 2 failed twice')
  expect(html).toContain('Retry')
  expect(render(s, { retry: null })).not.toContain('Retry')
})
