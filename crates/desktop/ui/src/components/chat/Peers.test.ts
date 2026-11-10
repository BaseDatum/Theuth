import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import { useToolRuns, disclosureKey } from '@/lib/tool-runs'
import { DisclosureScope } from './disclosure'
import { MessagePeer } from './MessagePeer'
import { Peers } from './Peers'

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
