import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode, act, createContext, createElement, useContext } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Engines from '@/link/engines'
import type * as Rpc from '@/lib/rpc'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { accessFocus, contactFirstOf, exportName, fingerprint, focusedContact, names, sharedLabel, sharingOf } from '@/lib/contacts'
import { ContactFirstScreen } from '@/components/attention/PlainScreens'
import { KindActions } from '@/components/hud/Hud'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Contact } from '@/rpc/bindings/Contact'
import type { FirstWaiting } from '@/rpc/bindings/FirstWaiting'
import type { Export } from '@/rpc/bindings/Export'
import type { GrantableTool } from '@/rpc/bindings/GrantableTool'
import type { NearbyAgent } from '@/rpc/bindings/NearbyAgent'
import type { PairCeremony } from '@/rpc/bindings/PairCeremony'
import { dispatch } from '@/store/sessions'
import { Dialog } from '@/components/ui/dialog'
import { AddContact, CeremonyBody, type CeremonyView, ContactAccess, Contacts, ShareDialog, SharingItems } from './Contacts'
import { A2aSection } from './Communication'
import { CommunicationNotice, useCommunication } from '@/components/chat/CommunicationNotice'
import type { A2aStatus } from '@/rpc/bindings/A2aStatus'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate, Link: ({ children }: { children: ReactNode }) => children }))
vi.mock('@/link/engines', async (original) => ({ ...(await original<typeof Engines>()), hasLocal: () => true }))
// Menus render their items in place (a click is a select); a select registers its setter under its trigger's label.
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: (p: { children: ReactNode }) => p.children,
  DropdownMenuTrigger: (p: { children: ReactNode }) => p.children,
  DropdownMenuContent: (p: { children: ReactNode }) => createElement('div', { role: 'menu' }, p.children),
  DropdownMenuItem: (p: { children: ReactNode; onSelect?: () => void; disabled?: boolean }) => createElement('button', { role: 'menuitem', disabled: p.disabled, onClick: () => p.onSelect?.() }, p.children),
}))
const selects: Record<string, (v: string) => void> = {}
const SelectCtx = createContext<{ value?: string; onValueChange?: (v: string) => void }>({})
vi.mock('@/components/ui/select', () => ({
  Select: (p: { value?: string; onValueChange?: (v: string) => void; children: ReactNode }) => createElement(SelectCtx.Provider, { value: p }, p.children),
  SelectTrigger: (p: { 'aria-label'?: string }) => {
    const c = useContext(SelectCtx)
    if (p['aria-label'] && c.onValueChange) selects[p['aria-label']] = c.onValueChange
    return createElement('span', { 'aria-label': p['aria-label'], 'data-value': c.value })
  },
  SelectValue: () => null,
  SelectContent: () => null,
  SelectItem: () => null,
}))

const NOW = Date.now()
const BOB: Contact = {
  name: 'bob',
  machine: "Bob's MacBook",
  endpoint: 'k3j9q2m4x7p1r8t5w6y0zabcdefgh',
  fingerprint: 'K3J9 Q2M4 X7P1 R8T5 W6Y0',
  path: 'relay',
  cap: 'read_only',
  exports: ['research', 'ops-chat'],
  first_contact: { research: 'allow' },
  modes: {},
  full_capable: true,
  shared: [],
  blocked: false,
  paired_ms: NOW,
}
const ALICE: Contact = { ...BOB, name: 'alice', machine: 'studio', path: 'lan', cap: 'mutating', exports: ['ops-chat'], first_contact: {}, fingerprint: 'AAAA BBBB CCCC DDDD EEEE', full_capable: false }
const EXPORTS: Export[] = [
  { name: 'research', target: { kind: 'agent', agent: 'research@studio' }, description: 'research@studio', paused: null },
  { name: 'ops-chat', target: { kind: 'chat', session: 's-ops' }, description: 'Ops chat', paused: 'chat archived' },
]
const NEARBY: NearbyAgent[] = [
  { name: 'Shopbot', service: '_agent._tcp', addr: '192.168.8.40:8080', eid: null, card: 'http://192.168.8.40:8080/.well-known/agent-card.json', verified: false },
  { name: 'Bob', service: '_a2a._tcp', addr: '192.168.8.21:4433', eid: 'k3j9q2m4x7p1r8t5', card: '/.well-known/agent-card.json', verified: true },
]
const CER: PairCeremony = { id: 'c1', peer: 'k3j9q2m4x7p1r8t5w6y0zabcdefgh', name: 'Bob', os_name: 'macOS', addr: "Bob's MacBook", code: '482913', state: 'waiting', expires_ms: NOW + 120_000 }
// What `a2a_status` lists: the contact bob (as a peer, with its chat export) beside a real HTTP peer, lab.
const A2A: A2aStatus = {
  enabled: false,
  listening: false,
  address: null,
  public_url: null,
  error: null,
  peers: [
    { name: 'bob', binding: 'b1', outbound: true, inbound: true, sessions: [], exports: ['ops-chat'], card: null, error: null },
    { name: 'lab', binding: 'b2', outbound: true, inbound: true, sessions: ['s-lab'], exports: ['docs'], card: 'Lab', error: null },
  ],
  exports: [
    { name: 'ops-chat', session: 's-ops', peer: 'bob', url: 'http://127.0.0.1:7780/agents/ops-chat/rpc', mode: 'cautious' },
    { name: 'docs', session: 's-docs', peer: 'lab', url: 'http://127.0.0.1:7780/agents/docs/rpc', mode: null },
  ],
}
const api = {
  contact_list: vi.fn(async (): Promise<Contact[]> => [BOB, ALICE]),
  a2a_exports: vi.fn(async (): Promise<Export[]> => EXPORTS),
  contact_window: vi.fn(async (open: boolean) => (open ? { ticket: 'theuth-contact:1.MFRGGZDFMZTWQ2LK', expires_ms: NOW + 600_000 } : null)),
  contact_nearby: vi.fn(async () => NEARBY),
  contact_pair: vi.fn(async (_t: unknown) => CER),
  contact_answer: vi.fn(async (_id: string, _accept: boolean) => null),
  contact_update: vi.fn(async (_name: string, _patch: unknown) => BOB),
  contact_remove: vi.fn(async (_name: string, _block: boolean) => null),
  contact_first_answer: vi.fn(async (_c: string, _e: string, _a: string) => null),
  contact_first_waiting: vi.fn(async (): Promise<FirstWaiting[]> => []),
  a2a_export_put: vi.fn(async (_x: Export) => null),
  a2a_export_remove: vi.fn(async (_name: string) => null),
  list_sessions: vi.fn(async () => [
    { id: 's-ops', parent: null, archived: false, title: 'Ops chat', created_at: '2026-11-09T10:00:00Z', here: true, owner: 'me' },
    { id: 's-plan', parent: null, archived: false, title: 'Launch plan', created_at: '2026-11-09T10:00:00Z', here: false, owner: 'xeus' },
    { id: 's-old', parent: null, archived: true, title: 'Old', created_at: '2026-11-09T10:00:00Z', here: true, owner: 'me' },
  ]),
  agents_list: vi.fn(async () => [{ file: { name: 'research' }, desk: 'd1', status: 'active' }]),
  mesh_status: vi.fn(async () => ({ self_id: 'me', self_name: 'studio', peers: [{ id: 'xeus', name: 'xeus' }] })),
  attention_dismiss: vi.fn(async () => null),
  attention_snooze: vi.fn(async () => null),
  sandboxes_list: vi.fn(async () => []),
  a2a_status: vi.fn(async (): Promise<A2aStatus> => A2A),
  a2a_grantable_tools: vi.fn(async () =>
    [
      { name: 'knowledge_search', risk: 'read_only' },
      { name: 'mcp:files/search', risk: 'read_only', mcp_server: 'files' },
      { name: 'read_url', risk: 'read_only' },
      { name: 'search_web', risk: 'read_only' },
    ] as GrantableTool[],
  ),
  reload_a2a: vi.fn(async (): Promise<A2aStatus> => A2A),
  get_communication: vi.fn(async () => ({ effective: true, global: true, opted_out: false, inherited_from: null, source: 'config' }) as unknown as CommunicationState),
  config_flag_get: vi.fn(async () => ({ key: 'a2a.enabled', enabled: false, running: false, restart_required: false })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const qcNew = () => new QueryClient({ defaultOptions: { queries: { retry: false } } })
const roots: (() => Promise<void>)[] = []
const realTimeout = setTimeout
const tick = () => new Promise<void>((r) => realTimeout(r, 0))
const run = (fn: () => unknown) =>
  act(async () => {
    await fn()
    await tick()
  })
async function mount(el: ReactNode, qc = qcNew()) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await run(() => root.render(createElement(QueryClientProvider, { client: qc, children: el })))
  await run(() => undefined)
  roots.push(() => run(() => root.unmount()))
  return { qc, unmount: roots[roots.length - 1]! }
}
const body = () => document.body.textContent ?? ''
const button = (label: string) => [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === label || b.getAttribute('aria-label') === label)
const click = (label: string) =>
  run(() => {
    const b = button(label)
    if (!b) throw new Error(`no button ${label}`)
    b.click()
  })

beforeEach(() => {
  vi.clearAllMocks()
  api.contact_list.mockImplementation(async () => [BOB, ALICE])
  api.contact_window.mockImplementation(async (open: boolean) => (open ? { ticket: 'theuth-contact:1.MFRGGZDFMZTWQ2LK', expires_ms: NOW + 600_000 } : null))
  api.contact_pair.mockImplementation(async () => CER)
  api.a2a_exports.mockImplementation(async () => EXPORTS)
  api.contact_first_waiting.mockImplementation(async () => [])
  api.a2a_status.mockImplementation(async () => A2A)
  for (const k of Object.keys(selects)) delete selects[k]
})
/** Unmount every root, then empty the page (clearing first would pull nodes out from under React). */
async function reset() {
  for (const u of roots.splice(0)) await u()
  document.body.innerHTML = ''
}
afterEach(reset)

test('contact words: export names, fingerprint, first-contact ids, sharing and its chip', () => {
  expect(exportName('Ops chat: deploys!', [])).toBe('ops-chat-deploys')
  expect(exportName('research@studio', ['research-studio'])).toBe('research-studio-2')
  expect(exportName('???', [])).toBe('export')
  expect(fingerprint('k3j9q2m4x7p1r8t5w6y0zabc')).toBe('k3j9 q2m4 x7p1 r8t5 w6y0')
  // A real endpoint id reads as the engine's `Contact.fingerprint` (base32), the ceremony's subtitle too.
  expect(fingerprint('e7ac7242a74ef96fc15d964ef848e564f6971cf2bc08b08a94308c4a6e020825')).toBe('46WH EQVH J34W 7QK5 SZHP')
  expect(contactFirstOf('contact_first:bob:ops-chat')).toEqual({ contact: 'bob', exp: 'ops-chat' })
  expect(contactFirstOf('approval:1')).toBeNull()
  expect(names([BOB, ALICE, { name: 'carol' }])).toBe('bob, alice and carol')
  const s = sharingOf('s-ops', EXPORTS, [BOB, ALICE, { ...BOB, name: 'eve', blocked: true }])
  expect(s?.contacts.map((c) => c.name)).toEqual(['bob', 'alice'])
  expect(sharedLabel(s)).toBe('Shared with bob · read only, alice · can change things · paused (chat archived)')
  expect(sharedLabel(sharingOf('s-plan', EXPORTS, [BOB]))).toBeNull()
  expect(sharedLabel(sharingOf('s-ops', EXPORTS, []))).toBeNull()
})

test('188 ?focus=access:<contact>: the contact it names, colons and all; any other focus names none', () => {
  for (const name of ['bob', 'Bob: lab', 'access:x']) expect(focusedContact(accessFocus(name))).toBe(name)
  for (const focus of [null, '', 'access:', 'model', 'spend', 'contact:bob']) expect(focusedContact(focus)).toBeNull()
})

test('§12.7 Contacts: each row has name, machine, cap, fingerprint, path and the exports it may reach (paused named)', async () => {
  await mount(createElement(Contacts))
  const t = body()
  expect(t).toContain('A contact reaches only what you share, and you choose how each time.')
  // Both of bob's shares are cautious: his cap still binds and shows.
  expect(t).toContain("Bob's MacBook · read only")
  expect(t).toContain('K3J9 Q2M4 X7P1 R8T5 W6Y0')
  expect(t).toContain('Relay')
  expect(t).toContain('studio · can change things')
  expect(t).toContain('LAN')
  expect(t).toContain('research@studio')
  expect(t).toContain('Ops chat · Paused (chat archived)')
})

test('§12.7 Contacts: empty', async () => {
  api.contact_list.mockImplementation(async () => [])
  await mount(createElement(Contacts))
  expect(body()).toContain('No contacts yet.')
})

test('⋯ Rename → contact_update(name), Block… and Remove… confirm → contact_remove(name, block), Unblock → contact_update(blocked: false)', async () => {
  api.contact_list.mockImplementation(async () => [BOB, { ...ALICE, blocked: true }])
  await mount(createElement(Contacts))
  await click('Rename')
  const input = document.querySelector<HTMLInputElement>('input[aria-label="Contact name"]')!
  input.value = 'robert'
  await run(() => input.blur())
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: 'robert', cap: null, exports: null, blocked: null, modes: null })
  await click('Block…')
  expect(document.querySelector('[role=alertdialog]')?.textContent).toContain('Block bob?')
  expect(api.contact_remove).not.toHaveBeenCalled()
  await click('Block')
  await vi.waitFor(() => expect(api.contact_remove).toHaveBeenCalledWith('bob', true))
  const removes = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Remove…')
  await run(() => removes[0]!.click())
  await click('Remove')
  await vi.waitFor(() => expect(api.contact_remove).toHaveBeenCalledWith('bob', false))
  await click('Unblock')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('alice', { name: null, cap: null, exports: null, blocked: false, modes: null }))
  await click('Choose what bob may reach…')
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'contacts' }, search: { focus: 'access:bob' } })
})

test('Add contact: opens the window, Nearby lists theuth agents first and other vendors as A2A · HTTPS; closing closes the window', async () => {
  const { unmount } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  expect(api.contact_window).toHaveBeenCalledWith(true)
  const t = body()
  expect(t).toContain('Nearby agents')
  expect(t.indexOf('Bob')).toBeLessThan(t.indexOf('Shopbot'))
  expect(t).toContain('verified')
  expect(t).toContain('A2A · HTTPS')
  expect(t).toContain('other vendor · _agent._tcp · 192.168.8.40:8080')
  await click('Add Shopbot')
  expect(api.contact_pair).toHaveBeenCalledWith({ kind: 'card_url', url: 'http://192.168.8.40:8080/.well-known/agent-card.json' })
  await unmount()
  expect(api.contact_window).toHaveBeenLastCalledWith(false)
})

test('Invite: the ticket as text and QR, Copy; pasting a ticket or a card URL starts the ceremony', async () => {
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn(), tab: 'invite' }))
  expect(document.querySelector('[aria-label="Invite ticket"]')?.textContent).toBe('theuth-contact:1.MFRGGZDFMZTWQ2LK')
  expect(document.querySelector('svg')).not.toBeNull()
  expect(body()).toContain('It works once')
  const paste = document.getElementById('contact-paste') as HTMLInputElement
  const type = (v: string) =>
    run(() => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(paste, v)
      paste.dispatchEvent(new Event('input', { bubbles: true }))
    })
  await type('theuth-contact:1.THEIRS')
  await click('Connect')
  expect(api.contact_pair).toHaveBeenCalledWith({ kind: 'ticket', ticket: 'theuth-contact:1.THEIRS' })
})

test('ceremony: the code and the check line, Cancel focused (never Accept); Accept → waiting; paired → added; Done closes', async () => {
  const onClose = vi.fn()
  const { qc } = await mount(createElement(AddContact, { onClose, onAccess: vi.fn() }))
  await click('Add Bob')
  expect(api.contact_pair).toHaveBeenCalledWith({ kind: 'nearby', eid: 'k3j9q2m4x7p1r8t5' })
  expect(document.querySelector('[role=group][aria-label="Contact code"]')?.textContent).toBe('482 913')
  expect(body()).toContain('Check that Bob shows the same code')
  expect(body()).toContain("Bob's MacBook · macOS · k3j9 q2m4 x7p1 r8t5 w6y0")
  expect(document.activeElement).toBe(button('Cancel'))
  // The dialog is described by the line that pairing alone shares nothing.
  const dialog = document.querySelector('[role=dialog]')!
  expect(document.getElementById(dialog.getAttribute('aria-describedby')!)?.textContent).toBe(
    'A paired contact reaches nothing until you share an agent or chat with them, and you choose how: Full collaboration (their agent and yours talk freely) or Cautious (plain text, one limited turn per message, read-only tools).',
  )
  await click('Accept')
  expect(api.contact_answer).toHaveBeenCalledWith('c1', true)
  expect(body()).toContain('Waiting for Bob to press Accept…')
  expect(button('Accept')?.disabled).toBe(true)
  await run(() => dispatch([{ type: 'contact_pair', data: { ...CER, state: 'paired' } }] as never))
  await run(() => void qc.setQueryData(queryKeys.contactPair, { c1: { ...CER, state: 'paired' } }))
  expect(body()).toContain('Bob is now a contact. It reaches nothing until you share an agent or a chat with it.')
  await click('Done')
  expect(onClose).toHaveBeenCalled()
})

test('ceremony: The codes differ refuses and says why; Cancel refuses and goes back to the tabs', async () => {
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  await click('Add Bob')
  await click('The codes differ')
  expect(api.contact_answer).toHaveBeenCalledWith('c1', false)
  expect(body()).toContain('Not added: the codes did not match')
  await click('Back')
  expect(body()).toContain('Nearby agents')
  api.contact_pair.mockImplementation(async () => ({ ...CER, id: 'c2' }))
  await click('Add Bob')
  await click('Cancel')
  expect(api.contact_answer).toHaveBeenCalledWith('c2', false)
  expect(body()).toContain('Nearby agents')
})

test('§7.1 Nearby: a card that fails verification on Add shows unverified and cannot start the ceremony again', async () => {
  api.contact_pair.mockImplementation(() => Promise.reject(new Error('unverified card: the signature does not verify')))
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  await click('Add Bob')
  expect(body()).toContain('unverified card')
  await click('Back')
  expect(body()).toContain('unverified')
  expect(body()).not.toMatch(/Bob\s*verified/)
  expect(button('Add Bob')?.disabled).toBe(true)
  expect(button('Add Shopbot')?.disabled).toBe(false)
})

test('ceremony: a stranger\'s waiting ceremony while the window is open shows its code', async () => {
  const { qc } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  await run(() => void qc.setQueryData(queryKeys.contactPair, { c9: { ...CER, id: 'c9', name: 'Carol', code: '111222' } }))
  expect(body()).toContain('Add Carol as a contact')
  expect(body()).toContain('111 222')
})

test('Add contact: a cancelled dial\'s late reply never lands on the next dialog', async () => {
  const resolves: ((c: PairCeremony) => void)[] = []
  api.contact_pair.mockImplementation(() => new Promise<PairCeremony>((r) => resolves.push(r)))
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  await click('Add Bob')
  await click('Cancel')
  await click('Add Shopbot')
  expect(body()).toContain('Connecting to Shopbot')
  await run(() => resolves[0]!(CER))
  expect(body()).toContain('Connecting to Shopbot')
  expect(body()).not.toContain('482 913')
  expect(api.contact_answer).toHaveBeenCalledWith('c1', false)
  await run(() => resolves[1]!({ ...CER, id: 'c2', name: 'Shopbot', code: '111222' }))
  expect(body()).toContain('111 222')
  expect(api.contact_answer).not.toHaveBeenCalledWith('c2', false)
})

test('contact_window: a remount\'s close and reopen reach the engine in call order, each after the last settles', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  try {
    // The open answers slowly, the close fast: unordered, the close would land last and leave no window.
    api.contact_window.mockImplementation((open: boolean) => new Promise((r) => setTimeout(() => r(open ? { ticket: 'theuth-contact:1.T', expires_ms: NOW + 600_000 } : null), open ? 100 : 10)))
    const first = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
    await first.unmount()
    const second = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
    const calls = () => api.contact_window.mock.calls.map(([open]) => open)
    expect(calls()).toEqual([true])
    await run(() => vi.advanceTimersByTime(100))
    expect(calls()).toEqual([true, false])
    await run(() => vi.advanceTimersByTime(10))
    expect(calls()).toEqual([true, false, true])
    await run(() => vi.advanceTimersByTime(100))
    expect(calls()).toEqual([true, false, true])
    // Drain the last close before real timers come back, so the next test's open is not queued behind it.
    await second.unmount()
    await run(() => vi.advanceTimersByTime(10))
    expect(calls()).toEqual([true, false, true, false])
  } finally {
    vi.useRealTimers()
  }
})

test('CeremonyBody: every end state', () => {
  const view = (v: CeremonyView, error: string | null = null) =>
    renderToStaticMarkup(
      createElement(Dialog, {
        open: true,
        children: createElement(CeremonyBody, { name: 'Bob', detail: '', ceremony: CER, view: v, error, onAccept: vi.fn(), onCancel: vi.fn(), onMismatch: vi.fn(), onBack: vi.fn(), onDone: vi.fn(), onAccess: vi.fn() }),
      }),
    )
  expect(view('starting')).toContain('Connecting to Bob…')
  expect(view('rejected')).toContain('Not added: Bob pressed Cancel, or saw a different code.')
  expect(view('expired')).toContain('The check timed out')
  expect(view('mismatch')).toContain('the codes did not match')
  expect(view('paired')).toContain('Choose what Bob may reach')
})

test('§7.6 Contacts\' access: cap → contact_update(cap); first contact → contact_first_answer; paused named', async () => {
  await mount(createElement(ContactAccess))
  expect(body()).toContain('Ops chat · Paused (chat archived)')
  expect(document.querySelector('[aria-label="Cap of bob"]')?.getAttribute('data-value')).toBe('read_only')
  expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('allow')
  expect(document.querySelector('[aria-label="First contact from bob to ops-chat"]')?.getAttribute('data-value')).toBe('ask')
  await run(() => selects['Cap of bob']!('mutating'))
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: 'mutating', exports: null, blocked: null, modes: null })
  await run(() => selects['First contact from bob to ops-chat']!('block'))
  expect(api.contact_first_answer).toHaveBeenCalledWith('bob', 'ops-chat', 'block')
})

test('§12.10 export picker: agents and chats (not archived, with their machine); a new chat export is put then granted in the chosen mode; an existing one only granted', async () => {
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: [] }, ALICE])
  await mount(createElement(ContactAccess))
  await click('Share with bob…')
  const agents = document.querySelector('[role=group][aria-label="Agents"]')!.textContent
  expect(agents).toContain('theuth')
  expect(agents).toContain('research@studio')
  const chats = document.querySelector('[role=group][aria-label="Chats"]')!.textContent
  expect(chats).toContain('Ops chat')
  expect(chats).toContain('Launch plan')
  expect(chats).toContain('xeus')
  expect(chats).not.toContain('Old')
  // 186 D3: picking a row grants nothing yet; the mode is the second, required step.
  await click('Launch planxeus')
  expect(api.contact_update).not.toHaveBeenCalled()
  await click('Full collaboration: share Launch plan with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['launch-plan'], blocked: null, modes: { 'launch-plan': 'full' } }))
  expect(api.a2a_export_put).toHaveBeenCalledWith({ name: 'launch-plan', target: { kind: 'chat', session: 's-plan' }, description: 'Launch plan', paused: null })
  vi.clearAllMocks()
  await click('research@studioagent')
  await click('Cautious: share research@studio with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['research'], blocked: null, modes: { research: 'cautious' } }))
  expect(api.a2a_export_put).not.toHaveBeenCalled()
})

test('§12.10 unset: taking an export back updates the grant; the export goes once nobody holds it', async () => {
  await mount(createElement(ContactAccess))
  await click('Stop sharing research@studio with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['ops-chat'], blocked: null, modes: null }))
  expect(api.a2a_export_remove).toHaveBeenCalledWith('research')
  vi.clearAllMocks()
  // alice still holds ops-chat: the export stays.
  await click('Stop sharing Ops chat with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['research'], blocked: null, modes: null }))
  expect(api.a2a_export_remove).not.toHaveBeenCalled()
})

test('183 Z3-R2 tools: each export row lists its allowlist (default the public web); × and Add tool put the export', async () => {
  api.a2a_exports.mockImplementation(async () => [EXPORTS[0]!, { ...EXPORTS[1]!, tools: [] } as Export])
  await mount(createElement(ContactAccess))
  expect(body()).toContain('Cautious shares can use only these tools. Everything else, including your files, sessions, knowledge and vault, is hidden from them.')
  expect(body()).toContain('Only the public web, read-only readers and read-only MCP tools can be added.')
  const tools = (label: string) => document.querySelector(`[role=group][aria-label="Tools ${label} offers contacts"]`)!
  expect(tools('research@studio').textContent).toContain('search_webread_url')
  expect(api.a2a_grantable_tools).toHaveBeenCalled()
  expect(tools('Ops chat').textContent).toContain('No tools: answers only')
  await click('Remove read_url from research@studio')
  await vi.waitFor(() => expect(api.a2a_export_put).toHaveBeenCalledWith({ ...EXPORTS[0], tools: ['search_web'] }))
  vi.clearAllMocks()
  // Add tool offers only what the engine lets an export grant (Z4: `a2a_grantable_tools`) and the list lacks; the first row is bob's research.
  const items = () => [...tools('research@studio').querySelectorAll('[role=menuitem]')]
  expect(items().map((b) => b.querySelector('.font-mono')?.textContent)).toEqual(['knowledge_search', 'mcp:files/search'])
  // Z4-R1: a reader warns that it sees all of its kind; an MCP tool that its risk is the server's word.
  expect(items()[0]!.textContent).toContain('Sees ALL your knowledge, not just what you share here')
  expect(items()[1]!.textContent).toContain('Read-only as the server declares it; for a server you don’t control, pin its risk on Settings › Tools › MCP')
  await run(() => (items()[0] as HTMLButtonElement).click())
  await vi.waitFor(() => expect(api.a2a_export_put).toHaveBeenCalledWith({ ...EXPORTS[0], tools: ['search_web', 'read_url', 'knowledge_search'] }))
})

test('183 Z4-R1 tools line: once a reader is granted it names what contacts can read, all of it', async () => {
  api.a2a_exports.mockImplementation(async () => [
    { ...EXPORTS[0]!, tools: ['read_url', 'knowledge_get', 'nb_search'] } as Export,
    { ...EXPORTS[1]!, tools: ['doc_read', 'knowledge_search'] } as Export,
  ])
  await mount(createElement(ContactAccess))
  expect(body()).toContain(
    'Cautious shares can use only these tools. Everything else, including your files, sessions and vault, is hidden from them. The readers granted below let them read ALL your knowledge, notebooks and notes and documents, not just what you share.',
  )
  expect(body()).not.toContain('sessions, knowledge and vault')
  await reset()
  api.a2a_exports.mockImplementation(async () => [{ ...EXPORTS[0]!, tools: ['artifact_read'] } as Export])
  await mount(createElement(ContactAccess))
  expect(body()).toContain('including your files, sessions, knowledge and vault, is hidden from them. The readers granted below let them read ALL your artifacts, not just what you share.')
})

test('⋯ Share with contact…: Cautious or Full collaboration grants this chat, Shared · Stop takes it back; Stop sharing with <contacts> removes the export', async () => {
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  expect(body()).toContain("They see only what this chat's agent sends back")
  expect(body()).toContain('You can switch this share between Full collaboration and Cautious, or stop sharing, at any time.')
  await click('Stop sharing with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['research'], blocked: null, modes: null }))
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: [] }])
  await mount(createElement(ShareDialog, { session: 's-new', title: 'New chat', onClose: vi.fn() }))
  // 186: no grant button takes focus on open, so Enter never shares in full by accident.
  expect(document.querySelectorAll('[role=group][aria-label="Share this chat with bob"] button')).toHaveLength(2)
  expect(document.activeElement?.tagName).not.toBe('BUTTON')
  await click('Cautious: share this chat with bob')
  await vi.waitFor(() => expect(api.a2a_export_put).toHaveBeenCalledWith({ name: 'new-chat', target: { kind: 'chat', session: 's-new' }, description: 'New chat', paused: null }))
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['new-chat'], blocked: null, modes: { 'new-chat': 'cautious' } })
  await reset()
  api.contact_list.mockImplementation(async () => [BOB, ALICE])
  const onShare = vi.fn()
  await mount(createElement(SharingItems, { session: 's-ops', onShare }))
  await click('Share with contact…')
  expect(onShare).toHaveBeenCalled()
  await click('Stop sharing with bob and alice')
  await vi.waitFor(() => expect(api.a2a_export_remove).toHaveBeenCalledWith('ops-chat'))
})

test('Stop sharing is absent for a chat nobody can message', async () => {
  await mount(createElement(SharingItems, { session: 's-plan', onShare: vi.fn() }))
  expect(body()).toContain('Share with contact…')
  expect(body()).not.toContain('Stop sharing')
})

const FIRST: AttentionItem = {
  id: 'contact_first:bob:research',
  kind: 'contact_first',
  title: "bob (Bob's MacBook) wants to reach research",
  detail: null,
  at_ms: NOW,
  task: null,
  agent: null,
  session: null,
  approval: null,
  decision: null,
  packet: null,
  ask: null,
  turn: null,
  rank: 5,
}

test('W-27 first contact: Allow once → ask, Always → allow, Block → block (contact and export from the id)', async () => {
  await mount(createElement(KindActions, { item: FIRST }))
  await click('Allow once')
  await click('Always')
  await click('Block')
  expect(api.contact_first_answer.mock.calls).toEqual([
    ['bob', 'research', 'ask'],
    ['bob', 'research', 'allow'],
    ['bob', 'research', 'block'],
  ])
})

test('ContactFirstScreen: who asks, what it wants to reach, the share\'s mode and the three answers', async () => {
  api.contact_first_waiting.mockImplementation(async () => [{ contact: 'bob', machine: "Bob's MacBook", export: 'research', session: 's1', since_ms: NOW, waiting: 1, preview: 'hi', mode: 'full' }])
  await mount(createElement(ContactFirstScreen, { item: FIRST }))
  const t = body()
  expect(t).toContain("bob (Bob's MacBook) wants to reach research")
  expect(t).toContain('Wants to reach')
  // research is an agent export: the hint names "this agent".
  await vi.waitFor(() => expect(body()).toContain('Full collaboration: Their agent and this agent talk freely'))
  expect(t).not.toContain('limited trust')
  expect(t).toContain('Allow once lets this message through. Always lets every later message to this export through. Block blocks this contact entirely.')
  for (const b of ['Allow once', 'Always', 'Block']) expect(button(b)).toBeDefined()
})

function Listeners() {
  useInvalidateOn()
  return null
}

test('the contact_nearby and contact_pair events write the cache; paired refetches contacts', async () => {
  const { qc } = await mount(createElement(Listeners))
  const spy = vi.spyOn(qc, 'invalidateQueries')
  await run(() => dispatch([{ type: 'contact_nearby', data: NEARBY }] as never))
  expect(qc.getQueryData(queryKeys.contactNearby)).toEqual(NEARBY)
  await run(() => dispatch([{ type: 'contact_pair', data: CER }, { type: 'contact_pair', data: { ...CER, id: 'c2', state: 'paired' } }] as never))
  expect(Object.keys(qc.getQueryData(queryKeys.contactPair) as object)).toEqual(['c1', 'c2'])
  expect(spy).toHaveBeenCalledWith({ queryKey: queryKeys.contacts })
})

test('188 Messaging › A2A lists only the HTTP peers: a contact is no peer row and reads nowhere in roots; its export has no URL to copy', async () => {
  await mount(createElement(A2aSection, { state: undefined }))
  const t = body()
  expect(t).toContain("A2A over HTTPAn HTTP listener, client and credential for agents outside theuth. Contacts don't need it")
  // a2a_status says the engine runs without it: the listener row says only that.
  expect(t).toContain('Off')
  expect(t).not.toContain('Disabled')
  // No second contacts list: Network › Contacts is the one.
  expect(t).not.toContain('bobcontact')
  expect(t).not.toContain('theuth/a2a/1 over iroh')
  expect(t).not.toContain('No peers or exports yet')
  // The HTTP peer lab is listed, the contact bob is not.
  expect(t.match(/local roots? may send/g)).toEqual(['local root may send'])
  expect(t).toContain('1 local root may send · may call docs')
  expect(t).not.toContain('0 local roots may send')
  // The contact's chat export has no HTTP URL to copy; the HTTP peer's export keeps it.
  expect(t).toContain('reached by the contact over theuth/a2a/1, cautious: one capped turn per message')
  expect(t).not.toContain('http://127.0.0.1:7780/agents/ops-chat/rpc')
  expect(button('Copy URL of ops-chat')).toBeUndefined()
  expect(button('Copy URL of docs')).toBeDefined()
})

function NoticeFor({ session }: { session: string }) {
  const { state, exports } = useCommunication(session)
  return createElement(CommunicationNotice, { session, state, exports })
}

test('the chat banner names HTTP-peer exports only: a chat shared with a contact carries no stale "Exported" banner', async () => {
  await mount(createElement(NoticeFor, { session: 's-ops' }))
  await vi.waitFor(() => expect(api.contact_list).toHaveBeenCalled())
  expect(body()).not.toContain('Exported')
  await reset()
  await mount(createElement(NoticeFor, { session: 's-docs' }))
  await vi.waitFor(() => expect(body()).toContain('Exported: docs → lab'))
})

const WAIT: FirstWaiting = { contact: 'bob', machine: "Bob's MacBook", export: 'ops-chat', session: 's-ops', since_ms: NOW, waiting: 1, preview: 'hello', mode: 'cautious' }

test('183 F3 Communication: a held first message sits under its export row; Allow once · Always · Block answer it; no machine, no parentheses', async () => {
  api.contact_first_waiting.mockImplementation(async () => [WAIT, { ...WAIT, contact: 'alice', machine: '' }])
  await mount(createElement(A2aSection, { state: undefined }))
  await vi.waitFor(() => expect(body()).toContain("bob (Bob's MacBook) is waiting for you to allow its first message to ops-chat"))
  expect(body()).toContain('alice is waiting for you to allow its first message to ops-chat')
  expect(body()).not.toContain('alice ()')
  const buttons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Allow once')
  expect(buttons).toHaveLength(2)
  await run(() => (buttons[0] as HTMLElement).click())
  expect(api.contact_first_answer).toHaveBeenLastCalledWith('bob', 'ops-chat', 'ask')
  // One answer per row: bob's are spent, alice's still answer.
  const always = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Always') as HTMLButtonElement[]
  expect(always.map((b) => b.disabled)).toEqual([true, false])
  await run(() => always[1]!.click())
  expect(api.contact_first_answer).toHaveBeenLastCalledWith('alice', 'ops-chat', 'allow')
})

test('183 F3 Contacts: each contact\'s row carries its own waiting line and answers; 186: a queued message is no hold and never shows here', async () => {
  api.contact_first_waiting.mockImplementation(async () => [WAIT, { ...WAIT, export: 'research', queued: true, position: 1 }])
  await mount(createElement(Contacts))
  await vi.waitFor(() => expect(body()).toContain("bob (Bob's MacBook) is waiting for you to allow its first message to ops-chat"))
  expect(body().match(/is waiting for you/g)).toHaveLength(1)
  await click('Always')
  expect(api.contact_first_answer).toHaveBeenCalledWith('bob', 'ops-chat', 'allow')
})

test('183 F10 Contacts: "Shared with you" lists what the contact shares, marking asks-first and paused; nothing when empty', async () => {
  api.contact_list.mockImplementation(async () => [
    {
      ...BOB,
      shared: [
        { name: 'ops', title: 'Ops', kind: 'chat', state: 'allowed', mode: 'full' },
        { name: 'bot', title: 'Bot', kind: 'agent', state: 'ask_first', mode: 'cautious' },
        { name: 'old', title: 'Old', kind: 'chat', state: 'unavailable', mode: 'cautious' },
      ],
    },
    ALICE,
  ])
  await mount(createElement(Contacts))
  expect(body()).toContain('Shared with you: Ops (full), Bot (asks first), Old (paused)')
  expect(body().match(/Shared with you/g)).toHaveLength(1)
})

test('183 F10 Contacts\' access: a granted export reads "<contact> reaches this as …/<export>" with the full form in the tooltip', async () => {
  await mount(createElement(ContactAccess))
  expect(body()).toContain('bob reaches this as …/ops-chat')
  expect(document.querySelector('[title*="peer:<the name bob gave you>/ops-chat"]')).not.toBeNull()
})

test('183 F2: after Share… the engine stores Always allow and the row shows it; an absent entry shows Ask first', async () => {
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: [] }])
  await mount(createElement(ContactAccess))
  await click('Share with bob…')
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['research'], first_contact: { research: 'allow' } }])
  await click('research@studioagent')
  await click('Cautious: share research@studio with bob')
  await vi.waitFor(() => expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('allow'))
  expect(body()).toContain('A new share starts on "Always allow"')
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['research'], first_contact: {} }])
  await mount(createElement(ContactAccess))
  expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('ask')
})

test('186 D3, D10: sharing is a required choice — two equal buttons, neither preselected; Full collaboration is off for a contact on an older build', async () => {
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: [] }, { ...ALICE, exports: [] }])
  await mount(createElement(ShareDialog, { session: 's-new', title: 'New chat', onClose: vi.fn() }))
  const choice = (who: string) => [...document.querySelectorAll(`[role=group][aria-label="Share this chat with ${who}"] button`)] as HTMLButtonElement[]
  for (const who of ['bob', 'alice']) {
    expect(choice(who).map((b) => b.querySelector('.font-medium')?.textContent)).toEqual(['Full collaboration', 'Cautious'])
    // Equal weight: one look for both, nothing pressed or primary.
    expect(new Set(choice(who).map((b) => b.className)).size).toBe(1)
    expect(choice(who).some((b) => b.getAttribute('aria-pressed') === 'true')).toBe(false)
  }
  expect(body()).toContain('Their agent and this chat’s agent talk freely, with Markdown, links and images, and this chat’s agent works with its normal tools.')
  expect(body()).toContain('Plain text, one limited turn per message, read-only tools.')
  expect(choice('bob').map((b) => b.disabled)).toEqual([false, false])
  expect(choice('alice').map((b) => b.disabled)).toEqual([true, false])
  expect(choice('alice')[0]!.textContent).toContain('update studio to share in full')
  // Nothing is granted until a mode is pressed.
  expect(api.contact_update).not.toHaveBeenCalled()
  await click('Full collaboration: share this chat with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: ['new-chat'], blocked: null, modes: { 'new-chat': 'full' } }))
})

test('186: each grant shows its mode and changes it; a full grant hides the tools editor, and the cap is off once every grant is full', async () => {
  const FULL = { ...BOB, modes: { research: 'full' as const } }
  api.contact_list.mockImplementation(async () => [FULL, ALICE])
  await mount(createElement(ContactAccess))
  const mode = (exp: string, who = 'bob') => document.querySelector(`[aria-label="Mode of ${exp} for ${who}"]`)?.getAttribute('data-value')
  expect(mode('research')).toBe('full')
  // No entry is cautious (D2).
  expect(mode('ops-chat')).toBe('cautious')
  expect(document.querySelector('[role=group][aria-label="Tools research@studio offers contacts"]')).toBeNull()
  expect(body()).toContain('Full collaboration: Their agent and this agent talk freely')
  expect(document.querySelector('[role=group][aria-label="Tools Ops chat offers contacts"]')).not.toBeNull()
  await run(() => selects['Mode of ops-chat for bob']!('full'))
  // Only the changed entry: a stale snapshot never re-sends research's mode (the engine merges entry by entry).
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: null, blocked: null, modes: { 'ops-chat': 'full' } })
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...FULL, exports: ['research'] }])
  await mount(createElement(ContactAccess))
  expect(body()).toContain('Every share with bob is full: the cap applies to cautious shares only.')
  await reset()
  // The Contacts list, the shared chat's header chip and the chat's share dialog say it too.
  api.contact_list.mockImplementation(async () => [{ ...BOB, modes: { 'ops-chat': 'full' } }, ALICE])
  await mount(createElement(Contacts))
  expect(body()).toContain('research@studio · cautious')
  expect(body()).toContain('Ops chat · Paused (chat archived) · full')
  expect(body()).toContain("Bob's MacBook · read only")
  await reset()
  // Every share full: the cap bounds nothing, so the row does not print it.
  api.contact_list.mockImplementation(async () => [{ ...BOB, modes: { research: 'full', 'ops-chat': 'full' } }])
  await mount(createElement(Contacts))
  expect(body()).toContain("Bob's MacBook")
  expect(body()).not.toContain('read only')
  expect(sharedLabel(sharingOf('s-ops', EXPORTS, [{ ...BOB, modes: { 'ops-chat': 'full' } }, ALICE]))).toBe('Shared with bob · full, alice · can change things · paused (chat archived)')
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...BOB, modes: { 'ops-chat': 'full' } }, ALICE])
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  expect(mode('ops-chat')).toBe('full')
  expect(mode('ops-chat', 'alice')).toBe('cautious')
  await run(() => selects['Mode of ops-chat for bob']!('cautious'))
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, cap: null, exports: null, blocked: null, modes: { 'ops-chat': 'cautious' } })
})

test('186 Communication: a full export says so; a cautious one keeps "one capped turn per message"', async () => {
  api.a2a_status.mockImplementation(async () => ({ ...A2A, exports: [{ ...A2A.exports[0]!, mode: 'full' }, A2A.exports[1]!] }))
  await mount(createElement(A2aSection, { state: undefined }))
  expect(body()).toContain('Full collaboration')
  expect(body()).toContain('reached by the contact over theuth/a2a/1, full collaboration')
  expect(body()).not.toContain('one capped turn per message')
})
