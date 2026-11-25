import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { A2AExportStatus } from '@/rpc/bindings/method.a2a.output'
import type { CommunicationState } from '@/rpc/bindings/method.agent.output'
import { CommunicationNotice, sharedExports } from './CommunicationNotice'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))

const exp = (o: Partial<A2AExportStatus> = {}): A2AExportStatus =>
  ({ name: 'logo-design-with-theuth-awaiting', session: 's1', url: 'http://x/agents/logo/rpc', peer: 'lab', ...o }) as A2AExportStatus
const ON = { effective: true, global: true, opted_out: false, inherited_from: null, source: 'default' } as unknown as CommunicationState

test('stopping a share leaves the export with no peer: reachable by nobody, so not shared and no banner', () => {
  // Shared with the contact bob: the header chip says it, not this banner.
  expect(sharedExports([exp({ peer: 'bob' })], 's1', [{ name: 'bob' }])).toEqual([])
  // After Stop sharing: the export stays (191), its grant is gone.
  expect(sharedExports([exp({ peer: null })], 's1', [{ name: 'bob' }])).toEqual([])
  expect(sharedExports([exp({ peer: null })], 's1', undefined)).toEqual([])
  // An HTTP peer's grant on this chat is the one shared state the banner shows; another chat's is not.
  expect(sharedExports([exp(), exp({ session: 's2' })], 's1', [{ name: 'bob' }])).toEqual([exp()])
})

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null
afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  root = null
})

function mount(exports: A2AExportStatus[], state: CommunicationState = ON) {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(CommunicationNotice, { session: 's1', state, exports })))
  return host
}

test('no shared export: no banner (the revoked case reaches it as an empty list)', () => {
  expect(mount([]).querySelector('[data-export-notice]')).toBeNull()
  expect(host!.textContent).toBe('')
})

test('shared: one plain sentence naming the peer, never the export slug, and the Messaging… link', () => {
  const el = mount([exp()]).querySelector<HTMLElement>('[data-export-notice]')!
  expect(el.textContent).toBe(
    "Shared with lab: their messages can use this chat's tools; approvals still ask you. What you type here stays private.Messaging…",
  )
  expect(el.textContent).not.toMatch(/logo-design|no peer granted|Exported|can be shared/)
  mount([exp(), exp({ name: 'other', peer: 'ops' }), exp({ name: 'third', peer: 'lab' })])
  expect(host!.textContent).toContain('Shared with lab and ops:')
})

test('compact: one line on a phone, two at most wider; a tap on the text shows the rest and folds it again', () => {
  const el = mount([exp()]).querySelector<HTMLElement>('[data-export-notice]')!
  expect(el.className).toContain('py-1')
  const text = el.querySelector<HTMLButtonElement>('button[aria-expanded]')!
  expect(text.className).toContain('line-clamp-2')
  expect(text.className).toContain('max-sm:line-clamp-1')
  expect(text.getAttribute('aria-expanded')).toBe('false')
  act(() => text.click())
  expect(text.getAttribute('aria-expanded')).toBe('true')
  expect(text.className).not.toContain('line-clamp')
  act(() => text.click())
  expect(text.className).toContain('max-sm:line-clamp-1')
})
