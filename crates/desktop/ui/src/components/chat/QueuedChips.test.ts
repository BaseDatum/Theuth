import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Files from '@/lib/files'
import type * as Rpc from '@/lib/rpc'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import { MessageUser } from './MessageUser'
import { QueuedChips } from './QueuedChips'
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

test('171F4 N7: a steer queued with an image shows its thumbnail at once, through a reload, and on its bubble once delivered', () => {
  const S = 'n7-session'
  const files = ['/tmp/n7/shot.png', '/tmp/n7/notes.pdf']
  const st = useSessions.getState()
  st.loadHistory(S, { ...FRESH, running: true }, 'replace')
  // The composer's own chip (with its files), then the engine's echo of the same id (none): the files stay.
  st.applyEvents([{ type: 'InputQueued', session: S, id: 'c1', text: 'and this one', defer: false, files }])
  st.applyEvents([{ type: 'InputQueued', session: S, id: 'c1', text: 'and this one', defer: false }])
  const chips = () => render(h(QueuedChips, { session: S, onEdit: () => undefined }))
  expect(chips()).toContain('src="asset:///tmp/n7/shot.png"')
  expect(chips()).toContain('notes.pdf')
  // A reload meanwhile: `list_queued` has the text only.
  st.loadHistory(S, { ...FRESH, running: true, queued: [{ id: 'c1', text: 'and this one' }] }, 'replace')
  expect(chips()).toContain('src="asset:///tmp/n7/shot.png"')

  st.applyEvents([{ type: 'InputInjected', session: S, id: 'c1', turn: 'T2' }])
  expect(chips()).toBe('')
  const you = useSessions.getState().sessions[S]?.entries.at(-1)
  expect(you).toMatchObject({ kind: 'user', text: 'and this one', injected: true, files })
  const bubble = render(h(MessageUser, { text: 'and this one', files: you?.kind === 'user' ? you.files : undefined, session: S }))
  expect(bubble).toContain('src="asset:///tmp/n7/shot.png"')
  expect(bubble).toContain('notes.pdf')
})
