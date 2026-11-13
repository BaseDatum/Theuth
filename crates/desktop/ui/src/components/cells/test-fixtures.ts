// Owner: U1. Test-only builders for the cells tests (the §3.8 runbook, runs, states, a rendering harness).
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import type { Api } from '@/rpc/bindings/client'
import type { CellRef } from "@/rpc/bindings/method.cells.output"
import type { CellRun } from "@/rpc/bindings/method.cells.output"
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import type { CellState } from "@/rpc/bindings/method.cells.output"
import { useConnection } from '@/store/connection'
import { active } from '@/store/engine-scope'
import { cellOf } from './grammar'
import { cells } from './store'

export const SESSION = '11111111-2222-3333-4444-555555555555'

/** §3.8, verbatim. */
export const RUNBOOK = [
  '#| name: export-ir-prod1\n#| label: Export state\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: Exported to\npulumi stack export -s ir-prod1 --file /tmp/sa-frp-integration-ir-prod1-pre.json\necho "Exported to /tmp/sa-frp-integration-ir-prod1-pre.json"',
  '#| name: preview-ir-prod1\n#| label: Preview\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: 3 to update\npulumi preview -s ir-prod1 --refresh=false --diff',
  '#| name: apply-ir-prod1\n#| label: Apply to ir-prod1\n#| danger\n#| after: preview-ir-prod1\n#| review\n#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration\n#| env: AWS_PROFILE=ir-prod1\n#| expect: 3 updated\npulumi up -s ir-prod1 --refresh=false --diff',
]
export const REPLY = `Three steps. Run them in order; the apply arms after the preview matches.\n\n${RUNBOOK.map((b) => `\`\`\`bash\n${b}\n\`\`\`\n`).join('\n')}`
export const spec = (i: number) => cellOf('bash', RUNBOOK[i] as string) as CellSpec
export const chatRef = (ordinal: number, entry = '42'): CellRef => ({ type: 'chat', session: SESSION, entry, ordinal })

export function run(over: Partial<CellRun> = {}): CellRun {
  return {
    id: 'r_01JC7Q2M8V0000000000000000',
    cell: chatRef(2),
    mode: { type: 'cell' },
    pass: null,
    kernel: { scope: { type: 'session', id: SESSION }, spec: { type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' } },
    interpreter: 'bash 5.2 (Homebrew) /opt/homebrew/bin/bash',
    cwd: '~/dr-t6/frp-integration/stacks/sa-frp-integration',
    env_names: ['AWS_PROFILE'],
    presented_sha: 'a',
    code_sha: 'a',
    edited: false,
    pressed_by: 'desktop',
    state: 'done',
    exit: { type: 'code', value: 0 },
    secs: 41,
    expect: [{ item: { kind: 'text', value: '3 to update' }, met: true }],
    summary: '~3 update · 0 replace · 0 delete',
    output_bytes: 1900,
    queued_ms: 1_762_180_000_000,
    started_ms: 1_762_180_000_100,
    ended_ms: 1_762_180_041_100,
    masked: 0,
    private: false,
    name: null,
    input: null,
    ran_past: null,
    ...over,
  }
}

export function state(cell: CellRef, over: Partial<CellState> = {}): CellState {
  return { cell, digest: 'a', problems: [], chips: [], armed: null, superseded_by: null, latest: null, stale: false, member_edit: null, interpreter: null, host: null, disarmed: null, ...over }
}

/** An `Api` whose every method resolves `null`, with `calls` recorded; `over` answers specific methods. */
export function fakeApi(over: Partial<Record<keyof Api, (...a: unknown[]) => unknown>> = {}) {
  const calls: [string, unknown[]][] = []
  const api = new Proxy({} as Api, {
    get: (_t, name: string) =>
      (...args: unknown[]) => {
        calls.push([name, args])
        const f = over[name as keyof Api]
        return Promise.resolve(f ? f(...args) : null)
      },
  })
  return { api, calls }
}

/**
 * Static markup under a query client with `api` as the connection's. Server rendering reads zustand's *initial*
 * state (`getServerSnapshot`), so the connection and cells stores answer their live state for the render.
 */
export function render(node: ReactNode, api: Api = fakeApi().api): string {
  useConnection.setState({ api } as never)
  for (const s of [active().connection, cells]) Object.assign(s, { getInitialState: s.getState })
  return renderToStaticMarkup(h(QueryClientProvider, { client: new QueryClient() }, node))
}
