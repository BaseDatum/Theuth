import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { receiveComponent } from '@/rpc/bindings/receive'
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
import { KindActions } from '@/components/attention/actions'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import type { Contact } from "@/rpc/bindings/method.a2a.output"
import type { FirstWaiting } from "@/rpc/bindings/method.a2a.output"
import type { Export } from "@/rpc/bindings/method.a2a.output"
import type { NearbyAgent } from "@/rpc/bindings/method.a2a.output"
import type { NearbyEngine } from "@/rpc/bindings/method.nodes.output"
import type { PairCeremony } from "@/rpc/bindings/method.a2a.output"
import { dispatch } from '@/store/sessions'
import { useConnection } from '@/store/connection'
import { Dialog } from '@/components/ui/dialog'
import { AddContact, CeremonyBody, type CeremonyView, ContactAccess, Contacts, ContactsTab, OLD_ENGINE, ShareDialog, SharingItems } from './Contacts'
import { toast } from 'sonner'
import { A2aSection } from './Communication'
import { CommunicationNotice, useCommunication } from '@/components/chat/CommunicationNotice'
import type { A2AStatus as A2aStatus } from "@/rpc/bindings/method.a2a.output"
import type { CommunicationState } from "@/rpc/bindings/method.agent.output"

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
// PlainScreens -> attention/shared -> attention/Page -> routes/c.$session -> chat/Transcript -> lib/nudges reads localStorage at import.
vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate, Link: ({ children }: { children: ReactNode }) => children }))
const local = vi.hoisted(() => ({ on: true }))
vi.mock('@/link/engines', async (original) => ({ ...(await original<typeof Engines>()), hasLocal: () => local.on }))
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
const BOB: Contact = receiveComponent("method.a2a.output.Contact", {
  name: 'bob',
  machine: "Bob's MacBook",
  endpoint: 'k3j9q2m4x7p1r8t5w6y0zabcdefgh',
  fingerprint: 'K3J9 Q2M4 X7P1 R8T5 W6Y0',
  path: 'relay',
  exports: ['research', 'ops-chat'],
  first_contact: { research: 'allow' },
  full_capable: true,
  shared: [],
  blocked: false,
  paired_ms: NOW,
  problem_kind: null,
  paired_on: null,
  machines: [],
})
const ALICE: Contact = receiveComponent("method.a2a.output.Contact", { ...BOB, name: 'alice', machine: 'studio', path: 'lan', exports: ['ops-chat'], first_contact: {}, fingerprint: 'AAAA BBBB CCCC DDDD EEEE', full_capable: false })
const EXPORTS: Export[] = [
  receiveComponent("method.a2a.output.Export", { name: 'research', target: { kind: 'agent', agent: 'research@studio' }, description: 'research@studio', paused: null }),
  receiveComponent("method.a2a.output.Export", { name: 'ops-chat', target: { kind: 'chat', session: 's-ops' }, description: 'Ops chat', paused: 'chat archived' }),
]
const NEARBY: NearbyAgent[] = [
  { name: 'Shopbot', service: '_agent._tcp', addr: '192.168.8.40:8080', eid: null, card: 'http://192.168.8.40:8080/.well-known/agent-card.json', verified: false },
  { name: 'Bob', service: '_a2a._tcp', addr: '192.168.8.21:4433', eid: 'k3j9q2m4x7p1r8t5', card: '/.well-known/agent-card.json', verified: true },
]
const MESH_NEARBY: NearbyEngine = { id: 'studio-machine', name: 'Studio', os_name: 'macOS', addr: '192.168.8.22', seen_ms: NOW }
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
    { name: 'ops-chat', session: 's-ops', peer: 'bob', url: 'http://127.0.0.1:7780/agents/ops-chat/rpc' },
    { name: 'docs', session: 's-docs', peer: 'lab', url: 'http://127.0.0.1:7780/agents/docs/rpc' },
  ],
}
// The engine answers the contact as the patch leaves it (a grant change shows in `exports`).
const echoUpdate = async (_name: string, p: { grant?: string[] | null; revoke?: string[] | null }): Promise<Contact> => ({ ...BOB, exports: [...BOB.exports.filter((x) => !p.revoke?.includes(x)), ...(p.grant ?? [])] })
const api = {
  contact_list: vi.fn(async (): Promise<Contact[]> => [BOB, ALICE]),
  a2a_exports: vi.fn(async (): Promise<Export[]> => EXPORTS),
  contact_window: vi.fn(async (open: boolean) => (open ? { ticket: 'theuth-contact:1.MFRGGZDFMZTWQ2LK', expires_ms: NOW + 600_000 } : null)),
  contact_nearby: vi.fn(async () => NEARBY),
  mesh_nearby: vi.fn(async (): Promise<NearbyEngine[]> => []),
  mesh_nearby_watch: vi.fn(async (_on: boolean) => null),
  contact_pair: vi.fn(async (_t: unknown) => CER),
  contact_answer: vi.fn(async (_id: string, _accept: boolean) => null),
  contact_update: vi.fn(echoUpdate),
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
  mesh_status: vi.fn(async () => ({ self_id: 'me', self_name: 'studio', peers: [{ id: 'xeus', name: 'xeus' }], nearby: true })),
  attention_dismiss: vi.fn(async () => null),
  attention_snooze: vi.fn(async () => null),
  sandboxes_list: vi.fn(async () => []),
  a2a_status: vi.fn(async (): Promise<A2aStatus> => A2A),
  reload_a2a: vi.fn(async (): Promise<A2aStatus> => A2A),
  get_communication: vi.fn(async () => ({ effective: true, global: true, opted_out: false, inherited_from: null, source: 'config' }) as unknown as CommunicationState),
  config_flag_get: vi.fn(async () => ({ key: 'a2a.enabled', enabled: false, running: false, restart_required: false })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

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
  local.on = true
  useConnection.setState(useConnection.getInitialState())
  api.contact_nearby.mockResolvedValue(NEARBY)
  api.mesh_nearby.mockResolvedValue([])
  api.contact_list.mockImplementation(async () => [BOB, ALICE])
  api.contact_update.mockImplementation(echoUpdate)
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
  expect(sharedLabel(s)).toBe('Shared with bob, alice · paused (chat archived)')
  expect(sharedLabel(sharingOf('s-plan', EXPORTS, [BOB]))).toBeNull()
  expect(sharedLabel(sharingOf('s-ops', EXPORTS, []))).toBeNull()
})

test('188 ?focus=access:<contact>: the contact it names, colons and all; any other focus names none', () => {
  for (const name of ['bob', 'Bob: lab', 'access:x']) expect(focusedContact(accessFocus(name))).toBe(name)
  for (const focus of [null, '', 'access:', 'model', 'spend', 'contact:bob']) expect(focusedContact(focus)).toBeNull()
})

test('§12.7 Contacts: each row has name, machine, fingerprint, path and the exports it may reach (paused named), no mode or cap', async () => {
  await mount(createElement(Contacts))
  const t = body()
  expect(t).toContain('A contact reaches only what you share, unless you give them full access. Their agent and yours talk freely')
  expect(t).toContain("Bob's MacBook")
  expect(t).toContain('K3J9 Q2M4 X7P1 R8T5 W6Y0')
  expect(t).toContain('Relay')
  expect(t).toContain('LAN')
  expect(t).toContain('research@studio')
  expect(t).toContain('Ops chat · Paused (chat archived)')
  // 190: a share chip is a plain share: no mode suffix, no cap beside the machine.
  for (const gone of ['read only', 'can change things', 'cautious', 'Cautious', 'Full collaboration']) expect(t).not.toContain(gone)
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
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: 'robert', blocked: null, full_access: null })
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
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('alice', { name: null, blocked: false, full_access: null }))
  await click('Choose what bob may reach…')
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'contacts' }, search: { focus: 'access:bob' } })
})

// 191 D8: one of each row — sound (a machine of theirs left), alert, refused, and waiting (a rename in flight: a second `bob`).
const ROWS: Contact[] = [
  receiveComponent("method.a2a.output.Contact", {
    ...BOB,
    id: 'r-bob',
    paired_on: 'studio',
    machines: [
      { eid: 'e-laptop', name: 'laptop', left: false },
      { eid: 'e-studio', name: 'studio', left: false },
      { eid: 'e-old', name: 'old-imac', left: true },
    ],
  }),
  receiveComponent("method.a2a.output.Contact", { ...ALICE, name: 'carol', id: 'r-carol', problem: 'a machine of carol follows a roster we never signed; one of ours may be compromised', problem_kind: 'alert', machines: [{ eid: 'e-tp', name: 'thinkpad', left: false }] }),
  receiveComponent("method.a2a.output.Contact", { ...ALICE, name: 'dave', id: 'r-dave', exports: [], problem: "cannot verify this contact's machines", problem_kind: 'refused', machines: [{ eid: 'd4v3q2m4xyz', name: 'd4v3q2m4', left: false }] }),
  receiveComponent("method.a2a.output.Contact", { ...BOB, id: 'r-bob-2', exports: [], problem: 'its name is taken by another contact (renaming)', problem_kind: 'waiting', machines: [{ eid: 'b0b2q2m4xyz', name: 'b0b2q2m4', left: false }] }),
]
const rowOf = (text: string) => [...document.querySelectorAll<HTMLElement>('div.border-b')].find((d) => d.textContent?.includes(text))!
const buttonsIn = (row: HTMLElement) => [...row.querySelectorAll('button')].map((b) => b.getAttribute('aria-label') ?? b.textContent?.trim())

test('191 D8 Contacts: their machines (one that left struck through, muted), Paired on, and each problem with its own action', async () => {
  api.contact_list.mockImplementation(async () => ROWS)
  await mount(createElement(Contacts))
  const bob = rowOf('Paired on studio')
  expect(bob.textContent).toContain('Their machines: laptop · studio · old-imac left')
  const left = bob.querySelector('s')!
  expect(left.textContent).toBe('old-imac')
  expect(left.parentElement!.className).toContain('text-muted-foreground')
  expect(bob.querySelector('[role=status]')).toBeNull()
  expect(buttonsIn(bob)).toEqual(['bob: Rename · Block · Remove', 'Rename', 'Choose what bob may reach…', 'Block…', 'Remove…'])
  // Alert: the contact still works (its menu stays), Remove and the way to unpair one of ours.
  const carol = rowOf('roster we never signed')
  expect(carol.querySelector('[role=status]')!.className).toContain('text-destructive')
  expect(buttonsIn(carol)).toEqual(['Network › Machines', 'Remove…', 'carol: Rename · Block · Remove', 'Rename', 'Choose what carol may reach…', 'Block…', 'Remove…'])
  // Refused: Remove only; nothing shared is offered.
  const dave = rowOf("cannot verify this contact's machines")
  expect(dave.querySelector('[role=status]')!.className).toContain('text-destructive')
  expect(buttonsIn(dave)).toEqual(['Remove…'])
  expect(dave.textContent).not.toContain('Nothing shared yet')
  expect(dave.textContent).toContain('Their machines: d4v3q2m4')
  // Waiting: muted, no action at all (Remove would delete a sound record).
  const waiting = rowOf('its name is taken')
  expect(waiting.querySelector('[role=status]')!.className).toContain('text-muted-foreground')
  expect(waiting.textContent).toContain('this clears by itself')
  expect(buttonsIn(waiting)).toEqual([])
  // Every contact once: the two `bob` records are two rows.
  expect(document.querySelectorAll('div.border-b')).toHaveLength(4)
})

test('191 D8 Contacts: Remove on a refused row names its record, on all your machines, with the rogue-machine line; the alert links to Machines; Block is on all your machines', async () => {
  api.contact_list.mockImplementation(async () => ROWS)
  await mount(createElement(Contacts))
  await run(() => rowOf("cannot verify this contact's machines").querySelector('button')!.click())
  const dialog = () => document.querySelector('[role=alertdialog]')?.textContent ?? ''
  expect(dialog()).toContain('Remove dave?')
  expect(dialog()).toContain('dave is forgotten on all your machines and its open tasks close.')
  expect(dialog()).toContain('Ask dave to remove you too, then pair again. Until they do, the removed machine may still reach them.')
  await click('Remove')
  await vi.waitFor(() => expect(api.contact_remove).toHaveBeenCalledWith('r-dave', false))
  await click('Network › Machines')
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'machines' } })
  await run(() => [...rowOf('Paired on studio').querySelectorAll('button')].find((b) => b.textContent === 'Block…')!.click())
  expect(dialog()).toContain('bob is refused on all your machines from now on')
  expect(dialog()).not.toContain('Ask bob to remove you too')
})

test('191 Access and Share… list a held contact with its state and its mesh-wide shares, never "reaches nothing yet"; a change names its record', async () => {
  api.contact_list.mockImplementation(async () => [ROWS[0]!, ROWS[1]!, { ...ROWS[2]!, exports: ['ops-chat'] }, ROWS[3]!])
  await mount(createElement(ContactAccess))
  const dave = rowOf('Refused on this machine')
  expect(dave.textContent).toContain("Refused on this machine: cannot verify this contact's machines. What you share applies on your machines that admit dave.")
  expect(dave.textContent).toContain('Ops chat')
  expect(dave.textContent).not.toContain('reaches nothing yet')
  const waiting = rowOf('Waiting on this machine')
  expect(waiting.textContent).toContain('Waiting on this machine: its name is taken by another contact (renaming). What you share applies once this clears.')
  expect(waiting.textContent).not.toContain('reaches nothing yet')
  await run(() => [...waiting.querySelectorAll('button')].find((b) => b.textContent?.includes('Share with bob'))!.click())
  await run(() => [...rowOf('Waiting on this machine').querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === 'Share Launch plan with bob')!.click())
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('r-bob-2', { name: null, blocked: null, full_access: null, grant: ['launch-plan'] }))
  await reset()
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  const labels = [...document.querySelectorAll('button[aria-label]')].map((b) => b.getAttribute('aria-label') ?? '')
  expect(labels.filter((l) => / with /.test(l))).toEqual(['Stop sharing with bob', 'Stop sharing with carol', 'Stop sharing with dave', 'Share this chat with bob'])
  expect(body()).toContain('Refused on this machine: cannot verify')
  expect(body()).toContain('Waiting on this machine: its name is taken')
  await click('Stop sharing with dave')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('r-dave', { name: null, blocked: null, full_access: null, revoke: ['ops-chat'] }))
})

test('191 Contacts and Access: a share of a chat on another machine reads "on <machine>"', async () => {
  const plan = receiveComponent('method.a2a.output.Export', { name: 'launch-plan', target: { kind: 'chat', session: 's-plan' }, description: 'Launch plan', paused: null })
  api.a2a_exports.mockImplementation(async () => [...EXPORTS, plan])
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['launch-plan', 'ops-chat'] }])
  await mount(createElement(ContactsTab, { focus: null } as never))
  // The contacts list and Access both say where the chat lives.
  await vi.waitFor(() => expect(body().split('Launch plan · on xeus')).toHaveLength(3))
  // A chat here or an agent names no machine.
  expect(body()).not.toContain('Ops chat · on')
  expect(body()).not.toContain('bob reaches nothing yet')
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

test('Add contact: mesh Nearby lease renews only while connected, rearms on reconnect and closes on unmount', async () => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
  try {
    const { unmount } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
    expect(api.mesh_nearby_watch.mock.calls).toEqual([[true]])
    await run(() => vi.advanceTimersByTime(10_000))
    expect(api.mesh_nearby_watch.mock.calls).toEqual([[true], [true]])
    await run(() => useConnection.setState({ state: 'reconnecting' }))
    await run(() => vi.advanceTimersByTime(30_000))
    expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(2)
    await run(() => useConnection.setState({ state: 'open' }))
    expect(api.mesh_nearby_watch.mock.calls).toEqual([[true], [true], [true]])
    await unmount()
    expect(api.mesh_nearby_watch).toHaveBeenLastCalledWith(false)
    await run(() => vi.advanceTimersByTime(30_000))
    expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(4)
  } finally {
    vi.useRealTimers()
  }
})

test('Add contact: mesh-only sightings are muted, non-actionable hints instead of an empty list', async () => {
  api.contact_nearby.mockResolvedValue([])
  api.mesh_nearby.mockResolvedValue([MESH_NEARBY])
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  const row = rowOf('Open Add contact on Studio to add it here.')
  expect(row.textContent).toContain('macOS')
  expect(row.classList.contains('text-fg-tertiary')).toBe(true)
  expect(row.querySelector('button, a, [tabindex], [role=button]')).toBeNull()
  expect(body()).not.toContain('No agent found')
  expect(body()).not.toContain('No A2A agent found')
})

test('Add contact: mesh hints follow agent rows and disappear when an A2A advert arrives', async () => {
  api.mesh_nearby.mockResolvedValue([MESH_NEARBY])
  const { qc } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  expect(body().indexOf('Shopbot')).toBeLessThan(body().indexOf('Open Add contact on Studio'))
  await run(() => qc.setQueryData(queryKeys.contactNearby, [...NEARBY, { ...NEARBY[1]!, name: 'Studio', eid: MESH_NEARBY.id }]))
  expect(body()).not.toContain('Open Add contact on Studio')
  expect(button('Add Studio')?.disabled).toBe(false)
})

test.each([
  ['a short A2A hint', 'agent-machine-123456789', 'agent-machine-12'],
  ['a short mesh id', 'agent-machine-12', 'agent-machine-123456789'],
])('Add contact: %s deduplicates in either prefix direction and existing contact endpoints stay hidden', async (_label, id, eid) => {
  api.contact_nearby.mockResolvedValue([{ ...NEARBY[1]!, eid }])
  api.mesh_nearby.mockResolvedValue([
    { ...MESH_NEARBY, id, name: 'Advertised' },
    { ...MESH_NEARBY, id: BOB.endpoint, name: 'Already a contact' },
    MESH_NEARBY,
  ])
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  expect(body()).not.toContain('Advertised')
  expect(body()).not.toContain('Already a contact')
  expect(body()).toContain('Open Add contact on Studio to add it here.')
})

test('Add contact: no agents or hints explains that the other theuth must open Add contact', async () => {
  api.contact_nearby.mockResolvedValue([])
  await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  expect(body()).toContain('No agent found on this network yet. Another theuth shows here while Add contact is open on it.')
  expect(body()).not.toContain('No A2A agent found')
})

test('Add contact: turning mesh Nearby off hides cached hints without hiding A2A agents', async () => {
  api.mesh_nearby.mockResolvedValue([MESH_NEARBY])
  const { qc } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }))
  expect(body()).toContain('Open Add contact on Studio to add it here.')
  await run(() => qc.setQueryData(['mesh'], { ...qc.getQueryData<object>(['mesh']), nearby: false }))
  expect(body()).not.toContain('Open Add contact on Studio')
  expect(button('Add Bob')?.disabled).toBe(false)
})

test.each(['mesh_nearby', 'mesh_nearby_watch'])('Add contact: engines without %s receive no mesh Nearby calls or cached hints', async (method) => {
  const discovery = useConnection.getState().discovery
  useConnection.setState({ discovery: { ...discovery, unavailable: new Set([method]) } })
  const qc = qcNew()
  qc.setQueryData(queryKeys.meshNearby, [MESH_NEARBY])
  const { unmount } = await mount(createElement(AddContact, { onClose: vi.fn(), onAccess: vi.fn() }), qc)
  expect(body()).not.toContain('Open Add contact on Studio')
  expect(button('Add Bob')?.disabled).toBe(false)
  await unmount()
  expect(api.mesh_nearby).not.toHaveBeenCalled()
  expect(api.mesh_nearby_watch).not.toHaveBeenCalled()
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
    'A paired contact reaches nothing until you share an agent or chat with them.',
  )
  await click('Accept')
  expect(api.contact_answer).toHaveBeenCalledWith('c1', true)
  expect(body()).toContain('Waiting for Bob to press Accept…')
  expect(button('Accept')?.disabled).toBe(true)
  await run(() => dispatch([{ key: 'a2a.contacts@1', event: { type: 'contact_pair', data: { ...CER, state: 'paired' } } }]))
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

test('§7.6 Contacts\' access: first contact → contact_first_answer; paused named; no cap, mode or tools editor', async () => {
  api.a2a_exports.mockImplementation(async () => [EXPORTS[0]!, { ...EXPORTS[1]!, tools: ['search_web'] } as Export])
  await mount(createElement(ContactAccess))
  expect(body()).toContain('Ops chat · Paused (chat archived)')
  expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('allow')
  expect(document.querySelector('[aria-label="First contact from bob to ops-chat"]')?.getAttribute('data-value')).toBe('ask')
  await run(() => selects['First contact from bob to ops-chat']!('block'))
  expect(api.contact_first_answer).toHaveBeenCalledWith('bob', 'ops-chat', 'block')
  // 190: every share is full: the only select per row is its first-contact answer.
  expect(Object.keys(selects).sort()).toEqual(['First contact from alice to ops-chat', 'First contact from bob to ops-chat', 'First contact from bob to research'])
  expect(document.querySelector('[role=group][aria-label^="Tools "]')).toBeNull()
  for (const gone of ['Cautious', 'cautious', 'Full collaboration', 'Read only', 'Can change things', 'Add tool', 'search_web']) expect(body()).not.toContain(gone)
})

test('190 export picker: agents and chats (not archived, with their machine); a pick grants at once, the patch naming no mode', async () => {
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['ops-chat'] }, ALICE])
  await mount(createElement(ContactAccess))
  await click('Share with bob…')
  const agents = document.querySelector('[role=group][aria-label="Agents"]')!.textContent
  expect(agents).toContain('theuth')
  expect(agents).toContain('research@studio')
  const chats = document.querySelector('[role=group][aria-label="Chats"]')!.textContent
  expect(chats).not.toContain('Ops chat')
  expect(chats).toContain('Launch plan')
  expect(chats).toContain('xeus')
  expect(chats).not.toContain('Old')
  // The consent line at the moment of sharing: what the agent may do there.
  expect(body()).toContain('yours works with its normal tools and your usual approvals')
  await click('Share Launch plan with bob')
  // A new export is put, then granted alone: the existing grant (ops-chat) is never sent (191); the patch names no mode.
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, grant: ['launch-plan'] }))
  expect(api.a2a_export_put).toHaveBeenCalledWith({ name: 'launch-plan', target: { kind: 'chat', session: 's-plan' }, description: 'Launch plan', paused: null })
  vi.clearAllMocks()
  await click('Share research@studio with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, grant: ['research'] }))
  expect(api.a2a_export_put).not.toHaveBeenCalled()
})

test('190: Share… is on for every paired contact, whatever its card says (`full_capable` false: card unread, offline)', async () => {
  await mount(createElement(ContactAccess))
  for (const who of ['bob', 'alice']) expect((button(`Share with ${who}…`) as HTMLButtonElement).disabled).toBe(false)
  expect(body()).not.toContain('to share')
})

test('§12.10 unset: taking an export back is one revoke; the export stays for the next share (never removed from this list)', async () => {
  await mount(createElement(ContactAccess))
  await click('Stop sharing research@studio with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, revoke: ['research'] }))
  await click('Stop sharing Ops chat with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, revoke: ['ops-chat'] }))
  expect(api.a2a_export_remove).not.toHaveBeenCalled()
})

test('191 version skew: an engine that ignores grant / revoke (its answer unchanged) fails the share and the unshare with "update it", never a silent success', async () => {
  api.contact_update.mockImplementation(async () => BOB)
  await mount(createElement(ContactAccess))
  await click('Share with bob…')
  await click('Share Launch plan with bob')
  await vi.waitFor(() => expect(toast.error).toHaveBeenCalledWith(OLD_ENGINE))
  vi.mocked(toast.error).mockClear()
  await click('Stop sharing research@studio with bob')
  await vi.waitFor(() => expect(toast.error).toHaveBeenCalledWith(OLD_ENGINE))
})

test('⋯ Share with contact…: one Share per contact grants this chat, Shared · Stop takes it back; Stop sharing with <contacts> revokes it from each', async () => {
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  expect(body()).toContain("They see only what this chat's agent sends back")
  expect(body()).toContain('You can stop sharing at any time.')
  expect(body()).toContain('A contact reaches only what you share, unless you give them full access. Their agent and yours talk freely, with Markdown, links and images, and yours works with its normal tools and your usual approvals.')
  for (const gone of ['Cautious', 'Full collaboration', 'read only', 'can change things']) expect(body()).not.toContain(gone)
  await click('Stop sharing with bob')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, revoke: ['ops-chat'] }))
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['research'] }, { ...ALICE, exports: [] }])
  await mount(createElement(ShareDialog, { session: 's-new', title: 'New chat', onClose: vi.fn() }))
  // 186: no share button takes focus on open, so Enter never shares by accident.
  expect(document.activeElement?.tagName).not.toBe('BUTTON')
  // alice's card says nothing about `full`: her Share is on all the same.
  expect((button('Share this chat with alice') as HTMLButtonElement).disabled).toBe(false)
  expect(body()).toContain('studio')
  expect(body()).not.toContain('to share')
  await click('Share this chat with bob')
  await vi.waitFor(() => expect(api.a2a_export_put).toHaveBeenCalledWith({ name: 'new-chat', target: { kind: 'chat', session: 's-new' }, description: 'New chat', paused: null }))
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, grant: ['new-chat'] })
  await reset()
  api.contact_list.mockImplementation(async () => [BOB, ALICE])
  const onShare = vi.fn()
  await mount(createElement(SharingItems, { session: 's-ops', onShare }))
  await click('Share with contact…')
  expect(onShare).toHaveBeenCalled()
  vi.clearAllMocks()
  await click('Stop sharing with bob and alice')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('alice', { name: null, blocked: null, full_access: null, revoke: ['ops-chat'] }))
  expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: null, revoke: ['ops-chat'] })
  expect(api.a2a_export_remove).not.toHaveBeenCalled()
})

test('Stop sharing is absent for a chat nobody can message', async () => {
  await mount(createElement(SharingItems, { session: 's-plan', onShare: vi.fn() }))
  expect(body()).toContain('Share with contact…')
  expect(body()).not.toContain('Stop sharing')
})

test('lifted 2026-11-17: on a phone or iPad (no local engine) Contacts lists every contact with Rename · Block · Remove, Access and its shares; Add contact stays on the machine', async () => {
  local.on = false
  await mount(createElement(ContactsTab, { focus: null } as never))
  await vi.waitFor(() => expect(body()).toContain("Bob's MacBook"))
  expect(body()).toContain('studio')
  expect(button('Add contact…')).toBeUndefined()
  expect(button('Share with bob…')).toBeDefined()
  expect(button('bob: Rename · Block · Remove')).toBeDefined()
  await click('Remove…')
  await click('Remove')
  await vi.waitFor(() => expect(api.contact_remove).toHaveBeenCalledWith('bob', false))
})

// The DOM here has no layout: what keeps the dialog usable at 390 px is checked as its parts (the live check renders it).
test('⋯ Share with contact… at 390 px: rows wrap, every action is a 44 px touch target, and the dialog scrolls inside the screen', async () => {
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
  expect(dialog.className).toContain('max-h-[calc(100dvh-2rem)]')
  expect(dialog.className).toContain('overflow-y-auto')
  const rows = [...dialog.querySelectorAll('button[aria-label^="Stop sharing with"], button[aria-label^="Share this chat with"]')].map((b) => b.parentElement!)
  expect(rows).toHaveLength(2)
  for (const r of rows) expect(r.className).toContain('flex-wrap')
  for (const b of dialog.querySelectorAll('button')) if (b.textContent !== 'Close') expect(b.className, b.textContent ?? '').toContain('pointer-coarse:min-h-11')
  expect(body()).toContain('You can stop sharing at any time.')
})

test('⋯ Share with contact…: a refused share says so in the dialog, and for a chat on another machine names where to share it', async () => {
  api.contact_update.mockRejectedValue(new Error('contacts are unavailable: the a2a plugin is not running'))
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: [] }])
  await mount(createElement(ShareDialog, { session: 's-plan', title: 'Launch plan', owner: 'xeus', onClose: vi.fn() }))
  await click('Share this chat with bob')
  await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toBe('This machine could not share it (contacts are unavailable: the a2a plugin is not running). Open this chat on xeus, the machine that runs it, to share it.'))
  await reset()
  await mount(createElement(ShareDialog, { session: 's-ops', title: 'Ops chat', onClose: vi.fn() }))
  await click('Share this chat with bob')
  await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toBe('Not shared: contacts are unavailable: the a2a plugin is not running'))
})

const FIRST: AttentionItem = receiveComponent("method.roster.output.AttentionItem", {
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
})

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

test('ContactFirstScreen: who asks, what it wants to reach and the three answers; no share mode', async () => {
  api.contact_first_waiting.mockImplementation(async () => [receiveComponent('method.a2a.output.FirstWaiting', { contact: 'bob', machine: "Bob's MacBook", export: 'research', session: 's1', since_ms: NOW, waiting: 1, preview: 'hi' })])
  await mount(createElement(ContactFirstScreen, { item: FIRST }))
  const t = body()
  expect(t).toContain("bob (Bob's MacBook) wants to reach research")
  expect(t).toContain('Wants to reach')
  expect(t).not.toContain('Full collaboration')
  expect(t).not.toContain('Cautious')
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
  await run(() => dispatch([{ key: 'a2a.contacts@1', event: { type: 'contact_nearby', data: NEARBY } }]))
  expect(qc.getQueryData(queryKeys.contactNearby)).toEqual(NEARBY)
  await run(() => dispatch([{ key: 'a2a.contacts@1', event: { type: 'contact_pair', data: CER } }, { key: 'a2a.contacts@1', event: { type: 'contact_pair', data: { ...CER, id: 'c2', state: 'paired' } } }]))
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
  expect(t).toContain('reached by the contact over theuth/a2a/1: its agent and this chat’s agent talk freely, like mesh peers')
  for (const gone of ['Cautious', 'cautious', 'Full collaboration', 'capped']) expect(t).not.toContain(gone)
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

const WAIT: FirstWaiting = receiveComponent("method.a2a.output.FirstWaiting", { contact: 'bob', machine: "Bob's MacBook", export: 'ops-chat', session: 's-ops', since_ms: NOW, waiting: 1, preview: 'hello' })

test('183 F3 Communication: a held first message sits under its export row; Allow once · Always · Block answer it; no machine, no parentheses; a queued row (186 engine) never shows', async () => {
  api.contact_first_waiting.mockImplementation(async () => [WAIT, { ...WAIT, contact: 'alice', machine: '' }, { ...WAIT, contact: 'carol', queued: true, position: 1 }])
  await mount(createElement(A2aSection, { state: undefined }))
  await vi.waitFor(() => expect(body()).toContain("bob (Bob's MacBook) is waiting for you to allow its first message to ops-chat"))
  expect(body()).toContain('alice is waiting for you to allow its first message to ops-chat')
  expect(body()).not.toContain('alice ()')
  expect(body()).not.toContain('carol')
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

test('183 F3 Contacts: each contact\'s row carries its own waiting line and answers; a queued message (186 engine) is no hold and never shows here', async () => {
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
  expect(body()).toContain('Shared with you: Ops, Bot (asks first), Old (paused)')
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
  await click('Share research@studio with bob')
  await vi.waitFor(() => expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('allow'))
  expect(body()).toContain('A new share starts on "Always allow"')
  await reset()
  api.contact_list.mockImplementation(async () => [{ ...BOB, exports: ['research'], first_contact: {} }])
  await mount(createElement(ContactAccess))
  expect(document.querySelector('[aria-label="First contact from bob to research"]')?.getAttribute('data-value')).toBe('ask')
})

test('200 Full access: on asks first, then sends full_access: true; off is immediate; each consequence and their-side line; held and blocked have no switch', async () => {
  api.contact_list.mockImplementation(async () => [
    { ...BOB, full_access: false, full_access_from: false },
    { ...ALICE, full_access: true, full_access_from: true },
    { ...ALICE, name: 'carol', full_access: false, full_access_from: null },
    { ...ALICE, name: 'eve', blocked: true, full_access: false, full_access_from: true },
    ROWS[2]!,
  ])
  // The engine answers the contact as the patch leaves it.
  api.contact_update.mockImplementation(async (name: string, p: { grant?: string[] | null; revoke?: string[] | null; full_access?: boolean | null }) => ({ ...BOB, name, full_access: p.full_access ?? false }))
  await mount(createElement(ContactAccess))
  const bob = rowOf('bob reaches only the chats you share')
  expect(bob.textContent).toContain('Full access is off. bob reaches only the chats you share.')
  expect(bob.textContent).toContain('bob has not given you full access.')
  const alice = rowOf('alice has given you full access')
  expect(alice.textContent).toContain("Full access: any of alice's sessions can message your sessions and see their list (titles, running or idle). Private chats and your other contacts' conversations stay hidden. Nothing is synced.")
  expect(alice.textContent).toContain('alice has given you full access: your sessions can list and message theirs.')
  expect(body()).toContain("carol's theuth does not support full access yet.")
  expect(button('Full access for eve')).toBeUndefined()
  expect(button('Full access for dave')).toBeUndefined()
  // A 44 px target on a phone: the label around the switch is the touch area.
  expect(button('Full access for bob')!.parentElement!.className).toContain('pointer-coarse:min-h-11')

  const dialog = () => document.querySelector('[role=alertdialog]')?.textContent ?? ''
  await click('Full access for bob')
  expect(dialog()).toContain('Give bob full access?')
  expect(dialog()).toContain("Full access: any of bob's sessions can message your sessions")
  expect(dialog()).toContain('It applies on all your machines as each receives it, normally within seconds. You can turn it off here at any time.')
  await click('Cancel')
  expect(api.contact_update).not.toHaveBeenCalled()
  await click('Full access for bob')
  await click('Give full access')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('bob', { name: null, blocked: null, full_access: true }))
  await vi.waitFor(() => expect(rowOf('bob reaches only').querySelector('[role=status]')?.textContent).toBe('bob has full access on this machine now, and on your other machines as each receives it (normally seconds).'))

  vi.clearAllMocks()
  await click('Full access for alice')
  expect(dialog()).toBe('')
  await vi.waitFor(() => expect(api.contact_update).toHaveBeenCalledWith('alice', { name: null, blocked: null, full_access: false }))
  await vi.waitFor(() =>
    expect(rowOf('alice has given you').querySelector('[role=status]')?.textContent).toBe(
      'Full access for alice is off: refused on this machine now, and on your other machines as each receives the change (normally seconds; a machine offline catches up when it reconnects).',
    ),
  )

  // An engine older than 200 ignores the field: never a silent success.
  api.contact_update.mockImplementation(echoUpdate)
  await click('Full access for carol')
  await click('Give full access')
  await vi.waitFor(() => expect(toast.error).toHaveBeenCalledWith(OLD_ENGINE))
})

