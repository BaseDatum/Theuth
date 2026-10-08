/**
 * 147 §10.2 — the six map methods behind one interface. `engineMapApi` forwards to the engine `Api` (flat names,
 * positional params); a method the connected engine lacks rejects with a one-line reason so the detail pane can disable
 * its action with that text. `mockMapApi` serves an in-memory map (theuth's 11-node partition from the spike) and replays
 * `CodeMapProgress` / `CodeMapChanged` through `emit` when `codemap_generate` is called, so the Map tab's states can be
 * walked without an engine (`dev/codemap-fixture.ts` wires `emit` to `dispatch`).
 */
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import type { Api } from '@/rpc/api'
import { localSearch } from './map'
import type { CodeMapChanged, CodeMapProgress, MapChild, MapEdge, MapElement, MapEvidence, MapExport, MapHit, MapIndex, MapJob, MapMeta, MapOrigin, MapPerspective, MapTreeRow } from './map-types'

export interface MapApi {
  codemap_overview(root: string): Promise<MapIndex>
  codemap_element(root: string, element: string): Promise<MapElement>
  codemap_generate(root: string, session: string, element: string | null): Promise<MapJob>
  codemap_cancel(root: string): Promise<MapJob>
  codemap_search(root: string, query: string, limit: number | null): Promise<MapHit[]>
  codemap_export(root: string): Promise<MapExport>
}
export const MAP_METHODS = ['codemap_overview', 'codemap_element', 'codemap_generate', 'codemap_cancel', 'codemap_search', 'codemap_export'] as const satisfies readonly (keyof MapApi)[]

/** Which of the six the engine offers (an older engine lacks all; a partial contract some). */
export function mapMethodsOf(api: unknown): Set<keyof MapApi> {
  const out = new Set<keyof MapApi>()
  if (api && typeof api === 'object') for (const m of MAP_METHODS) if (typeof (api as Record<string, unknown>)[m] === 'function') out.add(m)
  return out
}

/** The engine adapter: each method forwards when present, else rejects with the reason the UI shows in a tooltip. */
export function engineMapApi(api: Api | null): MapApi {
  const has = mapMethodsOf(api)
  const call = <K extends keyof MapApi>(m: K): MapApi[K] =>
    ((...args: unknown[]) =>
      api === null
        ? Promise.reject(new Error('not connected'))
        : has.has(m)
          ? (api as unknown as Record<K, (...a: unknown[]) => Promise<unknown>>)[m](...args)
          : Promise.reject(new Error(`${m} is not available on this engine`))) as unknown as MapApi[K]
  return {
    codemap_overview: call('codemap_overview'),
    codemap_element: call('codemap_element'),
    codemap_generate: call('codemap_generate'),
    codemap_cancel: call('codemap_cancel'),
    codemap_search: call('codemap_search'),
    codemap_export: call('codemap_export'),
  }
}

// ── the mock map ───────────────────────────────────────────────────────────────────────────────────────────────────

type Spec = {
  node: string
  name: string
  path: string | null
  files: number
  lines: number
  description: string
  concern?: string
  context?: string
  children?: Spec[]
  stale?: boolean
  kind?: string
}
type EdgeSpec = [from: string, to: string, label: string, evidence: MapEvidence[]]

const manifest = (n: number, from: string, to: string): MapEvidence[] => Array.from({ length: n }, (_, i) => ({ Manifest: { from: `${from}/${i ? `x${i}` : 'Cargo.toml'}`, to, kind: 'cargo-path' } }))
const imports = (count: number, sample: string): MapEvidence[] => [{ Imports: { count, sample } }]
const refs = (count: number, sample: string): MapEvidence[] => [{ Refs: { count, sample, confidence: 0.9 } }]
const inferred = (reason: string): MapEvidence[] => [{ Inferred: { reason } }]

const CODEINDEX: Spec = {
  node: 'unit:crates/codeindex',
  name: 'Code index',
  path: 'crates/codeindex',
  files: 31,
  lines: 14_210,
  description: 'tree-sitter parsing into one SQLite per root; rank, lenses and the code_* tools read it. Every unit below is a module of the crate.',
  concern: 'git.rs changed since this diagram was written; locate/profile/root/outline_hook live in the concern, not drawn.',
  children: [
    { node: 'unit:crates/codeindex/src/walk.rs', name: 'Walk', path: 'crates/codeindex/src/walk.rs', files: 1, lines: 410, description: 'ignore-based walk, manifests, entry points.' },
    { node: 'cluster:7c1e9a02', name: 'Parse + resolve', path: 'crates/codeindex/src/parse', files: 9, lines: 3_100, description: 'tree-sitter grammars and the import resolvers.', kind: 'cluster' },
    { node: 'unit:crates/codeindex/src/store.rs', name: 'SQLite store', path: 'crates/codeindex/src/store.rs', files: 1, lines: 1_520, description: 'Schema v4, FTS, the lens queries.' },
    { node: 'unit:crates/codeindex/src/index.rs', name: 'Codeindex / RootIndex', path: 'crates/codeindex/src/index.rs', files: 1, lines: 980, description: 'Per-root pool, reconcile and touch.' },
    { node: 'unit:crates/codeindex/src/git.rs', name: 'Git facts', path: 'crates/codeindex/src/git.rs', files: 1, lines: 300, description: 'Co-change pairs from gix history.', stale: true },
    { node: 'unit:crates/codeindex/src/rank.rs', name: 'Rank', path: 'crates/codeindex/src/rank.rs', files: 1, lines: 800, description: 'Seeded PageRank over files and symbols.' },
    { node: 'unit:crates/codeindex/src/lens.rs', name: 'Lenses Here/Fit/Change', path: 'crates/codeindex/src/lens.rs', files: 1, lines: 900, description: 'The three activity lenses.' },
    { node: 'unit:crates/codeindex/src/tools.rs', name: 'code_* tools', path: 'crates/codeindex/src/tools.rs', files: 1, lines: 2_050, description: 'Six read-only tools and the prompt block.' },
    { node: 'unit:crates/codeindex/src/event.rs', name: 'Events', path: 'crates/codeindex/src/event.rs', files: 1, lines: 90, description: 'CodeToolRan and CodeReconciled.' },
  ],
}

const STORES: Spec = {
  node: 'cluster:9f3a1c2e',
  name: 'Content stores',
  path: 'crates/knowledge',
  files: 119,
  lines: 54_183,
  kind: 'cluster',
  description:
    'Durable content the agent reads and writes (119 files / 54,183 lines). document is the Yjs authority for notes, boards and artifacts; knowledge (OKF bundle + gix history) and artifact sit on it; search is the shared fuzzy/snippet envelope; codeindex is the tree-sitter + SQLite code index whose lenses feed this panel.',
  concern: 'codeindex has no owner module for the map yet; a schema bump deletes derived rows.',
  children: [
    { node: 'unit:crates/document', name: 'Document authority', path: 'crates/document', files: 22, lines: 9_800, description: 'Yjs documents, boards, anchors.' },
    { node: 'unit:crates/knowledge', name: 'Knowledge bundle', path: 'crates/knowledge', files: 18, lines: 7_400, description: 'OKF bundle and gix history.' },
    { node: 'unit:crates/artifact', name: 'Artifacts', path: 'crates/artifact', files: 9, lines: 2_100, description: 'Attachment store.' },
    { node: 'unit:crates/search', name: 'Search envelope', path: 'crates/search', files: 6, lines: 1_300, description: 'Fuzzy and snippet search shared by stores.' },
    CODEINDEX,
  ],
}

const TOP: Spec = {
  node: 'overall-architecture',
  name: 'Overall architecture',
  path: null,
  files: 1_278,
  lines: 420_000,
  description: '49 crates in 11 layers. Clients talk to one RPC facade; the agent loop drives providers and tools; stores are durable content; foundation is imported by all.',
  children: [
    { node: 'cluster:1a2b3c4d', name: 'Clients', path: 'crates/desktop', files: 410, lines: 120_000, kind: 'cluster', description: 'Desktop (Tauri + React) and TUI; talk to rpc over one WebSocket.' },
    { node: 'unit:crates/rpc', name: 'RPC facade', path: 'crates/rpc', files: 40, lines: 18_000, description: 'jsonrpsee methods for every plugin; forwards to the owning engine.' },
    { node: 'cluster:2b3c4d5e', name: 'Agent loop', path: 'crates/agent', files: 120, lines: 60_000, kind: 'cluster', description: 'Session actors, model backends, the tool loop, ACP children.' },
    {
      node: 'cluster:3c4d5e6f',
      name: 'Foundation',
      path: 'crates/runtime',
      files: 90,
      lines: 30_000,
      kind: 'cluster',
      description: 'Runtime, platform, tool registry, surface, secrets — imported by all.',
      children: [
        { node: 'unit:crates/runtime', name: 'Runtime', path: 'crates/runtime', files: 30, lines: 12_000, description: 'Event bus, registry lookups, side calls.' },
        { node: 'unit:crates/tools', name: 'Tool registry', path: 'crates/tools', files: 25, lines: 9_000, description: 'RegisterTool, risk classes, the registry every plugin fills.' },
        { node: 'unit:crates/platform', name: 'Platform', path: 'crates/platform', files: 20, lines: 6_000, description: 'Paths, processes, OS facts.' },
        { node: 'unit:crates/surface', name: 'Surface', path: 'crates/surface', files: 15, lines: 3_000, description: 'json-render specs, Graph, Mermaid lowering.' },
      ],
    },
    STORES,
    { node: 'cluster:4d5e6f70', name: 'Local models + speech', path: 'crates/models', files: 60, lines: 20_000, kind: 'cluster', description: 'Decision model, voice, meeting and talk drive sessions.' },
    {
      node: 'cluster:5e6f7081',
      name: 'World access',
      path: 'crates/web',
      files: 70,
      lines: 22_000,
      kind: 'cluster',
      description: 'HTTP + extraction, headless browser, scrape/crawl/map, computer.',
      children: [
        { node: 'unit:crates/web', name: 'Web', path: 'crates/web', files: 20, lines: 7_000, description: 'HTTP fetch and extraction.' },
        { node: 'unit:crates/browser', name: 'Browser', path: 'crates/browser', files: 30, lines: 10_000, description: 'Headless Chromium control.' },
        { node: 'unit:crates/scrape', name: 'Scrape', path: 'crates/scrape', files: 20, lines: 5_000, description: 'Crawl, map and scrape tools.' },
      ],
    },
    { node: 'cluster:6f708192', name: 'Channels + peers', path: 'crates/signal', files: 80, lines: 25_000, kind: 'cluster', description: 'Signal, Slack, Telegram, A2A, link, nodes, relay → session input.' },
    { node: 'cluster:708192a3', name: 'Mesh + sync', path: 'crates/mesh', files: 45, lines: 15_000, kind: 'cluster', description: 'Engines, replication, sandboxes; owns remote agent sessions.' },
    { node: 'cluster:8192a3b4', name: 'Orchestration', path: 'crates/roster', files: 110, lines: 35_000, kind: 'cluster', description: 'Roster, tasks, schedules, the double, dreaming, app, update.' },
    { node: 'unit:crates/theuth', name: 't binary', path: 'crates/theuth', files: 5, lines: 1_200, description: 'main() builds the Runtime and starts every plugin.' },
  ],
}

const TOP_EDGES: EdgeSpec[] = [
  ['cluster:1a2b3c4d', 'unit:crates/rpc', 'WebSocket JSON-RPC', inferred('ui/src/rpc/transport.ts dials the daemon; no manifest edge')],
  ['unit:crates/theuth', 'cluster:3c4d5e6f', 'main() builds Runtime', manifest(9, 'crates/theuth', 'crates/runtime')],
  ['cluster:5e6f7081', 'cluster:4d5e6f70', 'browser_run decisions', manifest(2, 'crates/browser', 'crates/decision')],
  ['unit:crates/rpc', 'cluster:2b3c4d5e', 'send_input → ask', manifest(3, 'crates/rpc', 'crates/agent')],
  ['unit:crates/rpc', 'cluster:9f3a1c2e', 'lens, doc, knowledge', manifest(5, 'crates/rpc', 'crates/codeindex')],
  ['unit:crates/rpc', 'cluster:8192a3b4', 'schedule, tasks, roster', manifest(7, 'crates/rpc', 'crates/roster')],
  ['unit:crates/rpc', 'cluster:6f708192', 'settings + pairing', manifest(4, 'crates/rpc', 'crates/signal')],
  ['cluster:6f708192', 'cluster:2b3c4d5e', 'inbound → session input', manifest(6, 'crates/signal', 'crates/agent')],
  ['cluster:6f708192', 'cluster:8192a3b4', 'desks + tasks', manifest(2, 'crates/slack', 'crates/roster')],
  ['cluster:8192a3b4', 'cluster:2b3c4d5e', 'wake, run, spawn', manifest(8, 'crates/roster', 'crates/agent')],
  ['cluster:8192a3b4', 'cluster:708192a3', 'stats, sandboxes', manifest(2, 'crates/tasks', 'crates/mesh')],
  ['cluster:9f3a1c2e', 'cluster:708192a3', 'change feeds, engine ids', manifest(1, 'crates/document', 'crates/mesh')],
  ['cluster:4d5e6f70', 'cluster:2b3c4d5e', 'meeting/talk drive', manifest(3, 'crates/voice', 'crates/agent')],
  ['cluster:708192a3', 'cluster:2b3c4d5e', 'own agent sessions', manifest(2, 'crates/mesh', 'crates/agent')],
  ['cluster:2b3c4d5e', 'cluster:3c4d5e6f', 'provider HTTP client', manifest(12, 'crates/agent', 'crates/runtime')],
]
const STORES_EDGES: EdgeSpec[] = [
  ['unit:crates/knowledge', 'unit:crates/document', 'board anchors', manifest(1, 'crates/knowledge', 'crates/document')],
  ['unit:crates/artifact', 'unit:crates/document', 'replica feeds', manifest(1, 'crates/artifact', 'crates/document')],
  ['unit:crates/document', 'unit:crates/codeindex', 'anchor locate', [...manifest(1, 'crates/document', 'crates/codeindex'), ...refs(4, 'crates/document/src/anchor.rs#L88')]],
  ['unit:crates/knowledge', 'unit:crates/search', 'fuzzy envelope', imports(12, 'crates/knowledge/src/find.rs#L3')],
  ['unit:crates/codeindex', 'unit:crates/search', 'snippet envelope', imports(6, 'crates/codeindex/src/tools.rs#L19')],
  ['unit:crates/document', 'unit:crates/search', 'board search', inferred('search::Envelope named in document docs, no import')],
]
const CODEINDEX_EDGES: EdgeSpec[] = [
  ['unit:crates/codeindex/src/walk.rs', 'cluster:7c1e9a02', 'files to parse', refs(22, 'crates/codeindex/src/index.rs#L199')],
  ['cluster:7c1e9a02', 'unit:crates/codeindex/src/store.rs', 'rows', refs(37, 'crates/codeindex/src/parse/mod.rs#L140')],
  ['unit:crates/codeindex/src/index.rs', 'unit:crates/codeindex/src/walk.rs', 'reconcile', refs(9, 'crates/codeindex/src/index.rs#L371')],
  ['unit:crates/codeindex/src/index.rs', 'unit:crates/codeindex/src/rank.rs', 'persist rank', refs(4, 'crates/codeindex/src/index.rs#L417')],
  ['unit:crates/codeindex/src/index.rs', 'unit:crates/codeindex/src/git.rs', 'co-changes', refs(2, 'crates/codeindex/src/index.rs#L250')],
  ['unit:crates/codeindex/src/rank.rs', 'unit:crates/codeindex/src/store.rs', 'ranks', refs(6, 'crates/codeindex/src/rank.rs#L757')],
  ['unit:crates/codeindex/src/lens.rs', 'unit:crates/codeindex/src/store.rs', 'lens queries', refs(31, 'crates/codeindex/src/lens.rs#L314')],
  ['unit:crates/codeindex/src/tools.rs', 'unit:crates/codeindex/src/store.rs', 'symbol lookups', refs(44, 'crates/codeindex/src/tools.rs#L490')],
  ['unit:crates/codeindex/src/tools.rs', 'unit:crates/codeindex/src/event.rs', 'CodeToolRan', refs(6, 'crates/codeindex/src/tools.rs#L188')],
  ['unit:crates/codeindex/src/index.rs', 'unit:crates/codeindex/src/event.rs', 'CodeReconciled', refs(2, 'crates/codeindex/src/index.rs#L533')],
]
const FOUNDATION_EDGES: EdgeSpec[] = [
  ['unit:crates/tools', 'unit:crates/runtime', 'tool events', manifest(1, 'crates/tools', 'crates/runtime')],
  ['unit:crates/surface', 'unit:crates/runtime', 'spec publish', manifest(1, 'crates/surface', 'crates/runtime')],
  ['unit:crates/runtime', 'unit:crates/platform', 'paths + processes', manifest(1, 'crates/runtime', 'crates/platform')],
]
const EDGES: Record<string, EdgeSpec[]> = { 'overall-architecture': TOP_EDGES, 'cluster:9f3a1c2e': STORES_EDGES, 'unit:crates/codeindex': CODEINDEX_EDGES, 'cluster:3c4d5e6f': FOUNDATION_EDGES }

const LIFECYCLE: Spec = {
  node: 'request-lifecycle',
  name: 'Request lifecycle',
  path: null,
  files: 60,
  lines: 20_000,
  description: 'One user turn: the client sends send_input over the WebSocket; rpc forwards to the session actor; the loop streams provider frames back as events.',
  children: [
    { node: 'sym:ts:crates/desktop/ui/src/rpc/transport.ts#send(function)', name: 'Client send', path: 'crates/desktop/ui/src/rpc/transport.ts', files: 1, lines: 200, description: 'The WebSocket frame.' },
    { node: 'sym:rust:crates/rpc/src/api.rs#send_input(method)', name: 'send_input', path: 'crates/rpc/src/api.rs', files: 1, lines: 60, description: 'jsonrpsee method.' },
    { node: 'sym:rust:crates/agent/src/supervisor.rs#Agents::handle(method)', name: 'Supervisor', path: 'crates/agent/src/supervisor.rs', files: 1, lines: 300, description: 'Finds the live session actor.' },
    { node: 'sym:rust:crates/agent/src/loop_.rs#turn(function)', name: 'Turn loop', path: 'crates/agent/src/loop_.rs', files: 1, lines: 900, description: 'Provider stream, tool calls, events.' },
    { node: 'sym:rust:crates/runtime/src/events.rs#publish(function)', name: 'Event bus', path: 'crates/runtime/src/events.rs', files: 1, lines: 120, description: 'Frames back to every subscriber.' },
  ],
}
const LIFECYCLE_EDGES: EdgeSpec[] = [
  ['sym:ts:crates/desktop/ui/src/rpc/transport.ts#send(function)', 'sym:rust:crates/rpc/src/api.rs#send_input(method)', 'WebSocket frame', inferred('method name matches; no static edge across the socket')],
  ['sym:rust:crates/rpc/src/api.rs#send_input(method)', 'sym:rust:crates/agent/src/supervisor.rs#Agents::handle(method)', 'ask', [{ Call: { site: 'crates/rpc/src/api.rs#L2607', from: 'send_input', to: 'Agents::handle' } }]],
  ['sym:rust:crates/agent/src/supervisor.rs#Agents::handle(method)', 'sym:rust:crates/agent/src/loop_.rs#turn(function)', 'forward input', [{ Call: { site: 'crates/agent/src/supervisor.rs#L3660', from: 'Agents::handle', to: 'turn' } }]],
  ['sym:rust:crates/agent/src/loop_.rs#turn(function)', 'sym:rust:crates/runtime/src/events.rs#publish(function)', 'stream events', refs(40, 'crates/agent/src/loop_.rs#L5131')],
]
EDGES['request-lifecycle'] = LIFECYCLE_EDGES

const IMPORTED: Spec = {
  node: 'imported:.omm',
  name: 'imported: .omm',
  path: null,
  files: 0,
  lines: 0,
  description: 'Three perspectives read from the repository’s .omm/ directory. Read-only; freshness unknown.',
  children: [
    { node: 'ext:cli', name: 'CLI', path: 'src/cli.ts', files: 1, lines: 0, description: 'omm’s commander entry.' },
    { node: 'ext:viewer', name: 'Viewer', path: 'viewer', files: 0, lines: 0, description: 'The browser viewer bundle.' },
    { node: 'ext:parsers', name: 'Parsers', path: 'src/parse', files: 0, lines: 0, description: 'Two regex Mermaid parsers.' },
  ],
}

const PERSPECTIVES: { spec: Spec; kind: string; origin: MapOrigin }[] = [
  { spec: TOP, kind: 'overall-architecture', origin: 'Generated' },
  { spec: LIFECYCLE, kind: 'request-lifecycle', origin: 'Generated' },
  { spec: IMPORTED, kind: 'imported', origin: 'Imported' },
]

const PROVENANCE = { model: 'claude-fable-5-1', git_head: '5cddad8e9f1a', index_state: '1278:1759900000000', generated_ms: Date.now() - 3 * 60_000, call_ids: ['req_1', 'req_2'], cost_nanos: 40_000_000, currency: 'USD', input_tokens: 12_400, output_tokens: 1_500, repaired: false, facts_hash: 'a1b2c3d4' }

const thumbnail = (s: Spec): GraphProps | null =>
  s.children
    ? ({
        nodes: s.children.slice(0, 12).map((c) => ({ id: c.node, label: c.name })),
        edges: (EDGES[s.node] ?? []).map(([from, to]) => ({ from, to })),
      } as unknown as GraphProps)
    : null

const diagramOf = (s: Spec): GraphProps | null =>
  s.children
    ? ({
        nodes: s.children.map((c) => ({ id: c.node, label: c.name, sub: c.path ?? undefined, kind: c.kind === 'cluster' ? 'rounded' : undefined })),
        edges: (EDGES[s.node] ?? []).map(([from, to, label]) => ({ from, to, label })),
      } as unknown as GraphProps)
    : null

const mermaidOf = (s: Spec): string | null =>
  s.children
    ? ['flowchart TB', ...s.children.map((c, i) => `  n${i}["${c.name}<br/>${c.path ?? ''}"]`), ...(EDGES[s.node] ?? []).map(([from, to, label]) => `  n${s.children!.findIndex((c) => c.node === from)} -->|"${label}"| n${s.children!.findIndex((c) => c.node === to)}`)].join('\n')
    : null

class MockMap {
  state: MapMeta['state'] = 'Structure'
  seq = 200
  job: MapJob | null = null
  /** Elements with prose so far (structure view before generation, S2). */
  named = new Set<string>()
  timers: ReturnType<typeof setTimeout>[] = []
  constructor(readonly emit: (e: CodeMapProgress | CodeMapChanged) => void) {}

  meta(root: string): MapMeta {
    return { root, seq: ++this.seq, schema: 1, state: this.state, index_state: '1278:1759900000000', git_head: '7a1e02c3', stale_elements: 1, job: this.job, ms: 4 }
  }

  index(root: string): MapIndex {
    const perspectives: MapPerspective[] = PERSPECTIVES.map(({ spec, kind, origin }) => {
      const tree: MapTreeRow[] = []
      this.walk(spec, spec.node, null, 1, tree, origin)
      return { id: spec.node, kind, title: spec.name, origin, elements: tree.length, stale: tree.filter((r) => r.stale).length, tree }
    })
    // the generating index lists perspectives as they land; before any lands, only the structure perspective exists
    const shown = this.state === 'Structure' ? perspectives.slice(0, 1) : perspectives
    return { meta: this.meta(root), perspectives: shown, profile: { name: 'theuth', languages: ['rust', 'ts'] } }
  }
  walk(s: Spec, pid: string, parent: string | null, depth: number, out: MapTreeRow[], origin: MapOrigin): void {
    for (const c of s.children ?? []) {
      const id = `${pid}/${c.node}`
      const named = origin !== 'Generated' || this.state !== 'Structure' || this.named.has(id)
      out.push({ id, node: c.node, parent, depth, name: named ? c.name : (c.path?.split('/').pop() ?? c.name), path: c.path, kind: c.kind ?? (c.children ? 'unit' : 'file'), children: c.children?.length ?? 0, leaf: !c.children, stale: !!c.stale, origin, marks: [] })
      if (depth < 3) this.walk(c, pid, id, depth + 1, out, origin)
    }
  }

  find(id: string): { spec: Spec; pid: string; origin: MapOrigin; crumbs: [string, string][]; parent: string | null } | null {
    const [pid, ...rest] = id.split('/')
    const p = PERSPECTIVES.find((x) => x.spec.node === pid)
    if (!p) return null
    const node = rest.join('/')
    if (!node) return { spec: p.spec, pid: pid!, origin: p.origin, crumbs: [[pid!, p.spec.name]], parent: null }
    const path: Spec[] = []
    const dfs = (s: Spec): boolean => {
      for (const c of s.children ?? []) {
        path.push(c)
        if (c.node === node || dfs(c)) return true
        path.pop()
      }
      return false
    }
    if (!dfs(p.spec)) return null
    const crumbs: [string, string][] = [[pid!, p.spec.name], ...path.map((s): [string, string] => [`${pid}/${s.node}`, s.name])]
    const parentSpec = path.at(-2)
    return { spec: path.at(-1)!, pid: pid!, origin: p.origin, crumbs, parent: parentSpec ? `${pid}/${parentSpec.node}` : pid! }
  }

  element(root: string, id: string): MapElement {
    const hit = this.find(id)
    if (!hit) throw new Error(`no element ${id}`)
    const { spec, pid, origin, crumbs, parent } = hit
    const named = origin !== 'Generated' || this.state !== 'Structure' || this.named.has(id)
    const children: MapChild[] = (spec.children ?? []).map((c) => ({
      id: `${pid}/${c.node}`,
      node: c.node,
      name: named ? c.name : (c.path?.split('/').pop() ?? c.name),
      path: c.path,
      kind: c.kind ?? (c.children ? 'unit' : 'file'),
      files: c.files,
      lines: c.lines,
      share: c.lines / 420_000,
      leaf: !c.children,
      stale: !!c.stale,
      thumbnail: thumbnail(c),
    }))
    const edges: MapEdge[] = (EDGES[spec.node] ?? []).map(([from, to, label, evidence]) => ({ from, to, label: named ? label : '', backed: !evidence.some((e) => 'Inferred' in e), evidence }))
    const anchors = spec.path ? [{ path: spec.path, line_start: null, line_end: null, exists: spec.node !== 'ext:viewer' }] : children.filter((c) => c.path).map((c) => ({ path: c.path!, line_start: null, line_end: null, exists: true }))
    const also = PERSPECTIVES.filter((p) => p.spec.node !== pid && JSON.stringify(p.spec).includes(`"${spec.node}"`)).map((p) => `${p.spec.node}/${spec.node}`)
    return {
      meta: this.meta(root),
      id,
      node: spec.node,
      perspective: pid,
      parent,
      crumbs,
      name: named ? spec.name : (spec.path?.split('/').pop() ?? spec.name),
      path: spec.path,
      kind: spec.kind ?? (spec.children ? 'unit' : 'file'),
      members: (spec.children ?? []).map((c) => c.node),
      anchors,
      fields: { description: named ? spec.description : '', context: named ? (spec.context ?? null) : null, constraint: null, concern: named ? (spec.concern ?? null) : null, todo: null, note: null },
      diagram: diagramOf(spec),
      mermaid: mermaidOf(spec),
      edges,
      children,
      also_in: also,
      stale: !!spec.stale,
      origin: named ? origin : 'Structure',
      provenance: origin === 'Generated' && named ? PROVENANCE : null,
      listing: spec.children ? null : { files: [[spec.path ?? '', spec.lines, spec.lines / 420_000]], symbols: [], more: 0 },
    }
  }

  /** Stage A → B → C over ~6 s: progress every 700 ms, the tree gains names element by element. */
  generate(root: string, session: string, element: string | null): MapJob {
    if (this.job && !['done', 'failed', 'cancelled'].includes(this.job.stage)) return this.job
    const ids = element ? [element] : this.allIds()
    const job: MapJob = { id: `job_${this.seq}`, session, started_ms: Date.now(), stage: 'select', done: 0, total: ids.length, calls: 0, cost_nanos: 0, currency: 'USD', error: null }
    this.job = job
    this.state = 'Generating'
    const step = (i: number) => {
      const id = ids[i]
      if (id === undefined) {
        job.stage = 'done'
        this.state = 'Ready'
        this.emit({ type: 'CodeMapProgress', root, job: job.id, session, stage: 'done', done: job.done, total: job.total, element: null, calls: job.calls, cost_nanos: job.cost_nanos, currency: 'USD', error: null, seq: ++this.seq })
        this.emit({ type: 'CodeMapChanged', root, elements: ids, stale: [], more: 0, seq: ++this.seq })
        return
      }
      this.named.add(id)
      job.done = i + 1
      job.calls += i % 3 === 0 ? 1 : 0
      job.cost_nanos += 15_000_000
      job.stage = i === 0 ? 'top' : i < 4 ? 'flow' : 'describe'
      this.emit({ type: 'CodeMapProgress', root, job: job.id, session, stage: job.stage, done: job.done, total: job.total, element: id, calls: job.calls, cost_nanos: job.cost_nanos, currency: 'USD', error: null, seq: ++this.seq })
      this.emit({ type: 'CodeMapChanged', root, elements: [id], stale: [], more: 0, seq: ++this.seq })
      this.timers.push(setTimeout(() => step(i + 1), 700))
    }
    this.timers.push(setTimeout(() => step(0), 500))
    return job
  }
  allIds(): string[] {
    const out: string[] = []
    for (const { spec, origin } of PERSPECTIVES) {
      if (origin !== 'Generated') continue
      out.push(spec.node)
      const rows: MapTreeRow[] = []
      this.walk(spec, spec.node, null, 1, rows, origin)
      out.push(...rows.map((r) => r.id))
    }
    return out
  }
  cancel(): MapJob {
    for (const t of this.timers) clearTimeout(t)
    this.timers = []
    if (this.job) this.job.stage = 'cancelled'
    this.state = this.named.size ? 'Partial' : 'Structure'
    return this.job ?? { id: 'none', session: '', started_ms: 0, stage: 'cancelled', done: 0, total: 0, calls: 0, cost_nanos: 0, currency: 'USD', error: null }
  }
}

/** One mock per page; `emit` receives the replayed events (the dev fixture dispatches them into the store). */
export function mockMapApi(emit: (e: CodeMapProgress | CodeMapChanged) => void): MapApi & { mock: MockMap } {
  const mock = new MockMap(emit)
  return {
    mock,
    async codemap_overview(root) {
      return mock.index(root)
    },
    async codemap_element(root, element) {
      return mock.element(root, element)
    },
    async codemap_generate(root, session, element) {
      return mock.generate(root, session, element)
    },
    async codemap_cancel() {
      return mock.cancel()
    },
    async codemap_search(root, query, limit) {
      return localSearch(mock.index(root), query, limit ?? 20)
    },
    async codemap_export() {
      throw new Error('<root>/.omm/ exists and was not written by theuth (no generator: key) — export refused')
    },
  }
}
