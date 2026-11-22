// 207 §11.2: the flat list's cards rendered from the fixture — the answer on the card, per kind.
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { ATTENTION_FIXTURE, ATTENTION_FIXTURE_NOW as NOW } from '@/components/settings/attention-fixtures'
import { ATTENTION_SIM, SANDBOXES_SIM, SECRET_REQUESTS_SIM } from '@/components/settings/sandbox-queries'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { BulkBar } from './BulkBar'
import { Card, StackCard } from './Card'
import { EMPTY_FILTER, kindCounts, projectsOf } from './model'
import { useReply } from './queries'
import { Toolbar } from './Toolbar'

const rpc = vi.hoisted(() => ({ send_input: vi.fn(async (session: string, _text: string, _files: string[], _delivery: null) => session) }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    sandboxes_list: async () => SANDBOXES_SIM,
    secret_requests_list: async () => SECRET_REQUESTS_SIM,
    send_input: rpc.send_input,
  }),
}))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
}))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

const client = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
const F = ATTENTION_FIXTURE
const at = (i: number): AttentionItem => F[i] as AttentionItem

/** Paint, run every query the paint started, paint again (twice: a query enabled by another's data starts on the second pass). */
async function render(node: ReactNode, qc = client()): Promise<string> {
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: node }))
  for (let pass = 0; pass < 2; pass++) {
    paint()
    for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  }
  return paint()
}

const card = (item: AttentionItem, open = false) => createElement(Card, { item, now: NOW, onSelect: () => undefined, open })

test('an approval card: kind, priority, the reason line, the command in mono, where it came from, Allow / Deny / Snooze / Dismiss', async () => {
  const html = await render(card(at(0)))
  expect(html).toContain('>approval<')
  expect(html).toContain('>high<')
  expect(html).toContain('blocks 2 agents · denies itself in 2:00')
  expect(html).toMatch(/<p class="[^"]*font-mono[^"]*">kubectl delete deployment relay-canary -n prod<\/p>/)
  expect(html).toContain('>release-2.5 · Ship 2.5 release › builder<')
  expect(html).toContain('>3m<')
  for (const label of ['>Allow<', '>Deny<', 'Snooze', '>Dismiss<']) expect(html).toContain(label)
  // The selection box is there (hidden until hover or a selection exists).
  expect(html.match(/<button[^>]*role="checkbox"[^>]*>/)?.[0]).toContain('aria-checked="false"')
})

test('a stack card: ×3, Allow all 3 / Deny all 3, the head’s command, an expander', async () => {
  const html = await render(createElement(StackCard, { items: [at(3), at(4), at(5)], now: NOW, onSelect: () => undefined }))
  expect(html).toContain('×3')
  expect(html).toContain('Allow all 3')
  expect(html).toContain('Deny all 3')
  expect(html).toContain('Show 3')
  expect(html).toContain('GET https://api.crunchbase.com/v4/entities/organizations/initech')
  // The path already names the desk: the agent is not repeated after it.
  expect(html).toContain('>theuth · ops desk<')
})

test('a failed session-stopped card: the outcome word, the error, the reply box; a timed-out one adds its next step as a chip', async () => {
  const failed = await render(card(at(25)))
  expect(failed).toContain('>failed<')
  expect(failed).not.toContain('>stopped<')
  expect(failed).toContain('migration 0010 failed: UNIQUE constraint failed: wakes.key')
  expect(failed).toContain('urgent priority · ranked project · stopped 5 min ago')
  expect(failed).toMatch(/<input[^>]*aria-label="Reply to the session"/)
  expect(failed).toContain('Send')
  const timedOut = await render(card(at(26)))
  expect(timedOut).toContain('>timed out<')
  expect(timedOut).toContain('Drafted Highlights and Changes; Fixes not started.')
  expect(timedOut).toContain('peer work deadline passed after 40 min')
  expect(timedOut).toMatch(/<button[^>]*>Finish the Fixes section<\/button>/)
})

test('the stopped card’s reply starts the next turn: send_input(session, trimmed text, [], null)', async () => {
  rpc.send_input.mockClear()
  const probe: { send?: (session: string, text: string) => Promise<string> } = {}
  function Probe() {
    probe.send = useReply().send
    return null
  }
  await render(createElement(Probe))
  const session = at(25).session as string
  await expect(probe.send!(session, '  retry with a fresh key  ')).resolves.toBe(session)
  expect(rpc.send_input).toHaveBeenCalledWith(session, 'retry with a fresh key', [], null)
})

test('a single-question ask answers on the card, one button per option; a two-question ask opens the pane', async () => {
  const one = await render(card(at(1)))
  for (const label of ['>Yes<', '>No<', '>Only the relay tests<']) expect(one).toContain(label)
  expect(one).not.toContain('>Open<')
  const two = await render(card(at(7)))
  expect(two).toContain('>Open<')
  expect(two).not.toContain('Acme, Globex, Initech')
})

test('secret cards: Enter secret… opens the secret_requests row — title, one masked input per field, the VM name as origin', async () => {
  const closed = await render(card(at(9)))
  expect(closed).toContain('Enter secret…')
  expect(closed).not.toContain('value of')
  const items = ATTENTION_SIM.filter((i) => i.kind === 'secret')
  const html = await render(createElement('ul', null, ...items.map((i) => createElement(Card, { key: i.id, item: i, now: NOW, open: true }))))
  // The form raised on this host: its title and both fields, the password masked.
  expect(html).toContain('Postgres login')
  expect(html).toMatch(/<input[^>]*type="text"[^>]*aria-label="value of PGUSER"/)
  expect(html).toMatch(/<input[^>]*type="password"[^>]*aria-label="value of PGPASSWORD"/)
  // The single field raised on `lab`'s engine: the VM's name is its origin; the field's label is the note.
  expect(html).toContain('· on lab')
  expect(html).toContain('a fine-grained token with repo scope')
  expect(html).toContain('adds v2')
  expect(html).not.toContain('Answer it in the session')
})

test('decision, review and show cards open the pane; every card can be snoozed or dismissed', async () => {
  for (const i of [2, 21, 17]) {
    const html = await render(card(at(i)))
    expect(html).toContain('>Open<')
    expect(html).toContain('>Dismiss<')
  }
})

test('the toolbar: search, sort, one chip per kind with its count (lit chips pressed), the project select', async () => {
  const html = await render(
    createElement(Toolbar, { filter: { ...EMPTY_FILTER, kinds: new Set(['finished']) }, sort: 'priority', kinds: kindCounts(F), projects: projectsOf(F), onFilter: () => undefined, onSort: () => undefined }),
  )
  expect(html).toMatch(/<input[^>]*aria-label="Search attention"/)
  expect(html).toContain('aria-label="Sort"')
  expect(html).toContain('aria-label="Project"')
  expect(html.match(/aria-pressed="/g)).toHaveLength(16)
  expect(html).toMatch(/aria-pressed="true"[^>]*>stopped<span[^>]*>4<\/span>/)
  expect(html).toMatch(/aria-pressed="false"[^>]*>approval<span[^>]*>5<\/span>/)
  // `node` and `home` both read "offline" on a card; as filters they need two names.
  expect(html).toContain('>mesh<')
  expect(html).toContain('>machines<')
})

test('the bulk bar offers Allow N / Deny N only when every selected item is an approval', async () => {
  const bar = (approvals: boolean) =>
    render(createElement(BulkBar, { count: 4, approvals, pending: false, onDismiss: () => undefined, onSnooze: () => undefined, onResolve: () => undefined, onClear: () => undefined }))
  const all = await bar(true)
  for (const label of ['4 selected', '>Allow 4<', '>Deny 4<', 'Snooze 4', '>Dismiss 4<']) expect(all).toContain(label)
  const mixed = await bar(false)
  expect(mixed).toContain('>Dismiss 4<')
  expect(mixed).not.toContain('Allow 4')
  expect(mixed).not.toContain('Deny 4')
})

test('208: "Show in graph" with onShowInGraph and a session; absent without either', async () => {
  const withSession = { ...at(0), session: 'r1.builder' } as AttentionItem
  const shown = await render(createElement(Card, { item: withSession, now: NOW, onShowInGraph: () => undefined }))
  expect(shown).toContain('aria-label="Show in graph"')
  expect(await render(createElement(Card, { item: withSession, now: NOW }))).not.toContain('Show in graph')
  const noSession = { ...at(0), session: null } as AttentionItem
  expect(await render(createElement(Card, { item: noSession, now: NOW, onShowInGraph: () => undefined }))).not.toContain('Show in graph')
  const stack = await render(createElement(StackCard, { items: [{ ...at(3), session: 'r3' } as AttentionItem, at(4), at(5)], now: NOW, onShowInGraph: () => undefined }))
  expect(stack).toContain('aria-label="Show in graph"')
})
