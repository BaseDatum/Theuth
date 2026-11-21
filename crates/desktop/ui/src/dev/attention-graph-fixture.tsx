// 208 visual check, dev only: `bunx vite --port 1438` then open `/src/dev/attention-graph-fixture.html` (`?theme=dark`,
// `?speed=<simulated seconds per real second>` default 60 — one real second is one simulated minute; `?still=1` stops the script).
// The real Attention page with the graph region on the 40-node fixture of `components/attention/graph/fixture.ts`, no
// engine and no pairing: discovery is `legacy`, `attention_list` serves the 207 fixture with five items moved onto graph
// sessions (r1.builder, r3, r3.sweep, r7 ×2), and the graph source is a simulated engine — an accelerated clock drives
// the TTLs, and a script every 2 s spawns children, flips states, sends messages (sent → delivered → replied), returns
// child results, starts a new root every ~30 s and flips the remote root. Every change goes out as an
// `AgentGraphChanged` with seq + 1 through the page's real `applyDelta`; once a minute one seq is skipped on purpose,
// which exercises the re-snapshot path.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { Link, Outlet, RouterProvider, createMemoryHistory, createRootRoute, createRoute, createRouter } from '@tanstack/react-router'
import { StrictMode, useRef } from 'react'
import { createRoot } from 'react-dom/client'
import { graphFixture } from '@/components/attention/graph/fixture'
import { setGraphClock, setGraphSource } from '@/components/attention/graph/queries'
import { entryHead } from '@/components/attention/model'
import { Page, useAttentionView } from '@/components/attention/Page'
import { Screen } from '@/components/attention/Screen'
import { useAttention } from '@/components/hud/queries'
import { attentionFixture } from '@/components/settings/attention-fixtures'
import { SidebarProvider } from '@/components/ui/sidebar'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { AgentGraphChanged, GraphPulse, GraphPulseKind } from '@/rpc/bindings/event.agent_2egraph.output'
import type { Api } from '@/rpc/bindings/client'
import type { AgentGraph, AgentState, GraphEdge, GraphNode, GraphOutcome } from '@/rpc/bindings/method.agent.output'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')

// --- the simulated clock: one real second = `speed` simulated seconds × 60 (default: one simulated minute) ---
const speed = Number(query.get('speed') ?? 60) || 60
const realStart = Date.now()
const simNow = () => realStart + (Date.now() - realStart) * speed
setGraphClock(simNow)

// --- the simulated engine ---
const seed = graphFixture(simNow())
const nodes = new Map<string, GraphNode>(seed.nodes.map((n) => [n.id, n]))
const edges = new Map<string, GraphEdge>(seed.edges.map((e) => [e.id, e]))
const epoch = seed.epoch_ms
let seq = seed.seq
const listeners = new Set<(ev: AgentGraphChanged) => void>()

function snapshot(): AgentGraph {
  return { at_ms: simNow(), edges: [...edges.values()], epoch_ms: epoch, nodes: [...nodes.values()], older: seed.older, seq, unchanged: false, window_ms: seed.window_ms }
}
function emit(part: Partial<AgentGraphChanged>, gap = false) {
  seq += gap ? 2 : 1
  const ev: AgentGraphChanged = { at_ms: simNow(), edges: [], edges_removed: [], epoch_ms: epoch, nodes: [], older: seed.older, pulses: [], removed: [], resync: false, seq, type: 'AgentGraphChanged', ...part }
  setTimeout(() => {
    for (const l of listeners) l(ev)
  }, 0)
}
setGraphSource({
  snapshot: async (_w, _include, cursor) => {
    await new Promise((r) => setTimeout(r, 120))
    if (cursor && cursor.epoch_ms === epoch && cursor.seq === seq) return { ...snapshot(), unchanged: true, nodes: [], edges: [] }
    return snapshot()
  },
  subscribe: (cb) => {
    listeners.add(cb)
    return () => listeners.delete(cb)
  },
})

const put = (n: GraphNode): GraphNode => {
  nodes.set(n.id, n)
  return n
}
const touch = (id: string, patch: Partial<GraphNode>): GraphNode | null => {
  const n = nodes.get(id)
  return n ? put({ ...n, ...patch, last_event_ms: simNow() }) : null
}
const withState = (n: GraphNode, state: AgentState, outcome: GraphOutcome | null = null): Partial<GraphNode> => ({
  activity: { ...n.activity, state, live: state.state === 'running' || state.state === 'awaiting' || state.state === 'approval' },
  outcome,
})
const liveNodes = () => [...nodes.values()].filter((n) => n.kind !== 'peer' && !n.remote && (n.activity.live || n.waits.length > 0))
const pick = <T,>(xs: readonly T[]): T | undefined => xs[Math.floor(Math.random() * xs.length)]
const pairId = (a: string, b: string): [string, string, string] => (a < b ? [`${a}|${b}`, a, b] : [`${b}|${a}`, b, a])
let msgN = 0
let spawnN = 0

function message(from: string, to: string, result = false) {
  const [id, a, b] = pairId(from, to)
  const old = edges.get(id)
  const x = `sim-${++msgN}`
  const e: GraphEdge = {
    a,
    b,
    id,
    ab: (old?.ab ?? 0) + (from === a ? 1 : 0),
    ba: (old?.ba ?? 0) + (from === b ? 1 : 0),
    results: (old?.results ?? 0) + (result ? 1 : 0),
    open: (old?.open ?? 0) + (result ? 0 : 1),
    last_from: from,
    last_id: x,
    last_ms: simNow(),
  }
  edges.set(id, e)
  const pulse = (kind: GraphPulseKind): GraphPulse => ({ at_ms: simNow(), edge: id, from, id: x, kind, to })
  emit({ edges: [e], pulses: [pulse(result ? 'result' : 'sent')] })
  if (result) return
  setTimeout(() => {
    const cur = edges.get(id)
    if (!cur) return
    const done = { ...cur, open: Math.max(0, cur.open - 1), last_ms: simNow() }
    edges.set(id, done)
    const replied = Math.random() < 0.4
    emit({ edges: [done], pulses: [pulse(replied ? 'replied' : 'delivered')] })
  }, 1000)
}

const STEPS: (() => void)[] = [
  // spawn a child under a live node
  () => {
    const parent = pick(liveNodes())
    if (!parent) return
    const id = `${parent.id}.s${++spawnN}`
    const name = pick(['grep', 'check', 'fmt', 'probe', 'plan', 'diff', 'lint', 'scan']) ?? 'task'
    const n = put({ ...parent, id, kind: 'child', parent: parent.id, title: name, name, agent: null, observed: null, waits: [], outcome: null, started_ms: simNow(), last_event_ms: simNow(), activity: { ...parent.activity, state: { state: 'running' }, live: true, jobs: 0, ptys: 0 } })
    emit({ nodes: [n] })
  },
  // state round trips: awaiting / approval and back, or a finish
  () => {
    const n = pick(liveNodes().filter((x) => x.activity.state.state === 'running' && x.waits.length === 0 && x.kind !== 'root'))
    if (!n) return
    const roll = Math.random()
    const parentName = n.parent ? (nodes.get(n.parent)?.name ?? nodes.get(n.parent)?.title ?? 'parent') : 'parent'
    if (roll < 0.35) {
      emit({ nodes: [touch(n.id, withState(n, { state: 'awaiting', from: parentName })) as GraphNode] })
      setTimeout(() => emit({ nodes: [touch(n.id, withState(n, { state: 'running' })) as GraphNode] }), 4000)
    } else if (roll < 0.6) {
      emit({ nodes: [touch(n.id, withState(n, { state: 'approval', tool: 'bash' })) as GraphNode] })
      setTimeout(() => emit({ nodes: [touch(n.id, withState(n, { state: 'running' })) as GraphNode] }), 5000)
    } else {
      const outcome: GraphOutcome = Math.random() < 0.8 ? 'finished' : 'failed'
      emit({ nodes: [touch(n.id, withState(n, outcome === 'failed' ? { state: 'error' } : { state: 'idle' }, outcome)) as GraphNode] })
    }
  },
  // a message between two shown sessions
  () => {
    const xs = liveNodes()
    const from = pick(xs)
    const to = pick([...nodes.values()].filter((x) => x.id !== from?.id && x.kind !== 'peer' && (x.root === from?.root || Math.random() < 0.2) && simNow() - x.last_event_ms < 8 * 60_000))
    if (from && to) message(from.id, to.id)
  },
  // a child result reaches its parent
  () => {
    const n = pick(liveNodes().filter((x) => x.kind === 'child' && x.parent !== null && x.activity.state.state === 'running' && x.waits.length === 0))
    if (!n || !n.parent) return
    emit({ nodes: [touch(n.id, withState(n, { state: 'idle' }, 'finished')) as GraphNode] })
    message(n.id, n.parent, true)
  },
]
let tick = 0
let roots = 0
function step() {
  tick++
  if (tick % 15 === 0) {
    // a new root every ~30 s
    const id = `n${++roots}`
    const n = put({ ...(nodes.get('r1') as GraphNode), id, root: id, title: pick(['Draft the launch post', 'Triage inbox', 'Bench the parser', 'Rotate staging certs']) ?? 'New chat', project: 'theuth', started_ms: simNow(), last_event_ms: simNow() })
    emit({ nodes: [n] })
    return
  }
  if (tick % 10 === 0) {
    // the remote root's last-known state flips
    const r6 = nodes.get('r6') as GraphNode
    const running = r6.activity.state.state === 'running'
    emit({ nodes: [touch('r6', withState(r6, running ? { state: 'idle' } : { state: 'running' }, running ? 'finished' : null)) as GraphNode] })
    return
  }
  if (tick % 30 === 0) {
    // one deliberate gap a minute: the client must re-snapshot
    emit({}, true)
    return
  }
  pick(STEPS)?.()
}
if (query.get('still') !== '1') setInterval(step, 2000)

// --- the attention list: the 207 fixture, five items moved onto graph sessions ---
const GRAPH_SESSIONS: [number, string, string[]][] = [
  [0, 'r1.builder', ['Ship 2.5 release', 'builder']],
  [2, 'r3', ['ops desk']],
  [10, 'r3.sweep', ['ops desk', 'nightly-sweep']],
  [6, 'r7', ['concierge desk']],
  [7, 'r7', ['concierge desk']],
]
function items(): AttentionItem[] {
  const xs = attentionFixture(Date.now())
  for (const [i, session, path] of GRAPH_SESSIONS) {
    const it = xs[i]
    if (!it) continue
    const root = session.split('.')[0] as string
    xs[i] = { ...it, session, root_session: root, parent_session: session.includes('.') ? session.split('.').slice(0, -1).join('.') : null, path }
  }
  return xs
}
const FIXED = items()

const qc = active().qc
const api = new Proxy({} as Api, {
  get: (_t, name: string) => {
    if (name === 'then') return undefined
    return async () => {
      if (name === 'attention_list') return FIXED
      if (name === 'list_peer_messages') return { awaiting_answer: [], awaiting_reply: [], cursor: null, messages: [] }
      throw new Error(`fixture: ${name} not answered`)
    }
  },
})
active().connection.setState({ api, client: {} as RpcClient, info: null, state: 'open', discovery: { ...active().connection.getState().discovery, phase: 'legacy' } })

function Index() {
  const first = useAttentionView()?.entries[0]
  return first ? <Screen item={entryHead(first)} /> : null
}
function Selected() {
  const { id } = detail.useParams()
  const held = useRef<AttentionItem | null>(null)
  const live = useAttention().data?.find((i) => i.id === id)
  if (live) held.current = live
  const item = live ?? (held.current?.id === id ? held.current : undefined)
  return item ? <Screen item={item} /> : null
}
function Session() {
  const { session } = chat.useParams()
  return (
    <div className="m-auto flex flex-col items-center gap-2 p-6 text-sm text-fg-secondary">
      <p>Session {session} (the chat view is not part of this fixture).</p>
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
      <Toaster />
    </TooltipProvider>
  ),
})
const attention = createRoute({ getParentRoute: () => root, path: '/attention', component: Page })
const index = createRoute({ getParentRoute: () => attention, path: '/', component: Index })
const detail = createRoute({ getParentRoute: () => attention, path: '$id', component: Selected })
const chat = createRoute({ getParentRoute: () => root, path: '/c/$session', component: Session })
const router = createRouter({
  routeTree: root.addChildren([attention.addChildren([index, detail]), chat]),
  history: createMemoryHistory({ initialEntries: ['/attention'] }),
})

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
