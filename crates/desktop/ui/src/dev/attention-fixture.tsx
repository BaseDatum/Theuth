// 207 visual check, dev only: `bunx vite` then open `/src/dev/attention-fixture.html` (`?theme=dark`, `?id=<item id>`
// to open one item). The real Attention page — toolbar, flat list, bulk bar, detail pane — on the 34-item fixture of
// `settings/attention-fixtures.ts`, no engine and no pairing: discovery is `legacy` (every method counts as present, so
// the "Leave to agent" wording shows) and `?mock=crowded` is forced, so the list, Dismiss, Snooze, the bulk bar and the
// reply resolve locally. The per-kind buttons (Allow, an ask's option, Hire, Done, …) are answered by the fake api below
// and take their item out of the list; every other call fails.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { Link, Outlet, RouterProvider, createMemoryHistory, createRootRoute, createRoute, createRouter } from '@tanstack/react-router'
import { StrictMode, useRef } from 'react'
import { createRoot } from 'react-dom/client'
import { entryHead } from '@/components/attention/model'
import { Page, useAttentionView } from '@/components/attention/Page'
import { Screen } from '@/components/attention/Screen'
import { ATTENTION_KEY, dropFromList, useAttention } from '@/components/hud/queries'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/bindings/client'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
// The list and the §8.3 hooks read `?mock=crowded` from the page's own URL.
if (query.get('mock') !== 'crowded') {
  query.set('mock', 'crowded')
  history.replaceState(null, '', `${location.pathname}?${query}`)
}

const qc = active().qc
type Match = (i: AttentionItem, args: unknown[]) => boolean
/** The mutations the cards call, each with the items it resolves; the answer the engine would give. */
const ANSWERS: Record<string, [Match, unknown]> = {
  resolve_approval: [(i, [a]) => i.approval === a, true],
  answer_ask: [(i, [a]) => i.ask?.id === a, true],
  task_decide: [(i, [a]) => i.decision === a, null],
  handoff_resolve: [(i, [a]) => i.id === `handoff:${a}`, null],
  agent_approve: [(i, [a]) => i.kind === 'hire' && i.agent === a, null],
  agent_remove: [(i, [a]) => i.kind === 'hire' && i.agent === a, null],
  agent_pause: [(i, [a]) => i.kind === 'budget' && i.agent === a, null],
  task_move: [(i, [a]) => i.task === a, null],
  resume_turn: [(i, [a]) => i.kind === 'resume' && i.session === a, true],
  contact_first_answer: [(i, [c, e]) => i.id === `contact_first:${c}:${e}`, null],
  attention_dismiss: [(i, [a]) => i.id === a, null],
  attention_snooze: [(i, [a]) => i.id === a, null],
  double_take_over: [(i, [a]) => i.id === `double:${a}`, true],
}
const api = new Proxy({} as Api, {
  get: (_t, name: string) => {
    if (name === 'then') return undefined
    return async (...args: unknown[]) => {
      const answer = ANSWERS[name]
      if (!answer) throw new Error(`fixture: ${name} not answered`)
      const [match, result] = answer
      const gone = (qc.getQueryData<AttentionItem[]>(ATTENTION_KEY) ?? []).filter((i) => match(i, args)).map((i) => i.id)
      dropFromList(qc, gone)
      return result
    }
  },
})
active().connection.setState({ api, client: {} as RpcClient, info: null, state: 'open', discovery: { ...active().connection.getState().discovery, phase: 'legacy' } })

/** `/attention` with nothing picked: the first card of the list as filtered and sorted (`routes/attention.index.tsx`). */
function Index() {
  const first = useAttentionView()?.entries[0]
  return first ? <Screen item={entryHead(first)} /> : null
}

/** `/attention/$id` (`routes/attention.$id.tsx`): an item that left the list stays on screen until the user moves. */
function Selected() {
  const { id } = detail.useParams()
  const held = useRef<AttentionItem | null>(null)
  const live = useAttention().data?.find((i) => i.id === id)
  if (live) held.current = live
  const item = live ?? (held.current?.id === id ? held.current : undefined)
  if (item) return <Screen item={item} />
  return (
    <div className="m-auto flex flex-col items-center gap-2 p-6 text-center text-sm text-fg-tertiary">
      <p>This item is no longer in the inbox.</p>
      <Link to="/attention" className="underline underline-offset-2">
        Back to attention
      </Link>
    </div>
  )
}

const root = createRootRoute({
  component: () => (
    <TooltipProvider>
      <SidebarProvider>
        <div className="flex h-dvh min-w-0 flex-1 flex-col bg-background">
          <Outlet />
        </div>
      </SidebarProvider>
    </TooltipProvider>
  ),
})
const attention = createRoute({ getParentRoute: () => root, path: '/attention', component: Page })
const index = createRoute({ getParentRoute: () => attention, path: '/', component: Index })
const detail = createRoute({ getParentRoute: () => attention, path: '$id', component: Selected })
const start = query.get('id')
const router = createRouter({
  routeTree: root.addChildren([attention.addChildren([index, detail])]),
  history: createMemoryHistory({ initialEntries: [start ? `/attention/${encodeURIComponent(start)}` : '/attention'] }),
})

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
