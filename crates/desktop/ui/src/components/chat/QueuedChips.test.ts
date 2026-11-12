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
import { QueuedChips, withdrawInto } from './QueuedChips'
import { FRESH } from './slash'
import { StopButton } from './StopButton'
import { interrupt, phase } from '@/lib/composer-state'
import type { UiEvent as WireEvent } from '@/store/ui-events'
import { label as chordLabel } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'

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

test('171F4 N7: a steer queued with an image shows its thumbnail at once and on its bubble once delivered', () => {
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
  // A reload meanwhile: the engine's listing carries the files too.
  st.loadHistory(S, { ...FRESH, running: true, queued: [{ id: 'c1', text: 'and this one', files }] }, 'replace')
  expect(chips()).toContain('src="asset:///tmp/n7/shot.png"')

  st.applyEvents([{ type: 'InputInjected', session: S, id: 'c1', turn: 'T2' }])
  expect(chips()).toBe('')
  const you = useSessions.getState().sessions[S]?.entries.at(-1)
  expect(you).toMatchObject({ kind: 'user', text: 'and this one', injected: true, files })
  const bubble = render(h(MessageUser, { text: 'and this one', files: you?.kind === 'user' ? you.files : undefined, session: S }))
  expect(bubble).toContain('src="asset:///tmp/n7/shot.png"')
  expect(bubble).toContain('notes.pdf')
})

test('171F4: after an app reload the queue from `list_queued` shows each steer’s attachments, and delivery keeps them', () => {
  const S = 'n7-reloaded'
  const st = useSessions.getState()
  st.loadHistory(S, { ...FRESH, running: true, queued: [{ id: 'q1', text: 'look', files: ['/data/attachments/ab12.png'] }, { id: 'q2', text: 'plain' }] }, 'replace')
  const html = render(h(QueuedChips, { session: S, onEdit: () => undefined }))
  expect(html).toContain('src="asset:///data/attachments/ab12.png"')
  expect(html.match(/<img /g)).toHaveLength(1)
  st.applyEvents([{ type: 'InputInjected', session: S, id: 'q1', turn: 'T3' }])
  expect(useSessions.getState().sessions[S]?.entries.at(-1)).toMatchObject({ kind: 'user', text: 'look', files: ['/data/attachments/ab12.png'] })
})

test('171F4: editing a queued steer brings back its text and its attachments; a refused withdraw edits nothing', async () => {
  const edits: [string, string[]][] = []
  const onEdit = (text: string, files: string[]) => void edits.push([text, files])
  const api = { withdraw: async (_s: string, id: string) => (id === 'gone' ? Promise.reject(new Error('already delivered')) : 'and this one') }
  await withdrawInto(api as never, 's', { id: 'c1', files: ['/tmp/n7/shot.png'] }, onEdit)
  await withdrawInto(api as never, 's', { id: 'c2' }, onEdit)
  await withdrawInto(api as never, 's', { id: 'gone', files: ['/tmp/x.png'] }, onEdit)
  expect(edits).toEqual([
    ['and this one', ['/tmp/n7/shot.png']],
    ['and this one', []],
  ])
})

test('190: during a contact\'s turn (full, or an old cautious `<contact_message>` opener) the composer reads as in any chat: the chip says how to interrupt, Stop is plain, interject is not held back', () => {
  const origin = { id: 'm1', from: 'peer:sky/ops', reply_to: null, scope: null, contact_mode: 'full' as const }
  const openers: WireEvent[][] = [
    [
      { type: 'PeerDelivered', session: 'contact-full', origin, text: 'hi', seq: 5, turn: 'T1' },
      { type: 'TurnStarted', session: 'contact-full', turn: 'T1', text: '<message from="peer:sky/ops" id="m1">\nhi\n</message>', model: 'm', turn_seq: 5 },
    ],
    [
      {
        type: 'TurnStarted',
        session: 'contact-cautious',
        turn: 'T1',
        text: '<contact_message contact="sky" machine="m" export="ops" cap="read_only">\nThis message comes from sky, a contact outside your mesh.\nhi\n</contact_message>',
        model: 'm',
        turn_seq: 5,
      },
    ],
  ]
  for (const events of openers) {
    const S = (events[0] as Extract<WireEvent, { type: 'TurnStarted' | 'PeerDelivered' }>).session
    const st = useSessions.getState()
    st.loadHistory(S, { ...FRESH, running: true, queued: [{ id: 'q1', text: 'mine' }] }, 'replace')
    st.applyEvents(events)
    const s = useSessions.getState().sessions[S]!
    expect(s.running).toBe(true)
    const chips = render(h(QueuedChips, { session: S, onEdit: () => undefined }))
    // The wording of any chat: the interject chord, never "runs after <contact>'s turn".
    expect(chips).toContain(`queued · ${chordLabel(useShortcuts.getState().chords.interject)} to interrupt`)
    expect(chips).not.toContain('runs after')
    expect(render(h(StopButton, { count: 0, ring: 0, onStop: () => undefined }))).toContain('aria-label="Stop"')
    expect(interrupt(phase(s.running, s.queued?.length ?? 0), true)).toBe('interject')
    expect(interrupt(phase(s.running, s.queued?.length ?? 0), false)).toBe('flush')
  }
})
