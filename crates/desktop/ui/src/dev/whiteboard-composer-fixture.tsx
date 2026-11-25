// 218 composer live proof, dev only: `bunx vite` then open `/src/dev/whiteboard-composer-fixture.html` (`?theme=dark`).
// The REAL whiteboard shell (canvas, cluster, the session view's `Composer` docked bottom right with the tools button
// and the session picker) on a local Y.Doc with three stickies, against a stub engine: a catalog, two bound sessions,
// and every other call answered empty.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import * as Y from 'yjs'
import { TooltipProvider } from '@/components/ui/tooltip'
import { WhiteboardShell } from '@/components/whiteboard/WhiteboardView'
import { addCard } from '@/components/whiteboard/canvas/ops'
import { modelRequirements } from '@/components/chat/requirements'
import type { Api } from '@/rpc/bindings/client'
import type { ModelInfo } from '@/rpc/bindings/method.agent.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

document.documentElement.classList.toggle('dark', new URLSearchParams(location.search).get('theme') === 'dark')

const model = (id: string, name: string, efforts: string[]): ModelInfo => ({ id, name, provider: id.split('/')[0]!, context: 200_000, reasoning: efforts.length > 0, image: true, efforts, budget_min: null, budget_max: null, local: false })
const CATALOG = [model('anthropic/claude-opus-4-8', 'Claude Opus 4.8', ['low', 'medium', 'high']), model('openai/gpt-5.4', 'GPT-5.4', ['low', 'high'])]
const info = { default_model: CATALOG[0]!.id, default_effort: 'high', favorite_models: [CATALOG[0]!.id] }
const answers: Record<string, unknown> = {
  info,
  list_models: CATALOG,
  vision_model: null,
  voice_status: { model: { state: 'ready' }, accelerator: '', voice: 'af', voices: [], speaking: null, queued: [], input_devices: [] },
}
const api = new Proxy({} as Api, { get: (_, name: string) => async () => answers[name] ?? (name.startsWith('list_') ? [] : null) })
const BOUND = [
  { session: '00000000-0000-4000-8000-00000000b001', title: 'Checkout redesign' },
  { session: '00000000-0000-4000-8000-00000000b002', title: 'Pricing page copy review' },
]
const client = { call: async (m: string) => (m === 'whiteboard_sessions' ? BOUND : null) } as unknown as RpcClient
const methods = [...modelRequirements.methods, 'vision_model', 'voice_status', 'list_mcp', 'set_mcp_hidden', 'whiteboard_sessions', 'subscribe_topics', 'unsubscribe_topics']
const discovery = {
  methods: methods.map((name) => ({ name, params: [], result: {} })),
  'x-theuth': { subscriptions: [{ subscribe: 'subscribe_topics', unsubscribe: 'unsubscribe_topics', notification: 'subscribe_topics' }], events: [{ topic: 'agent.events', version: 1 }] },
} as never
active().connection.setState({ api, client, info: info as never, discovery: { phase: 'ready', capabilities: null, document: discovery, unavailable: new Set() } })

const doc = new Y.Doc()
for (const [i, text] of ['Cart step drops 18 % on mobile', 'Move promo field below totals', 'Apple Pay first on iOS'].entries())
  addCard(doc, { kind: 'sticky', title: `Sticky ${i + 1}`, x: 80 + i * 260, y: 120 + (i % 2) * 90, w: 220, h: 160, source: { t: 'sticky', text } })
const provider = { doc, meta: { title: 'Checkout ideas', kind: 'board' }, onStatus: () => () => {} }

function Fixture() {
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
