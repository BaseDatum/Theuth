// 171 visual check, dev only: `bunx vite` then open `/src/dev/cells-fixture.html` (`?quit` adds the pre-quit sheet,
// `?theme=dark`). The §3.8 runbook and the §11.2 variants rendered by the real `MessageAssistant` → `CellFence` →
// `Cell` against a fake `Api` and RPC client (no engine): every `cell_*` answer below is local.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { KernelStrip } from '@/components/cells/KernelStrip'
import { QuitSheet } from '@/components/cells/QuitSheet'
import { refKey } from '@/components/cells/store'
import { MessageAssistant } from '@/components/chat/MessageAssistant'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/api'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { CellState } from '@/rpc/bindings/CellState'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import type { KernelState } from '@/rpc/bindings/KernelState'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

const MIN = 60_000
const now = Date.now()
const CWD = '~/dr-t6/frp-integration/stacks/sa-frp-integration'
const fence = (body: string, lang = 'bash') => `\`\`\`${lang}\n${body}\n\`\`\``
const EXPORT = '#| name: export-ir-prod1\n#| label: Export state\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: Exported to\npulumi stack export -s ir-prod1 --file /tmp/sa-frp-integration-ir-prod1-pre.json\necho "Exported to /tmp/sa-frp-integration-ir-prod1-pre.json"'
const PREVIEW = '#| name: preview-ir-prod1\n#| label: Preview\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: 3 to update\npulumi preview -s ir-prod1 --refresh=false --diff'
const APPLY = '#| name: apply-ir-prod1\n#| label: Apply to ir-prod1\n#| danger\n#| after: preview-ir-prod1\n#| review\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: 3 updated\npulumi up -s ir-prod1 --refresh=false --diff'
const RUNBOOK = `Three steps. Run them in order; the apply arms after the preview matches.\n\n${[EXPORT, PREVIEW, APPLY].map((b) => fence(b)).join('\n\n')}\n\nPreview matches: 3 updates, nothing replaced or deleted. Apply when ready.`
const PLAN = `Previewing update (ir-prod1):
     \x1b[1mType                              Name                          Plan       Info\x1b[0m
     pulumi:pulumi:Stack               sa-frp-integration-ir-prod1
 \x1b[33m~\x1b[0m   ├─ aws:secretsmanager:Secret      frp-api-credentials           \x1b[33mupdate\x1b[0m     [diff: ~tags]
 \x1b[33m~\x1b[0m   ├─ aws:iam:Policy                 replica-read                  \x1b[33mupdate\x1b[0m     [diff: ~policy]
 \x1b[33m~\x1b[0m   └─ aws:iam:Role                   frp-integration-role          \x1b[33mupdate\x1b[0m     [diff: ~managedPolicyArns]
\x1b[1mResources:\x1b[0m
    \x1b[33m~ 3 to update\x1b[0m
    4 unchanged
`
const PROMPT = `${PLAN.replaceAll('\n', '\r\n')}\r\n\x1b[1mDo you want to perform this update?\x1b[0m  [Use arrows to move, type to filter]\r\n  yes\r\n\x1b[36m> no\x1b[0m\r\n  details\r\n`

type Scenario = { title: string; session: string; text: string; states: (ref: CellRef, i: number) => Partial<CellState>; kernels?: KernelState['phase'] }
const sid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`
const kernelOf = (session: string, phase: KernelState['phase']): KernelState => ({
  id: { scope: { type: 'session', id: session }, spec: { type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' } },
  phase,
  label: 'bash 5.2 (Homebrew)',
  cwd: '/Users/sky/dr-t6/frp-integration/stacks/sa-frp-integration',
  env_warning: null,
})
const runs = new Map<string, { run: CellRun; text: string }>()
function run(ref: CellRef, over: Partial<CellRun>, text = ''): CellRun {
  const r: CellRun = {
    id: `r_${refKey(ref).replace(/\W/g, '')}`,
    cell: ref,
    mode: { type: 'cell' },
    pass: null,
    kernel: kernelOf(ref.type === 'chat' ? ref.session : '', 'ready').id,
    interpreter: 'bash 5.2 (Homebrew) /opt/homebrew/bin/bash',
    cwd: CWD,
    env_names: ['AWS_PROFILE'],
    presented_sha: 'x',
    code_sha: 'x',
    edited: false,
    pressed_by: 'desktop',
    state: 'done',
    exit: { type: 'code', value: 0 },
    secs: 41,
    expect: [],
    summary: null,
    output_bytes: text.length,
    queued_ms: now - 20 * MIN,
    started_ms: now - 20 * MIN,
    ended_ms: now - 19 * MIN,
    masked: 0,
    private: false,
    name: null,
    input: null,
    ...over,
  }
  runs.set(r.id, { run: r, text })
  return r
}
const met = (value: string, ok = true) => [{ item: { kind: 'text' as const, value }, met: ok }]
const exported = (ref: CellRef) => run(ref, { secs: 2.1, expect: met('Exported to'), summary: '“Exported to /tmp/sa-frp-integration-ir-prod1-pre.json”' }, 'Exported to /tmp/sa-frp-integration-ir-prod1-pre.json\n')
const previewed = (ref: CellRef) => run(ref, { expect: met('3 to update'), summary: '~3 update · 0 replace · 0 delete', ended_ms: now - 2 * MIN }, PLAN)
const armed = { by: 'r_x', at_ms: now - 2 * MIN, summary: '~3 update · 0 replace · 0 delete' }

const SCENARIOS: Scenario[] = [
  {
    title: 'A · chat runbook — 1 done and folded, 2 done and open, 3 danger and armed',
    session: sid(1),
    text: RUNBOOK,
    kernels: 'ready',
    states: (ref, i) => (i === 0 ? { latest: exported(ref) } : i === 1 ? { latest: previewed(ref) } : { armed }),
  },
  {
    title: 'B · running — the danger cell live: Stop, Kill after 5 s, the arrow prompt in the terminal',
    session: sid(2),
    text: RUNBOOK,
    kernels: 'busy',
    states: (ref, i) =>
      i === 0
        ? { latest: exported(ref) }
        : i === 1
          ? { latest: previewed(ref) }
          : { armed, latest: run(ref, { state: 'running', exit: null, secs: null, ended_ms: null, started_ms: now - 41_000 }) },
  },
  { title: 'Ready + disarmed — before the preview ran', session: sid(3), text: RUNBOOK, states: () => ({}) },
  {
    title: 'Failed + Run all stopped at a failed step',
    session: sid(4),
    text: `${fence(EXPORT)}\n\n${fence('#| name: verify\n#| label: Verify\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| expect: exit 0\npulumi stack output -s ir-prod1 --json')}`,
    states: (ref, i) =>
      i === 0
        ? { latest: { ...exported(ref), pass: 'p_1' } }
        : { latest: run(ref, { pass: 'p_1', state: 'failed', exit: { type: 'code', value: 1 }, secs: 3, expect: met('exit 0', false) }, "\x1b[31merror:\x1b[0m no stack named 'ir-prod1' found\n\x1b[31merror:\x1b[0m failed to load project config\n") },
  },
  {
    title: 'Superseded',
    session: sid(5),
    text: fence(PREVIEW),
    states: (ref) => ({ superseded_by: { ...ref, entry: '99' }, latest: previewed(ref) }),
  },
  {
    title: 'Lint chips + cwd missing + warning',
    session: sid(6),
    text: fence('#| label: Install\n#| cwd: ~/dr-t6/x\ncurl -fsSL https://example.test/install.sh | sh'),
    states: () => ({
      chips: [
        { kind: 'invisible', text: '2 invisible characters', stops_run_all: true, count: 2, name: null },
        { kind: 'pipe_to_shell', text: 'pipes a download into a shell', stops_run_all: true, count: null, name: null },
      ],
      problems: [{ code: 'cwd_not_found', severity: 'warning', message: '~/dr-t6/x not found', key: 'cwd' }],
    }),
  },
  {
    title: 'Invalid (after_unknown) · queued · lost',
    session: sid(7),
    text: `${fence('#| label: Apply\n#| cwd: /tmp\n#| after: preview\nmake apply')}\n\n${fence('#| label: Build\n#| cwd: /tmp\nmake build')}\n\n${fence('#| label: Deploy\n#| cwd: /tmp\nmake deploy')}`,
    states: (ref, i) =>
      i === 0
        ? { problems: [{ code: 'after_unknown', severity: 'error', message: '`after: preview` names no cell; cells here: build, deploy', key: 'after' }] }
        : i === 1
          ? { latest: run(ref, { state: 'queued', exit: null, secs: null, started_ms: null, ended_ms: null }) }
          : { latest: run(ref, { state: 'lost', exit: null, secs: 12 }, 'building…\n') },
  },
  {
    title: 'Private',
    session: sid(8),
    text: fence('#| label: Whoami\n#| cwd: /tmp\n#| private\naws sts get-caller-identity'),
    states: (ref) => ({ latest: run(ref, { secs: 0.9 }, '{ "Account": "123456789012", "Arn": "arn:aws:sts::…:assumed-role/ir-prod1-admin/sky" }\n') }),
  },
]

const sessionOf = (scope: CellScope) => SCENARIOS.find((s) => s.session === scope.id)
const api = new Proxy({} as Api, {
  get: (_t, name: string) =>
    async (...args: unknown[]) => {
      switch (name) {
        case 'cell_state': {
          const [scope, refs] = args as [CellScope, CellRef[]]
          const s = sessionOf(scope)
          return {
            cells: refs.map((ref, i): CellState => ({
              cell: ref,
              digest: 'x',
              problems: [],
              chips: [],
              armed: null,
              superseded_by: null,
              latest: null,
              stale: false,
              member_edit: null,
              interpreter: { kind: 'bash', source: 'Homebrew', path: '/opt/homebrew/bin/bash', version: '5.2', why: '', auto: true },
              host: null,
              ...s?.states(ref, ref.type === 'chat' ? ref.ordinal - 1 : i),
            })),
            kernels: s?.kernels ? [kernelOf(s.session, s.kernels)] : [],
          }
        }
        case 'cell_run_get': {
          const r = runs.get(args[0] as string)
          if (!r) throw new Error('not found')
          return { run: r.run, outputs: [{ output_type: 'stream', name: 'stdout', text: r.text }] }
        }
        case 'cell_runs':
          return [...runs.values()].map((r) => r.run)
        case 'cell_live':
          return [...runs.values()].map((r) => r.run).filter((r) => r.state === 'running')
        default:
          throw new Error(`fixture: ${name} not answered`)
      }
    },
})
const b64 = (s: string) => btoa(Array.from(new TextEncoder().encode(s), (b) => String.fromCharCode(b)).join(''))
const client = {
  stream: async (_m: string, _p: unknown[], _u: string, onItem: (c: unknown) => void) => {
    setTimeout(() => onItem({ type: 'reset', data: b64(PROMPT), rows: 30, cols: 100 }), 50)
    return () => undefined
  },
} as unknown as RpcClient
active().connection.setState({ api, client, info: null })

const context = (seq: number) => ({ target: { kind: 'message', seq }, active: true }) as unknown as ContextEntry
const quit = new URLSearchParams(location.search).has('quit')

function Fixture() {
  return (
    <TooltipProvider>
      {/* The app stylesheet locks body scroll (the shell scrolls its panes); the fixture scrolls its own column. */}
      <div className="h-dvh overflow-y-auto">
      <div className="mx-auto flex max-w-[768px] flex-col gap-10 px-4 py-8">
        {SCENARIOS.map((s, i) => (
          <section key={s.session} className="flex flex-col gap-2">
            <h2 className="border-b border-border-subtle pb-1 text-xs font-semibold tracking-wide text-fg-tertiary uppercase">{s.title}</h2>
            <MessageAssistant id={`e${i}`} text={s.text} open={false} session={s.session} turn={1} context={context(40 + i)} />
            {s.kernels && <KernelStrip session={s.session} />}
            {s.kernels && <div className="mx-auto h-12 w-full rounded-b-xl border border-border bg-layer-1 px-4 py-3 text-sm text-fg-tertiary md:max-w-[768px]">Message theuth…</div>}
          </section>
        ))}
        {quit && <QuitSheet live={[...runs.values()].map((r) => r.run).filter((r) => r.state === 'running')} api={api} onQuit={() => undefined} onCancel={() => undefined} />}
      </div>
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
