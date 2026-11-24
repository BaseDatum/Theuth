import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Files from '@/lib/files'
import type * as Rpc from '@/lib/rpc'
import type { ContextEntry, HistoryEntry } from '@/rpc/bindings/method.agent.output'
import { active } from '@/store/engine-scope'
import { unstampedKey } from '@/store/reduce'
import { type Entry, freshness, useSessions } from '@/store/sessions'
import { MessageUser } from './MessageUser'
import { FRESH } from './slash'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/lib/mobile', () => ({ usePointerCoarse: () => false }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/files', async (original) => ({ ...(await original<typeof Files>()), resolveFile: (_api: unknown, path: string) => `asset://${path}` }))

/** Static markup; server rendering reads zustand's initial state, so the sessions store answers its live state. */
const render = (node: ReactNode) => {
  const store = active().sessions
  Object.assign(store, { getInitialState: store.getState })
  return renderToStaticMarkup(h(QueryClientProvider, { client: new QueryClient() }, h(TooltipProvider, null, node)))
}

const SHOT = '/data/attachments/ab12.png'
/** The stored opener (seq 3) with its pasted screenshot, as `load_session` lists it. */
const stored = {
  role: 'user',
  text: 'what is this?[image: ab12.png]',
  turn: 3,
  context: {
    target: { kind: 'message', seq: 3 },
    active: true,
    removed: false,
    remove_with: [],
    attachments: [{ mime: 'image/png', name: 'ab12.png', path: SHOT, removed: false, target: { kind: 'binary', seq: 3, part: 1 } }],
  } as unknown as ContextEntry,
} as unknown as HistoryEntry

const last = (s: string) => useSessions.getState().sessions[s]?.entries.at(-1) as Extract<Entry, { kind: 'user' }>
const bubble = (s: string) => {
  const u = last(s)
  return render(h(MessageUser, { id: u.id, text: u.text, files: u.files, turn: u.turn, context: u.context, session: s }))
}

test('another client’s send: its `TurnStarted` echo (no attachments) gets the stored row’s image from the mid-turn stamp read', () => {
  const S = 'echo-other-window'
  const st = useSessions.getState()
  st.loadHistory(S, FRESH, 'replace')
  st.applyEvents([{ type: 'TurnStarted', session: S, turn: 'T1', text: 'what is this?', model: 'm', turn_seq: 3 }])
  expect(last(S)).toMatchObject({ kind: 'user', text: 'what is this?', turn: 3, echo: true })
  expect(bubble(S)).not.toContain('<img ')
  // The route reads on `TurnStarted` only when the echoed opener lacks its stored row.
  const s = useSessions.getState().sessions[S]!
  expect(unstampedKey(s)).toBe(`3:${last(S).id}`)
  // That read: the opener is stored before `TurnStarted` is published.
  st.stampTail(S, { ...FRESH, running: true, history: [stored] }, freshness(s))
  expect(last(S).context?.attachments.map((a) => a.path)).toEqual([SHOT])
  expect(bubble(S)).toContain(`src="asset://${SHOT}"`)
  expect(unstampedKey(useSessions.getState().sessions[S]!)).toBe(null)
})

test('the sending client shows its own files and reads nothing for its opener', () => {
  const S = 'echo-own-window'
  const st = useSessions.getState()
  st.loadHistory(S, FRESH, 'replace')
  st.markSent(S, 'what is this?', [SHOT])
  st.applyEvents([{ type: 'TurnStarted', session: S, turn: 'T1', text: 'what is this?', model: 'm', turn_seq: 3 }])
  expect(last(S)).not.toHaveProperty('echo')
  expect(unstampedKey(useSessions.getState().sessions[S]!)).toBe(null)
  expect(bubble(S)).toContain(`src="asset://${SHOT}"`)
})
