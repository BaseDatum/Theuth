// 184 visual check (U), dev only: `bunx vite` then open `/src/dev/notes-184-fixture.html?view=<v>`. Views: `p1` the note
// lives on xeus, its cell ran there; `p2` after Run here; `p3` xeus offline; `dual` also open on studio-mac; `p4` / `p4mesh` /
// `p4un` the Share dialog (via home, via mesh, mesh link unavailable); `move` / `board` the Move to ▸ menu; `single` a lone
// engine (nothing new). Every answer below is local (no engine).
import '@/styles.css'
import { BlockNoteEditor } from '@blocknote/core'
import { blocksToYXmlFragment } from '@blocknote/core/yjs'
import { QueryClientProvider } from '@tanstack/react-query'
import { FRAGMENT, schema } from '@theuth/documents/schema'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Awareness } from 'y-protocols/awareness'
import * as Y from 'yjs'
import { Editor } from '@/components/editor/Editor'
import { MoveNote } from '@/components/notes/MoveNote'
import { ServerBanner, ServerChip } from '@/components/notes/ServerChip'
import { ShareAction, ShareDialog } from '@/components/share/ShareDialog'
import { MESH } from '@/components/tasks/mock'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { EngineProvider } from '@/lib/doc/provider'
import type { Api } from '@/rpc/bindings/client'
import type { CellRef } from "@/rpc/bindings/method.cells.output"
import type { CellRun } from "@/rpc/bindings/method.cells.output"
import type { CellState } from "@/rpc/bindings/method.cells.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { Peer } from "@/rpc/bindings/method.nodes.output"
import { active } from '@/store/engine-scope'

const VIEW = new URLSearchParams(location.search).get('view') ?? 'p1'
const DOC = 'notefixture184000000000000'
const now = Date.now()
const MIN = 60_000
const id = (tag: string) => tag.padEnd(64, '0')
const SELF = id('mac1')
const XEUS = id('xeus')
const STUDIO = id('studio')
const MINI = id('mini')

const peer = (pid: string, name: string, reachable: boolean, seen: number): Peer => ({ ...(MESH.peers[0] as Peer), id: pid, name, reachable, last_seen_ms: now - seen })
const mesh: MeshStatus = {
  ...MESH,
  self_id: SELF,
  self_name: "Sky's MacBook",
  peers: VIEW === 'single' ? [] : [peer(XEUS, 'xeus', VIEW !== 'p3', VIEW === 'p3' ? 40 * MIN : 5_000), peer(STUDIO, 'studio-mac', true, 3_000), peer(MINI, 'old-mini', false, 12 * MIN)],
  docs_homes_on: true,
}

const BLOCKS = [
  { type: 'paragraph', content: 'Steps to roll the staging cluster.' },
  { type: 'codeBlock', props: { language: 'bash' }, content: '#| name: nodes\n#| cwd: ~/Develop/infra\nkubectl get nodes' },
]
const ref: CellRef = { type: 'note', doc: DOC, key: { kind: 'name', value: 'nodes' } }
const ownRun = (): CellRun =>
  ({
    id: 'r_nodes',
    cell: ref,
    mode: { type: 'cell' },
    pass: null,
    kernel: { scope: { type: 'doc', id: DOC }, spec: { type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' } },
    interpreter: 'bash 5.2 (Homebrew) /opt/homebrew/bin/bash',
    cwd: '/Users/sky/Develop/infra',
    env_names: [],
    presented_sha: 'x',
    code_sha: 'x',
    edited: false,
    pressed_by: 'desktop',
    state: 'done',
    exit: { type: 'code', value: 0 },
    secs: 0.6,
    expect: [],
    summary: null,
    output_bytes: 14,
    queued_ms: now - 2_000,
    started_ms: now - 2_000,
    ended_ms: now - 1_000,
    masked: 0,
    private: false,
    name: 'nodes',
    input: null,
    ran_past: null,
  }) as unknown as CellRun
const foreignAge = VIEW === 'p3' ? 60 * MIN : 4 * MIN
const latest = {
  id: `${DOC}:nodes`,
  doc: DOC,
  cell: 'nodes',
  run: 'r_xeus',
  state: 'done',
  exit: 0,
  secs: 0.7,
  expect: null,
  summary: null,
  code_sha: 'x',
  ended_ms: now - foreignAge,
  text: '3 nodes Ready\n',
  engine: VIEW === 'single' ? '' : XEUS,
}
const SECRET = 'c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0MTI'

const api = new Proxy({} as Api, {
  get: (_t, name: string) =>
    async (...args: unknown[]) => {
      switch (name) {
        case 'mesh_status':
          return mesh
        case 'cell_state': {
          const [, refs] = args as [unknown, CellRef[]]
          return {
            cells: refs.map(
              (r): CellState => ({ cell: r, digest: 'x', problems: [], chips: [], armed: null, superseded_by: null, latest: VIEW === 'p2' ? ownRun() : null, stale: false, member_edit: null, interpreter: null, host: null, disarmed: null }),
            ),
            kernels: [],
          }
        }
        case 'cell_latest':
          return [latest]
        case 'cell_run_get':
          return { run: ownRun(), outputs: [{ output_type: 'stream', name: 'stdout', text: '3 nodes Ready\n' }] }
        case 'cell_runs':
        case 'cell_interpreters':
          return []
        case 'doc_members':
          return { members: [], links: [] }
        case 'doc_link_create': {
          if (VIEW === 'p4un') throw new Error('mesh_link_unavailable')
          const via = VIEW === 'p4mesh' ? 'mesh' : 'home'
          const frag = via === 'mesh' ? `1.bWVzaC05MWMy.${DOC}.${SECRET}` : `2.eGV1cy03ZjNh.bWVzaC05MWMy.${DOC}.${SECRET}`
          return { url: `https://relay.theuth.io/d#${frag}`, link: { id: 'l1', role: args[1], name: null, created_ms: now, expires_ms: null, revoked_ms: null }, via }
        }
        case 'doc_move_home': {
          // Never answers: the menu stays in its `Moving…` state for the screenshot.
          const { promise } = Promise.withResolvers<never>()
          return promise
        }
        default:
          throw new Error(`fixture: ${name} not answered`)
      }
    },
})
active().connection.setState({ api, client: null, info: null })

/** A provider-shaped object over a local Y.Doc holding `BLOCKS`, with the 184 `server`/`dual` and a homed meta. */
function localProvider(kind: string): EngineProvider {
  const doc = new Y.Doc()
  const headless = BlockNoteEditor.create({ schema })
  headless.replaceBlocks(headless.document, BLOCKS as never)
  blocksToYXmlFragment(headless, headless.document, doc.getXmlFragment(FRAGMENT))
  const off = () => () => undefined
  return {
    id: DOC,
    doc,
    awareness: new Awareness(doc),
    meta: { title: kind === 'board' ? 'Infra board' : 'Deploy checklist', kind, home: XEUS, members: [], links: [], agent: { color: '#7c3aed' } },
    server: VIEW === 'p3' ? STUDIO : XEUS,
    dual: VIEW === 'dual' ? STUDIO : null,
    status: { kind: 'live' },
    viewer: false,
    onEvent: off,
    onAuthored: off,
    onStatus: off,
    agentPresence: () => null,
  } as unknown as EngineProvider
}
const provider = localProvider(VIEW === 'board' ? 'board' : 'note')

function Fixture() {
  const share = VIEW.startsWith('p4')
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <header className="flex h-12 items-center gap-2 border-b border-border-subtle px-4 text-sm">
          <span className="font-medium">{provider.meta?.title}</span>
          <span className="flex-1" />
          <ServerChip provider={provider} />
          {(VIEW === 'move' || VIEW === 'board' || VIEW === 'single') && <MoveNote provider={provider} />}
          <ShareAction provider={provider} />
        </header>
        <ServerBanner provider={provider} />
        {!share && (
          <div className="flex min-h-[700px] flex-col">
            <Editor provider={provider} status={{ kind: 'live' }} />
          </div>
        )}
        {share && <ShareDialog provider={provider} open onOpenChange={() => undefined} />}
      </div>
    </TooltipProvider>
  )
}

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <Fixture />
    </QueryClientProvider>
  </StrictMode>,
)
