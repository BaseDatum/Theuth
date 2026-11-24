import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { SharingList, SharingRow } from './sharing-wire'
import { SharedNotesTab, SharingOverviewTab } from './SharingHub'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))

const row = (r: Partial<SharingRow> & Pick<SharingRow, 'kind' | 'id'>): SharingRow => ({
  title: 'Plan',
  audience: '',
  permission: null,
  state: 'active',
  created_ms: null,
  expires_ms: null,
  last_access_ms: null,
  count: null,
  url: null,
  engine: null,
  parent: null,
  revocable: true,
  open: null,
  ...r,
})
const NOW = Date.now()
const LIST: SharingList = {
  rows: [
    row({ kind: 'document_member', id: 'd1|pa', parent: 'd1', audience: 'Ann', permission: 'editor', last_access_ms: NOW - 120_000 }),
    row({ kind: 'document_link', id: 'd1|l1', parent: 'd1', audience: 'anyone with the link', permission: 'editor', created_ms: NOW - 86_400_000, count: 1 }),
    row({ kind: 'document_link', id: 'd1|l0', parent: 'd1', audience: 'anyone with the link', permission: 'viewer', state: 'revoked', count: 1 }),
    row({ kind: 'document_member', id: 'd2|pb', parent: 'd2', title: 'Reading list', audience: 'Ben', permission: 'viewer' }),
    row({ kind: 'public_share', id: 's1', title: 'app.sky.tun.zip/', audience: 'public', state: 'live', created_ms: NOW - 10_000 }),
    row({ kind: 'public_share', id: 's0', title: 'old.sky.tun.zip/', audience: 'public', state: 'stopped' }),
  ],
  partial: [{ kind: 'contact', reason: 'a2a plugin is not running' }, { kind: 'export', reason: 'a2a plugin is not running' }],
}
const api = {
  sharing_list: vi.fn(async (_kinds: unknown): Promise<SharingList> => LIST),
  sharing_revoke: vi.fn(async (kind: string, id: string, _sub: string | null) => ({ kind, id, pending: false })),
  notebook_list: vi.fn(async (_archived: boolean) => [{ doc: { id: 'd2' } }]),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

let root: Root | undefined
beforeEach(() => vi.clearAllMocks())
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
})
async function mount(el: Parameters<typeof h>[0]) {
  const div = document.createElement('div')
  document.body.append(div)
  root = createRoot(div)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(el as never))))
}
const text = () => document.body.textContent ?? ''
const buttonsIn = (el: Element, label: string) => [...el.querySelectorAll('button')].filter((b) => b.textContent === label)

test('213 Overview: live counts per kind (documents, not rows), the last changes, a contacts-off line, and Network', async () => {
  await mount(SharingOverviewTab)
  await vi.waitFor(() => expect(text()).toContain('2 shared documents'))
  expect(text()).toContain('1 live')
  expect(text().match(/Contacts are off — turn on agent messaging in Network/g)).toHaveLength(1)
  expect(text()).toContain('app.sky.tun.zip/Public share · public · live · added just now')
  await act(async () => buttonsIn(document.body, 'Open Network')[0]!.click())
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'machine' } })
})

test('213 Notes: per document its members and links; revoke a link alone or with the members who joined through it', async () => {
  await mount(SharedNotesTab)
  await vi.waitFor(() => expect(document.querySelector('[data-doc="d1"]')).not.toBeNull())
  expect(api.sharing_list).toHaveBeenCalledWith(['document_member', 'document_link'])
  const d1 = document.querySelector('[data-doc="d1"]')!
  expect(d1.textContent).toContain('Ann · editor · seen 2 min ago')
  expect(d1.textContent).toContain('1 joined')
  // A revoked link offers no Revoke, but still sweeps the member who came through it; the live one offers both.
  expect(buttonsIn(d1, 'Revoke link')).toHaveLength(1)
  await act(async () => buttonsIn(d1, 'Revoke and remove its members')[0]!.click())
  expect(api.sharing_revoke).toHaveBeenCalledWith('document_link', 'd1|l1', 'members')
  await act(async () => buttonsIn(d1, 'Remove its members')[0]!.click())
  expect(api.sharing_revoke).toHaveBeenCalledWith('document_link', 'd1|l0', 'members')
  await act(async () => buttonsIn(d1, 'Remove')[0]!.click())
  expect(api.sharing_revoke).toHaveBeenCalledWith('document_member', 'd1|pa', null)
  await act(async () => buttonsIn(d1, 'Open document')[0]!.click())
  expect(navigate).toHaveBeenCalledWith({ to: '/notes/$id', params: { id: 'd1' } })
  // A notebook opens on its own page.
  await vi.waitFor(() => expect(api.notebook_list).toHaveBeenCalled())
  const d2 = document.querySelector('[data-doc="d2"]')!
  await vi.waitFor(async () => {
    await act(async () => buttonsIn(d2, 'Open document')[0]!.click())
    expect(navigate).toHaveBeenCalledWith({ to: '/notebooks/$id', params: { id: 'd2' } })
  })
})
