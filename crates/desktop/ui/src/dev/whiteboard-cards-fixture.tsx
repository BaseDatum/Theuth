// 218 cards proof, dev only: `bunx vite` then open `/src/dev/whiteboard-cards-fixture.html` (`?theme=dark`).
// The REAL whiteboard canvas on a local Y.Doc against a stub engine: an agent-style sticky, a json-render Graph wrapped
// in a Card (what agents place), a mermaid chart, all sized like the engine's placement defaults; `?big=1` puts them
// at owner-resized sizes far past the defaults. `?chat=1` shows the chat cards of `wb_create` and `show_whiteboard`;
// `?tray=1` the bar above the composer of a chat that made both (its whiteboard dashes and, opened, the tray tile).
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import * as Y from 'yjs'
import { BackgroundArea } from '@/components/chat/BackgroundArea'
import { MessageTool } from '@/components/chat/MessageTool'
import { TooltipProvider } from '@/components/ui/tooltip'
import { WhiteboardShell } from '@/components/whiteboard/WhiteboardView'
import { requirements } from '@/components/whiteboard/requirements'
import { addCard } from '@/components/whiteboard/canvas/ops'
import type { Api } from '@/rpc/bindings/client'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { type Entry, useSessions } from '@/store/sessions'

const q = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', q.get('theme') === 'dark')

const api = new Proxy({} as Api, { get: (_, name: string) => async () => (name.startsWith('list_') ? [] : null) })
const client = { call: async () => null } as unknown as RpcClient
const methods = [...requirements.methods, 'doc_unsubscribe', 'subscribe_topics', 'unsubscribe_topics']
const discovery = {
  methods: methods.map((name) => ({ name, params: [], result: {} })),
  'x-theuth': {
    subscriptions: [
      { subscribe: 'subscribe_topics', unsubscribe: 'unsubscribe_topics', notification: 'subscribe_topics' },
      { subscribe: 'doc_subscribe', unsubscribe: 'doc_unsubscribe', notification: 'doc_subscribe' },
    ],
    events: [{ topic: 'document.changed', version: 1 }],
  },
} as never
active().connection.setState({ api, client, info: {} as never, discovery: { phase: 'ready', capabilities: null, document: discovery, unavailable: new Set() } })

const GRAPH = {
  root: 'card',
  elements: {
    card: { type: 'Card', props: { title: 'Checkout flow' }, children: ['g'] },
    g: {
      type: 'Graph',
      props: {
        direction: 'LR',
        nodes: [
          { id: 'cart', label: 'Cart', kind: 'rounded' },
          { id: 'ship', label: 'Shipping' },
          { id: 'pay', label: 'Payment', state: 'active' },
          { id: 'done', label: 'Confirmation', kind: 'pill', state: 'done' },
        ],
        edges: [
          { from: 'cart', to: 'ship' },
          { from: 'ship', to: 'pay', label: 'address ok' },
          { from: 'pay', to: 'done', label: 'charged' },
        ],
      },
    },
  },
}
const MERMAID = 'flowchart TD\n  A[Visit] --> B{Signed in?}\n  B -- yes --> C[Cart]\n  B -- no --> D[Sign in]\n  D --> C'

const big = q.get('big') === '1'
const doc = new Y.Doc()
addCard(doc, { kind: 'sticky', title: 'Idea', x: 40, y: 40, w: big ? 520 : 220, h: big ? 420 : 150, source: { t: 'sticky', text: 'Apple Pay first on iOS; move the promo field below the totals so the cart step stops dropping 18 % on mobile.' } })
addCard(doc, { kind: 'diagram', title: 'Checkout flow', x: big ? 600 : 300, y: 40, w: big ? 980 : 360, h: big ? 620 : 260, source: { t: 'diagram', lang: 'json-render', code: JSON.stringify(GRAPH) } })
addCard(doc, { kind: 'diagram', title: 'Sign-in gate', x: 40, y: big ? 700 : 340, w: 360, h: 300, source: { t: 'diagram', lang: 'mermaid', code: MERMAID } })
const provider = { doc, meta: { title: 'Checkout ideas', kind: 'board' }, onStatus: () => () => {} }

const WB = '01jabcdefghjkmnpqrstvwxyz0'
type Tool = Extract<Entry, { kind: 'tool' }>
const call = (name: string, callId: string, args: unknown, output: string): Tool => ({ id: callId, kind: 'tool', callId, name, args, pending: false, child: callId.includes('.'), result: { output, isError: false, files: [] } })
const CALLS = [
  call('wb_create', 'c1.1', { title: 'Checkout ideas' }, `created whiteboard "Checkout ideas" ${WB} — show it to the user with show_whiteboard("${WB}")\ntheuth://whiteboards/${WB}\n0 cards`),
  call('show_whiteboard', 'c2', { whiteboard: WB }, `opened whiteboard "Checkout ideas"\ntheuth://whiteboards/${WB}\n3 cards`),
]
const WB2 = '01jabcdefghjkmnpqrstvwxyz2'
if (q.get('tray') === '1') {
  useSessions.getState().ensure('s1')
  const entries = [...CALLS, call('show_whiteboard', 'c3', { whiteboard: WB2 }, `opened whiteboard "Pricing page"\ntheuth://whiteboards/${WB2}\n5 cards`)]
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, entries } } }))
}

function Fixture() {
  if (q.get('tray') === '1')
    return (
      <TooltipProvider>
        <div className="mx-auto flex h-dvh max-w-[720px] flex-col justify-end p-6">
          <BackgroundArea session="s1" />
          <div className="h-24 rounded-2xl border bg-layer-1" aria-label="composer" />
        </div>
      </TooltipProvider>
    )
  if (q.get('chat') === '1')
    return (
      <TooltipProvider>
        <div className="mx-auto max-w-[720px] p-6">
          {CALLS.map((t) => (
            <MessageTool key={t.callId} tool={t} session="s1" />
          ))}
        </div>
      </TooltipProvider>
    )
  return (
    <TooltipProvider>
      <div className="h-dvh">
        <WhiteboardShell doc="wb-fixture" provider={provider as never} frame="relative size-full overflow-hidden bg-background" embedded />
      </div>
    </TooltipProvider>
  )
}

const router = createRouter({ routeTree: createRootRoute({ component: Fixture }), history: createMemoryHistory() })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
