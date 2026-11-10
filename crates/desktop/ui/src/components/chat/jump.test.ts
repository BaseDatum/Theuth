import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { active } from '@/store/engine-scope'
import { empty } from '@/store/reduce'
import type { Entry } from '@/store/sessions'
import { SendCard } from './SendCard'
import { takeJump, useTranscriptJump } from './jump'
import { buildRows } from './rows'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
// A record fetch that never settles: the card shows what the tool output and the store hold.
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({ peer_message: () => Promise.withResolvers().promise }) }))

const A = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af5'
const KID = '01a1c28b-8e10-7757-afd9-db253e4f5e73'
const answer: Entry = { id: 'p9', kind: 'peer', from: `session:${KID}`, text: 'the answer', origin: { id: 'm-p9', from: `session:${KID}`, reply_to: A, scope: null, contact_mode: null } }
const tool: Extract<Entry, { kind: 'tool' }> = { id: 'e1', kind: 'tool', callId: 'c1', name: 'send_message', args: { to: `session:${KID}`, message: 'hi' }, pending: false, child: false, result: { output: `session:${KID} accepted id=${A}`, isError: false, files: [] } }

afterEach(() => {
  active().sessions.setState((s) => ({ sessions: { ...s.sessions, s: empty('s') } }))
  useTranscriptJump.getState().clear()
  document.body.innerHTML = ''
})

async function mount(readOnly: boolean) {
  active().sessions.setState((s) => ({ sessions: { ...s.sessions, s: { ...empty('s'), entries: [answer] } } }))
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  await act(async () => root.render(h(QueryClientProvider, { client: new QueryClient() }, h(SendCard, { tool, session: 's', readOnly }))))
  return { root, button: [...el.querySelectorAll('button')].find((b) => b.textContent?.includes('jump to message')) }
}

test('a click asks the transcript; the request survives the send card unmounting', async () => {
  const { root, button } = await mount(false)
  expect(button).toBeDefined()
  await act(async () => button?.click())
  // The card's row leaves the virtual window: it unmounts before the transcript takes the request.
  await act(async () => root.unmount())
  expect(takeJump('s', buildRows([tool, answer], [], 's', undefined, true))?.entry.id).toBe('p9')
})

test("a read-only card (search's Earlier messages) shows the answer but offers no jump, so nothing goes stale", async () => {
  const { root, button } = await mount(true)
  expect(document.body.textContent).toContain('the answer')
  expect(button).toBeUndefined()
  expect(useTranscriptJump.getState().entryId).toBeNull()
  await act(async () => root.unmount())
})
