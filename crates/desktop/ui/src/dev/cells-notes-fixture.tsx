// 171 visual check (U2), dev only: `bunx vite` then open `/src/dev/cells-notes-fixture.html` (`?theme=dark`; `?hl` is
// the highlighting note instead). A note in the real `Editor` (BlockNote over a local Y.Doc, `CodeBlock` → `NoteCell`)
// with a `cells` defaults block, a bash cell, a python cell with an HTML output, a stale cell and a member-edited cell,
// the header pill (`?mesh`: in a mesh, `Sky's MacBook` + `xeus`, `sum-check` last ran on xeus), then the same note as a share-link guest sees it. Every `cell_*` answer below is local (no engine).
import '@/styles.css'
import { BlockNoteEditor } from '@blocknote/core'
import { blocksToYXmlFragment } from '@blocknote/core/yjs'
import { QueryClientProvider } from '@tanstack/react-query'
import { FRAGMENT, schema } from '@theuth/documents/schema'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Awareness } from 'y-protocols/awareness'
import * as Y from 'yjs'
import { NoteKernelPill } from '@/components/cells/NoteKernelPill'
import { Editor } from '@/components/editor/Editor'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { EngineProvider } from '@/lib/doc/provider'
import type { Api } from '@/rpc/api'
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { CellState } from '@/rpc/bindings/CellState'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { MESH } from '@/components/tasks/mock'
import { active } from '@/store/engine-scope'
import { PLAIN, PWSH, PYTHON, SAMPLE } from './cells-sample'

const DOC = 'notefixture000000000000000'
const GUEST_DOC = 'notefixtureguest0000000000'
const now = Date.now()
const MIN = 60_000
const MESHED = new URLSearchParams(location.search).has('mesh')
const XEUS = 'xeus00000000000000000000000000000000000000000000000000000000xeus'

const HIGHLIGHT = [
  { type: 'heading', content: 'East pre-flight' },
  { type: 'codeBlock', props: { language: 'cells' }, content: '#| cwd: /tmp/cellhl' },
  { type: 'codeBlock', props: { language: 'shellscript' }, content: SAMPLE },
  { type: 'codeBlock', props: { language: 'python' }, content: PYTHON },
  { type: 'codeBlock', props: { language: 'pwsh' }, content: PWSH },
  { type: 'paragraph', content: 'The same code as a plain code block:' },
  { type: 'codeBlock', props: { language: 'bash' }, content: PLAIN },
]
const BLOCKS = new URLSearchParams(location.search).has('hl') ? HIGHLIGHT : [
  { type: 'heading', content: 'Cost check' },
  { type: 'paragraph', content: 'Spend by project from the usage export. The first cell fetches it; the second runs on the project’s own virtualenv.' },
  { type: 'codeBlock', props: { language: 'cells' }, content: '#| cwd: ~/Develop/costs\n#| shell: /opt/homebrew/bin/bash\n#| python: .venv' },
  { type: 'codeBlock', props: { language: 'bash' }, content: '#| name: fetch-usage\n#| label: Fetch usage\ncurl -fsSL https://example.test/usage.csv -o usage-2026-10.csv\nwc -l usage-2026-10.csv' },
  {
    type: 'codeBlock',
    props: { language: 'python' },
    content: '#| name: top-projects\n#| python: .venv\nimport pandas as pd\ndf = pd.read_csv("usage-2026-10.csv")\ntop = df.groupby("project").cost_usd.sum().nlargest(3)\ntop.to_frame()',
  },
  { type: 'heading', props: { level: 2 }, content: 'Checks' },
  { type: 'codeBlock', props: { language: 'bash' }, content: "#| name: sum-check\nawk -F, '{s+=$3} END {printf \"%.2f\\n\", s}' usage-2026-10.csv" },
  { type: 'codeBlock', props: { language: 'bash' }, content: '#| name: rotate-key\n#| label: Rotate key\n#| danger\naws iam create-access-key --user-name frp-bot\naws iam list-access-keys --user-name frp-bot' },
  { type: 'codeBlock', props: { language: 'bash' }, content: 'echo "a plain snippet stays code"' },
  { type: 'codeBlock', props: { language: 'mermaid' }, content: 'flowchart LR\n  Fetch --> Top --> Check' },
]

const TABLE = `<table><thead><tr><th>project</th><th>cost_usd</th></tr></thead><tbody><tr><td>theuth</td><td>412.80</td></tr><tr><td>sa-frp-integration</td><td>238.15</td></tr><tr><td>dr-t6</td><td>171.40</td></tr></tbody></table>`
const b64 = (s: string) => btoa(Array.from(new TextEncoder().encode(s), (b) => String.fromCharCode(b)).join(''))

function run(ref: CellRef, name: string, over: Partial<CellRun> = {}): CellRun {
  return {
    id: `r_${name}`,
    cell: ref,
    mode: { type: 'cell' },
    pass: null,
    kernel: { scope: { type: 'doc', id: DOC }, spec: { type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' } },
    interpreter: 'bash 5.2 (Homebrew) /opt/homebrew/bin/bash',
    cwd: '/Users/sky/Develop/costs',
    env_names: [],
    presented_sha: 'x',
    code_sha: 'x',
    edited: false,
    pressed_by: 'desktop',
    state: 'done',
    exit: { type: 'code', value: 0 },
    secs: 0.8,
    expect: [],
    summary: null,
    output_bytes: 10,
    queued_ms: now - 12 * MIN,
    started_ms: now - 12 * MIN,
    ended_ms: now - 12 * MIN + 800,
    masked: 0,
    private: false,
    name,
    input: null,
    ran_past: null,
    ...over,
  }
}
const python = { kind: 'python' as const, source: '.venv', path: '/Users/sky/Develop/costs/.venv/bin/python', version: '3.12.4', why: 'nearest .venv: ~/Develop/costs/.venv', auto: true }
const OUTPUTS: Record<string, unknown[]> = {
  'r_fetch-usage': [{ output_type: 'stream', name: 'stdout', text: '    4812 usage-2026-10.csv\n' }],
  'r_top-projects': [{ output_type: 'execute_result', execution_count: 3, data: { 'text/html': TABLE, 'text/plain': 'project  cost_usd\ntheuth 412.80' }, metadata: {} }],
  'r_sum-check': [{ output_type: 'stream', name: 'stdout', text: '972.70\n' }],
  'r_rotate-key': [{ output_type: 'stream', name: 'stdout', text: '{ "AccessKeyMetadata": [ … ] }\n' }],
}
function stateOf(ref: CellRef): Partial<CellState> {
  const name = ref.type === 'note' ? ref.key.value : ''
  switch (name) {
    case 'fetch-usage':
      return { latest: run(ref, name), interpreter: { kind: 'bash', source: 'homebrew', path: '/opt/homebrew/bin/bash', version: '5.2', why: '', auto: true } }
    case 'top-projects':
      return {
        latest: run(ref, name, { kernel: { scope: { type: 'doc', id: DOC }, spec: { type: 'python', path: python.path } }, interpreter: 'Python 3.12.4 (.venv)', ended_ms: now - 4 * MIN }),
        interpreter: { ...python, version: '3.12' },
      }
    case 'sum-check':
      return { latest: run(ref, name, { ended_ms: now - 30 * MIN }), stale: true }
    case 'rotate-key':
      return { latest: run(ref, name, { ended_ms: now - 90 * MIN }), member_edit: 'AbCdEf12' }
  }
  return {}
}
const kernels: KernelState[] = [{ id: { scope: { type: 'doc', id: DOC }, spec: { type: 'python', path: python.path } }, phase: 'ready', label: '.venv 3.12', cwd: '/Users/sky/Develop/costs', env_warning: null }]

const api = new Proxy({} as Api, {
  get: (_t, name: string) =>
    async (...args: unknown[]) => {
      switch (name) {
        case 'cell_state': {
          const [scope, refs] = args as [CellScope, CellRef[]]
          return {
            cells: refs.map(
              (ref): CellState => ({ cell: ref, digest: 'x', problems: [], chips: [], armed: null, superseded_by: null, latest: null, stale: false, member_edit: null, interpreter: null, host: null, disarmed: null, ...stateOf(ref) }),
            ),
            kernels: scope.id === DOC ? kernels : [],
          }
        }
        case 'cell_run_get': {
          const id = args[0] as string
          return { run: run({ type: 'note', doc: DOC, key: { kind: 'name', value: id.slice(2) } }, id.slice(2)), outputs: OUTPUTS[id] ?? [] }
        }
        case 'cell_output_file':
          return args[0] === 'r_rotate-key' ? b64('aws iam list-access-keys --user-name frp-bot') : b64("awk -F, '{s+=$3} END {print s}' usage-2026-10.csv")
        case 'cell_interpreters':
          return [
            python,
            { kind: 'python', source: 'system', path: '/usr/bin/python3', version: '3.9.6', why: 'python3 on the login PATH', auto: false },
            { kind: 'monty', source: 'Monty', path: null, version: null, why: 'sandboxed, no tools, no network', auto: false },
            { kind: 'bash', source: 'homebrew', path: '/opt/homebrew/bin/bash', version: '5.2.37', why: 'Homebrew', auto: true },
          ]
        case 'mesh_status':
          return MESHED
            ? { ...MESH, self_id: 'mac00000000000000000000000000000000000000000000000000000000mac1', self_name: "Sky's MacBook", peers: [{ ...MESH.peers[0], id: XEUS, name: 'xeus', reachable: true }] }
            : { ...MESH, peers: [] }
        case 'cell_latest':
          return MESHED && args[0] === DOC ? [latestRow('sum-check', '972.70\n', { id: `${DOC}:sum-check`, doc: DOC, ended_ms: BigInt(now - 4 * MIN), engine: XEUS })] : []
        case 'cell_runs':
          return []
        default:
          throw new Error(`fixture: ${name} not answered`)
      }
    },
})
active().connection.setState({ api, client: null, info: null })

const latestRow = (cell: string, text: string, over: Partial<CellLatest> = {}): CellLatest => ({
  id: `${GUEST_DOC}:${cell}`,
  doc: GUEST_DOC,
  cell,
  run: `r_${cell}`,
  state: 'done',
  exit: 0,
  secs: 0.8,
  expect: null,
  summary: null,
  code_sha: 'x',
  ended_ms: BigInt(now - 20 * MIN),
  text,
  engine: '',
  ...over,
})
const guestApi = {
  cell_latest: async () => [latestRow('fetch-usage', '    4812 usage-2026-10.csv\n'), latestRow('top-projects', 'project  cost_usd\ntheuth  412.80\nsa-frp  238.15\ndr-t6   171.40\n', { summary: 'DataFrame 3×1' })],
} as unknown as Api

/** A provider-shaped object over a local Y.Doc holding `BLOCKS` (no engine, no network). */
function localProvider(id: string): EngineProvider {
  const doc = new Y.Doc()
  const headless = BlockNoteEditor.create({ schema })
  headless.replaceBlocks(headless.document, BLOCKS as never)
  blocksToYXmlFragment(headless, headless.document, doc.getXmlFragment(FRAGMENT))
  const off = () => () => undefined
  return {
    id,
    doc,
    awareness: new Awareness(doc),
    meta: { members: [{ pub: 'AbCdEf12rest', name: 'Alice' }], agent: { color: '#7c3aed' } },
    viewer: false,
    onEvent: off,
    onAuthored: off,
    onStatus: off,
    agentPresence: () => null,
  } as unknown as EngineProvider
}
const owner = localProvider(DOC)
const guest = localProvider(GUEST_DOC)

function Fixture() {
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <section className="flex flex-col">
          <header className="flex h-10 items-center gap-2 border-b border-border-subtle px-4 text-sm">
            <span className="text-fg-tertiary">Notes /</span>
            <span className="font-medium">Cost check</span>
            <NoteKernelPill doc={DOC} />
          </header>
          <div className="flex min-h-[1400px] flex-col">
            <Editor provider={owner} status={{ kind: 'live' }} />
          </div>
        </section>
        <section className="flex flex-col border-t-4 border-border">
          <header className="flex h-10 items-center gap-2 border-b border-border-subtle px-4 text-sm">
            <span className="font-medium">Guest view</span>
            <span className="ml-auto rounded-full border border-border-subtle px-2 py-px text-[11px]">you are a guest · viewer</span>
          </header>
          <div className="flex min-h-[1200px] flex-col">
            <Editor provider={guest} status={{ kind: 'readonly' }} guest={guestApi} />
          </div>
        </section>
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
