import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { keys } from '@/components/tasks/queries'
import type * as Files from '@/lib/files'
import { classify, parseFileNotice } from '@/lib/injected'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { AgentEvent } from '@/rpc/bindings/event.agent_2eevents.receiver'
import type { ContactFile } from '@/rpc/bindings/method.agent.output'
import type { HistoryEntry } from '@/rpc/bindings/method.agent.receiver'
import { receiveComponent, receiveTopic } from '@/rpc/bindings/receive'
import { active } from '@/store/engine-scope'
import { apply, empty, fromHistory } from '@/store/reduce'
import type { Entry, SessionSlice } from '@/store/sessions'
import type { UiEvent as WireEvent } from '@/store/ui-events'
import { FileCard, fileView, recordView, sentFiles } from './ContactFiles'
import { Message } from './Message'
import { SendCard } from './SendCard'

let local = true
vi.mock('@/lib/files', async (original) => ({ ...(await original<typeof Files>()), isLocal: () => local }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))

const HOLDER = '5f3a'.padEnd(64, '0')
const OTHER = '9e1b'.padEnd(64, '0')
const F1 = '9d0c4b7e21a35f6880e1c2d3b4a59687'
const F2 = '41b8d2f0c7e65a1394b2a8d0e3f1c5a7'
const M = '0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f'
const SAVED = '/Users/sky/.theuth/files/received/maya/2026-11-17/site-survey.zip'
const S = 'sess'
const T = 'turn'

// The engine's exact forms (`files::placeholders`, `notice`, `ended_notice`), attributes escaped as `loop_::attr` does.
const head = (name: string, type: string) => `<message_file from="peer:maya" message="${M}" name="${name}" type="${type}"`
const tail = (id: string) => ` holder="${HOLDER}" file="${id}"`
const ROW = `<message from="peer:maya" id="${M}" at="14:31">
Site survey and the old drawings, as promised.
</message>
${head('site-survey.zip', 'application/zip')} bytes="1200000000"${tail(F1)} state="receiving"/>
${head('a &quot;b&quot; &lt;c&gt; &amp; d.pdf', 'application/pdf')} bytes="24100000"${tail(F2)} state="receiving"/>
Files from peer:maya are arriving; a notice follows when each one is saved or fails.`
const RECEIVED = `${head('site-survey.zip', 'application/zip')} state="received" saved="${SAVED}" bytes="1200000000" blake3="${'7c1e'.padEnd(64, 'a')}"${tail(F1)} machine="Sky's &amp; MacBook"/>
File notice from theuth, not from peer:maya: no reply to the contact is needed. The file is saved on disk, not shown to you; open it with read_file if you need it.`
const FAILED = `${head('drawings.pdf', 'application/pdf')} state="failed" reason="the sender's file &lt;changed&gt;"${tail(F2)}/>
File notice from theuth, not from peer:maya: no reply to the contact is needed.`
const ENDED = `${head('site-survey.zip', 'application/zip')} state="received" saved="${SAVED}" bytes="1200000000" blake3="${'7c1e'.padEnd(64, 'a')}"${tail(F1)} machine="Sky's MacBook"/>
File notice from theuth: the share with peer:maya has ended, so this was not given to the agent.`

const record = (state: ContactFile['state'], over: Partial<ContactFile> = {}): ContactFile =>
  ({ id: F1, holder: HOLDER, direction: 'in', state, exchange: M, contact_id: 'rec', contact: 'Maya', name: 'site-survey.zip', media_type: 'application/zip', size: 1200000000, done: 0, machine: "Sky's MacBook", deadline: '2026-11-24T14:32:00Z', ...over }) as ContactFile

const ctx = () => ({ sent: new Set<string>(), now: 0 })
const run = (events: (WireEvent | AgentEvent)[], s: SessionSlice = empty(S)) => events.reduce((acc, e) => apply(acc, receiveTopic('agent.events@1', e) as WireEvent, ctx()), s)
const history = (h: HistoryEntry) => fromHistory(receiveComponent('method.agent.output.HistoryEntry', h))
const origin = { id: M, from: 'peer:maya', reply_to: null, scope: null, contact_mode: 'full' as const }

let qc: QueryClient
beforeEach(() => {
  local = true
  qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(keys.mesh, { self_id: HOLDER, self_name: "Sky's MacBook", peers: [{ id: OTHER, name: 'Mac mini' }] })
  active().sessions.setState((st) => ({ sessions: { ...st.sessions, [S]: empty(S) } }))
})
const render = (el: ReactElement) => renderToStaticMarkup(h(QueryClientProvider, { client: qc }, el))
const slice = (s: SessionSlice) => {
  const store = active().sessions
  store.setState((st) => ({ sessions: { ...st.sessions, [S]: s } }))
  // Server rendering reads zustand's initial state: let it read the live one.
  Object.assign(store, { getInitialState: store.getState })
}
const msg = (entry: Entry) => render(h(Message, { entry, prompts: null, session: S }))

test('classify: the message row\'s placeholders, unescaped, after the peer card; the hint line is dropped', () => {
  const cls = classify(ROW)
  expect(cls?.map((c) => c.kind)).toEqual(['peer', 'file', 'file'])
  expect(cls?.[1]).toEqual({ kind: 'file', file: { from: 'peer:maya', message: M, name: 'site-survey.zip', type: 'application/zip', state: 'receiving', holder: HOLDER, id: F1, bytes: 1200000000 } })
  expect(cls?.[2]).toMatchObject({ file: { name: 'a "b" <c> & d.pdf', id: F2, bytes: 24100000 } })
})

test('classify: received and failed notices are one file card each, every attribute read (blake3 too); ended is flagged', () => {
  expect(classify(RECEIVED)).toEqual([
    { kind: 'file', file: { from: 'peer:maya', message: M, name: 'site-survey.zip', type: 'application/zip', state: 'received', holder: HOLDER, id: F1, bytes: 1200000000, saved: SAVED, blake3: '7c1e'.padEnd(64, 'a'), machine: "Sky's & MacBook" } },
  ])
  expect(classify(FAILED)).toEqual([{ kind: 'file', file: expect.objectContaining({ state: 'failed', reason: "the sender's file <changed>", id: F2 }) }])
  expect(classify(ENDED)?.[0]).toMatchObject({ kind: 'file', ended: true })
  expect(classify(RECEIVED)?.[0]).not.toHaveProperty('ended')
  expect(parseFileNotice(ENDED)).toMatchObject({ state: 'received', saved: SAVED })
})

test('classify: malformed elements are not cards (human text stays text); words after a notice are no notice', () => {
  expect(classify(`${head('x', 'y')}${tail(F1)} state="received"`)).toBeNull() // never closed
  expect(classify(`${head('x', 'y')} file="${F1}" state="received"/>`)).toBeNull() // no holder
  expect(classify(`${head('x', 'y')}${tail(F1)} state="verified"/>`)).toBeNull() // unknown state
  expect(parseFileNotice(`${RECEIVED}\nand then some words`)).toBeNull()
  expect(parseFileNotice('<message_file oops')).toBeNull()
})

test('reducer: a stored message row is the peer card of its words plus its placeholder notes; live PeerDelivered gives the same', () => {
  const [card, ...rest] = history({ role: 'user', text: ROW, turn: 7, peer: origin })
  expect(rest).toEqual([])
  expect(card).toMatchObject({ kind: 'peer', text: 'Site survey and the old drawings, as promised.' })
  const replayed = card?.kind === 'peer' ? card.files : undefined
  expect(replayed?.map((f) => [f.name, f.state, f.id])).toEqual([['site-survey.zip', 'receiving', F1], ['a "b" <c> & d.pdf', 'receiving', F2]])
  const files = [record('receiving', { done: 456000000 }), record('queued', { id: F2, name: 'a "b" <c> & d.pdf', media_type: 'application/pdf', size: 24100000 })]
  const live = run([{ type: 'PeerDelivered', session: S, origin, text: 'Site survey and the old drawings, as promised.', seq: 7, turn: T, files }])
  const entry = live.entries[0]
  expect(entry?.kind === 'peer' ? entry.files : undefined).toEqual(replayed)
  // The records seed the live states.
  expect(live.files.get(`${HOLDER}/${F1}`)?.done).toBe(456000000)
})

test('reducer: ContactFileChanged keeps the newest record; PeerDelivered never overwrites one', () => {
  const s = run([
    { type: 'ContactFileChanged', session: S, file: record('receiving', { done: 10 }) },
    { type: 'ContactFileChanged', session: S, file: record('received', { done: 1200000000, path: SAVED }) },
    { type: 'PeerDelivered', session: S, origin, text: 'hi', seq: 7, turn: T, files: [record('queued')] },
  ])
  expect(s.files.get(`${HOLDER}/${F1}`)).toMatchObject({ state: 'received', path: SAVED })
  expect(s.files.size).toBe(1)
})

test('reducer: a notice row is a file card in history and live (TurnStarted); placeholders never become notice cards', () => {
  expect(history({ role: 'user', text: FAILED, turn: 9 })).toMatchObject([{ kind: 'file', file: { state: 'failed', id: F2 } }])
  const live = run([{ type: 'TurnStarted', session: S, turn: T, text: FAILED, model: 'm', turn_seq: 9 }])
  expect(live.entries).toMatchObject([{ kind: 'file', file: { state: 'failed', id: F2 } }])
  const row = run([{ type: 'TurnStarted', session: S, turn: T, text: ROW, model: 'm', turn_seq: 7 }])
  expect(row.entries).toEqual([])
})

test('fileView: a notice ends the placeholder; a live record overrides it unless a notice already ended the file', () => {
  const note = classify(ROW)?.[1]
  const received = classify(RECEIVED)?.[0]
  if (note?.kind !== 'file' || received?.kind !== 'file') throw new Error('fixture')
  expect(fileView(note.file)).toMatchObject({ state: 'receiving', size: 1200000000, peer: 'from maya' })
  expect(fileView(note.file, record('receiving', { done: 600000000 }))).toMatchObject({ state: 'receiving', done: 600000000, peer: 'from Maya' })
  expect(fileView(note.file, record('receiving'), received.file)).toMatchObject({ state: 'received', path: SAVED })
  expect(fileView(note.file, record('failed', { error: 'disk full on Sky' }))).toMatchObject({ state: 'failed', reason: 'disk full on Sky' })
})

test('cards: receiving shows progress; received shows the thumbnail (never the original) and the holder\'s actions', () => {
  const receiving = render(h(FileCard, { v: recordView(record('receiving', { done: 456000000 })), session: S }))
  expect(receiving).toContain('Receiving · 38%')
  expect(receiving).toContain('aria-valuenow="38"')
  expect(receiving).not.toContain('Open')
  const png = record('received', { name: 'layout.png', media_type: 'image/png', path: '/x/layout.png', size: 884736 })
  qc.setQueryData(queryKeys.contactFileThumbnail(HOLDER, F1), { mime: 'image/jpeg', data: 'AAAA' })
  const html = render(h(FileCard, { v: recordView(png), session: S }))
  expect(html).toContain('src="data:image/jpeg;base64,AAAA"')
  expect(html).not.toContain('asset://')
  for (const label of ['Open', 'Save as…', 'Copy path', '/x/layout.png', 'Received<']) expect(html).toContain(label)
  // No thumbnail (failed or not yet answered): the type icon, the file still a card.
  qc.removeQueries({ queryKey: queryKeys.contactFileThumbnail(HOLDER, F1) })
  expect(render(h(FileCard, { v: recordView(png), session: S }))).not.toContain('<img')
})

test('cards: another of our machines says where it is stored and offers Fetch here; a phone gets the path only', () => {
  const v = recordView(record('received', { holder: OTHER, path: SAVED, machine: '' }))
  const there = render(h(FileCard, { v, session: S }))
  expect(there).toContain('Received · stored on Mac mini')
  expect(there).toContain('Fetch here')
  expect(there).not.toContain('Save as')
  local = false
  const phone = render(h(FileCard, { v: recordView(record('received', { path: SAVED })), session: S }))
  expect(phone).toContain('Copy path')
  expect(phone).toContain(SAVED)
  for (const label of ['Open', 'Save as', 'Fetch here', 'Reveal']) expect(phone).not.toContain(label)
})

test('cards: a failed file shows its reason; sender states queued, sending with progress, sent, failed', () => {
  expect(render(h(FileCard, { v: recordView(record('failed', { error: 'the sender’s file changed' })), session: S }))).toMatch(/Failed[\s\S]*the sender’s file changed/)
  const out = (state: ContactFile['state'], over: Partial<ContactFile> = {}) => render(h(FileCard, { v: recordView(record(state, { direction: 'out', contact: 'Sky', ...over } as Partial<ContactFile>), '/p/site-survey.zip'), session: S }))
  expect(out('queued')).toContain('Queued')
  expect(out('sending', { done: 456000000 })).toContain('Sending · 38% · 456.0 MB of 1.2 GB')
  expect(out('sent')).toContain('Sent · Sky&#x27;s machine received it')
  expect(out('failed', { error: 'expired without a receipt' })).toContain('expired without a receipt')
  expect(out('sent')).toContain('>Open<')
})

test('SendCard: the receipts\' records, live changes over them; the call\'s own paths while none is known', () => {
  const R = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af5'
  const outRec = record('sending', { direction: 'out', exchange: R, contact: 'Sky', done: 0 } as Partial<ContactFile>)
  const tool: Extract<Entry, { kind: 'tool' }> = {
    id: 'e1', kind: 'tool', callId: 'c1', name: 'send_message', pending: false, child: false,
    args: { to: 'peer:sky', message: 'as promised', files: ['/p/site-survey.zip'] },
    result: { output: `peer:sky accepted id=${R}`, isError: false, files: [] },
  }
  expect(render(h(SendCard, { tool, session: S }))).toMatch(/site-survey\.zip[\s\S]*Queued/)
  slice({ ...empty(S), files: new Map([[`${HOLDER}/${F1}`, { ...outRec, state: 'sent' } as ContactFile]]) })
  expect(render(h(SendCard, { tool, session: S }))).toContain('Sent · Sky&#x27;s machine received it')
  expect(sentFiles([R], [[outRec]], new Map([[`${HOLDER}/${F1}`, { ...outRec, done: 5 } as ContactFile]]))).toEqual([{ ...outRec, done: 5 }])
  expect(sentFiles([R], [], new Map([[`${HOLDER}/x`, { ...outRec, exchange: 'other' } as ContactFile]]))).toEqual([])
})

test('messages: a notice row and an ended share\'s errors row are file cards; unreadable markup never shows', () => {
  const [notice] = history({ role: 'user', text: RECEIVED, turn: 9 })
  const html = msg(notice as Entry)
  expect(html).toContain('File notice from theuth')
  expect(html).not.toContain('&lt;message_file')
  const ended = msg({ id: 'e2', kind: 'error', text: ENDED })
  expect(ended).toContain('not given to the agent')
  expect(ended).toContain('site-survey.zip')
  expect(ended).not.toContain('message_file')
  expect(msg({ id: 'e3', kind: 'error', text: '<message_file name="x"/>' })).toContain('File notice (unreadable)')
  expect(msg({ id: 'e4', kind: 'error', text: 'provider refused' })).toContain('provider refused')
  // The message card carries its placeholders as cards, never the tag.
  const [card] = history({ role: 'user', text: ROW, turn: 7, peer: origin })
  const peer = msg(card as Entry)
  expect(peer).toContain('a &quot;b&quot; &lt;c&gt; &amp; d.pdf')
  expect(peer).not.toContain('message_file')
  expect(peer).not.toContain('are arriving')
  expect(peer.match(/Receiving/g)).toHaveLength(2)
  // Its notice row, once in the transcript, ends that card too: the file reads Received with its saved path.
  const [received] = history({ role: 'user', text: RECEIVED, turn: 9 })
  slice({ ...empty(S), entries: [card as Entry, received as Entry] })
  const after = msg(card as Entry)
  expect(after.match(/Receiving/g)).toHaveLength(1)
  expect(after).toContain(SAVED)
})

test('notice rows: with the message row loaded one compact line (no second card); without it the full card', () => {
  const [card] = history({ role: 'user', text: ROW, turn: 7, peer: origin })
  const [received] = history({ role: 'user', text: RECEIVED, turn: 9 })
  const [failed] = history({ role: 'user', text: FAILED, turn: 10 })
  // No message row loaded: the full card, with the ended label for an ended row.
  slice({ ...empty(S), entries: [received as Entry] })
  const alone = msg(received as Entry)
  expect(alone).toContain('File notice from theuth')
  expect(alone).toContain(SAVED)
  expect(msg({ id: 'e2', kind: 'error', text: ENDED })).toContain('not given to the agent')
  expect(msg({ id: 'e2', kind: 'error', text: ENDED })).toContain(SAVED)
  // Loaded: the compact line, the card stays on the message.
  slice({ ...empty(S), entries: [card as Entry, received as Entry, failed as Entry] })
  const line = msg(received as Entry)
  expect(line).toContain('site-survey.zip')
  expect(line).toContain('Received')
  expect(line).toContain('given to the agent')
  expect(line).toContain('Show')
  expect(line).not.toContain('File notice from theuth')
  expect(line).not.toContain(SAVED)
  expect(line).not.toContain('Open')
  expect(msg(failed as Entry)).toContain('Failed: the sender&#x27;s file &lt;changed&gt;')
  const ended = msg({ id: 'e2', kind: 'error', text: ENDED })
  expect(ended).toContain('not given to the agent')
  expect(ended).toContain('Show')
  expect(ended).not.toContain(SAVED)
  // The message's own card carries the stable id the link scrolls to.
  expect(msg(card as Entry)).toContain(`id="contact-file-${HOLDER}-${F1}"`)
})

test('cards: an outgoing sending card with done > 0 shows the percent, sizes and a progress bar', () => {
  const html = render(h(FileCard, { v: recordView(record('sending', { direction: 'out', done: 600000000 } as Partial<ContactFile>), '/p/a.zip'), session: S }))
  expect(html).toContain('Sending · 50% · 600.0 MB of 1.2 GB')
  expect(html).toContain('role="progressbar"')
  expect(html).toContain('aria-valuenow="50"')
})
