import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import { useToolRuns, disclosureKey } from '@/lib/tool-runs'
import { DisclosureScope } from './disclosure'
import { MessagePeer } from './MessagePeer'
import { Peers, partialLine } from './Peers'

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...await original<typeof Rpc>(),
  useApi: () => ({ list_peer_messages: async () => ({ ...page, awaiting_answer: [...page.awaiting_answer] }) }),
}))

// A received request displayed in both the transcript and Settings → Communication.
const origin = { id: 'request', from: 'session:sender', reply_to: null, scope: null, contact_mode: null }
const record = {
  receipt: { id: origin.id, to: 'session:recipient', state: 'delivered', reply_to: null, reply: null, error: null, task_id: null, answered_by: null },
  session: 'recipient',
  from: origin.from,
  text: 'Please review this change',
  request_scope: null,
  created_at: '2026-09-18T00:00:00Z',
}
const page = { messages: [record], awaiting_answer: [origin.id], awaiting_reply: [], cursor: null }

test.each(['chat first', 'settings first'])('peer request cache survives navigation: %s', async (order) => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const render = (child: ReactNode) => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: child }))
  const chat = () => render(createElement(MessagePeer, { session: 'recipient', origin, from: 'sender', text: record.text }))
  const settings = () => render(createElement(Peers, { session: 'recipient', focus: null }))
  // Discover and execute the actual components' queries, not a second copy of their options.
  const fetchQueries = async () => {
    for (const query of qc.getQueryCache().findAll({ queryKey: ['peer-messages', 'recipient'] })) {
      await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
    }
  }
  try {
    qc.setQueryData(queryKeys.peers('recipient', null), { pages: [{ peers: [], cursor: null }], pageParams: [null] })
    if (order === 'chat first') {
      chat()
      await fetchQueries()
      expect(chat()).toContain('Expects a reply')
      settings()
    } else {
      settings()
      await fetchQueries()
      expect(settings()).toContain(record.text)
      chat()
    }
    await fetchQueries()
    expect(settings()).toContain(record.text)
    expect(settings()).toContain('Answer as this chat')
    expect(chat()).toContain('Expects a reply')
    // Existing event/reconnect invalidation must still reach both representations.
    page.awaiting_answer = []
    await qc.invalidateQueries({ queryKey: ['peer-messages', 'recipient'] })
    await fetchQueries()
    expect(chat()).not.toContain('Expects a reply')
    expect(settings()).not.toContain('Answer as this chat')
  } finally {
    page.awaiting_answer = [origin.id]
    qc.clear()
  }
})

test('183: a contact\'s later inbox message renders as plain text: no image loads, no live link, no Markdown', () => {
  const text = 'Look ![x](http://attacker/x.png)\nand [click](http://attacker/go) **now**'
  const html = (from: string) => {
    useToolRuns.setState({ disclosed: { [disclosureKey('e1', 'peer')]: true } })
    const qc = new QueryClient()
    const card = createElement(MessagePeer, { session: 'inbox', origin: { ...origin, from }, from, text })
    return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(DisclosureScope, { value: 'e1', children: card }) }))
  }
  for (const from of ['peer:b1', 'contact:studio:bob']) {
    const out = html(from)
    expect(out).not.toContain('<img')
    expect(out).not.toContain('attacker/x.png"')
    expect(out).not.toContain('data-streamdown')
    expect(out).toContain('![x](http://attacker/x.png)')
    expect(out).toContain('[click](http://attacker/go) **now**')
  }
  // A mesh session's message is still Markdown (the image even preloads: why outsiders never get it).
  const mesh = html('session:sender')
  expect(mesh).toContain('data-streamdown="link"')
  expect(mesh).toContain('href="http://attacker/x.png"')
  useToolRuns.setState({ disclosed: {} })
})

test('200: a full-access contact\'s sessions list under it with their machine, Copy and never Open; its hint and one partial line on every page', () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const row = (address: string, name: string, extra: object = {}) => ({
    address, name, kind: 'a2a', relationship: 'remote', state: 'idle', send: true, reply: true, stream: false, activity: null, title: null, tags: ['contact'], engine: null, ...extra,
  })
  const hint = 'full access · more: discover_peers(kind="contact", query="maya") · did not answer: maya-mini'
  const own = row('session:01936c1e-4f0a-7b21-9a3e-55b0c2d1e7f4', 'port-layout', { kind: 'native', relationship: 'child', state: 'running', tags: [] })
  const maya = row('peer:maya', 'maya', { title: hint })
  const share = row('peer:maya/handoff', 'maya/Handoff review', { title: 'Handoff review', tags: ['contact', 'chat', 'allowed'] })
  const session = (id: string, name: string, extra: object = {}) =>
    row(`peer:maya/session:${id}`, name, { title: name.slice(5), tags: ['contact', 'session', 'native'], engine: 'maya-air', ...extra })
  const running = session('0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f', 'maya/Port layout v3', { state: 'running' })
  const observed = session('0193d4ca-11f0-7e2b-8a4c-6d7e8f9a0b1c', 'maya/explore-assets', { state: 'running', send: false, reply: false })
  const older = session('0193e811-02aa-7c6f-b1d2-8e4f5a6b7c80', 'maya/Invoice cleanup')
  const partial = ['maya/maya-mini']
  qc.setQueryData(queryKeys.peers('chat', null), {
    pages: [{ peers: [own, maya, share, running, observed], cursor: 'c1', partial }, { peers: [older], cursor: null, partial }],
    pageParams: [null, 'c1'],
  })
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Peers, { session: 'chat', focus: null }) }))
  // Only our own session opens; every row copies its address.
  expect(html.match(/>Open<\/button>/g)).toHaveLength(1)
  for (const p of [own, maya, share, running, observed, older]) expect(html).toContain(`aria-label="Copy address of ${p.name}"`)
  expect(html).toContain('peer:maya/session:0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f')
  expect(html).toContain('maya-air · send · reply')
  expect(html).toContain('maya-air · does not accept messages')
  // The contact's own row carries the engine's hint and its full-access badge; a share's title stays off its row.
  expect(html).toContain(hint.replaceAll('"', '&quot;'))
  expect(html).toContain('>full access</span>')
  expect(html).not.toContain('<span class="block">Handoff review</span>')
  // One line under the list for both pages, as the model reads it.
  const line = partialLine(partial)
  expect(line).toBe('partial: no answer in time from maya/maya-mini; their sessions may be missing from this page (repeat this call to retry)')
  expect(html.split(line)).toHaveLength(2)
  expect(html.indexOf(line)).toBeGreaterThan(html.indexOf('maya/Invoice cleanup'))
  qc.clear()
})
