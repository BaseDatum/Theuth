// 144 wave 2 §5.2: the context line under the chat header, from a mocked `notebook_context_estimate`.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ContextEstimate } from "@/rpc/bindings/method.agent.output"
import { ContextLine } from './ChatColumn'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/components/chat/Composer', () => ({ Composer: () => null }))
vi.mock('@/components/chat/Transcript', () => ({ Transcript: () => null }))
vi.mock('@/components/sidebar/AgentTree', () => ({ ago: () => '' }))
vi.mock('@/store/sessions', () => ({ on: () => () => {}, useSessions: { getState: () => ({}) } }))
vi.mock('@/store/connection', () => ({ useConnection: (sel: (s: { client: unknown }) => unknown) => sel({ client: {} }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}), queryKeys: { notes: ['notes'], roster: ['roster'] } }))

const estimate: ContextEstimate = {
  items: [
    { id: 's1', kind: 'source', title: 'Runbook', level: 'summary', pinned: false, tokens: 3000, status: 'available' },
    { id: 's2', kind: 'source', title: 'SLA', level: 'summary', pinned: false, tokens: 3000, status: 'available' },
    { id: 's3', kind: 'source', title: 'Transcript', level: 'full', pinned: true, tokens: 5000, status: 'available' },
    { id: 's4', kind: 'source', title: 'Ticket', level: 'mention', pinned: false, tokens: 40, status: 'available' },
    { id: 's5', kind: 'source', title: 'Old deck', level: 'off', pinned: true, tokens: 0, status: 'omitted' },
    { id: 'n1', kind: 'note', title: 'Timeline', level: 'full', pinned: false, tokens: 800, status: 'available' },
    { id: 'n2', kind: 'note', title: 'Questions', level: 'full', pinned: false, tokens: 560, status: 'available' },
  ],
  total_tokens: 12_400,
  omitted: 1,
  empty: false,
  injected: false,
  pending_changes: 0,
}

const render = (e: ContextEstimate | undefined, error: Error | null = null) => renderToStaticMarkup(createElement(ContextLine, { estimate: e, error }))

test('the §5.2 sentence, the omitted chip aimed at the first omitted card, no budget bar', () => {
  const html = render(estimate)
  expect(html.replace(/<[^>]+>/g, '')).toContain('in context: 4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k tokens1 omitted')
  expect(html).toContain('title="Scroll to the first omitted item"')
  expect(html).not.toContain('progressbar')
  expect(html).not.toContain('40k')
})

test('pending changes once the block is in the chat; nothing in context is said before send', () => {
  const sent = render({ ...estimate, injected: true, pending_changes: 2 }).replace(/<[^>]+>/g, '')
  expect(sent).toContain('2 changes go with your next message')
  expect(render({ ...estimate, injected: true }).replace(/<[^>]+>/g, '')).not.toContain('next message')
  const empty = render({ items: [], total_tokens: 0, omitted: 0, empty: true, injected: false, pending_changes: 0 })
  expect(empty).toContain('nothing in context — the agent sees only the description')
  expect(empty).not.toContain('omitted')
  expect(render(undefined)).toContain('in context: …')
  expect(render(undefined, new Error('no hub'))).toContain('no hub')
})
