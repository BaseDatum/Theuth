import { receiveComponent } from '@/rpc/bindings/receive'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ObservedMeta } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import { StripDot } from './OrgChart'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: unknown }) => createElement('a', null, children as never),
  useNavigate: () => () => undefined,
}))

const row = (over: Partial<AgentInfo> = {}): AgentInfo => (receiveComponent("method.agent.output.AgentInfo", {
  id: 's1',
  name: 'NapA',
  parent: 'harness',
  depth: 2,
  agent: null,
  model: 'anthropic/claude-fable-5-1',
  state: { state: 'running' },
  started_at: new Date(Date.now() - 22_000).toISOString(),
  finished_at: null,
  last: '',
  kind: 'chat',
  ...over,
}))
const observed = (state: ObservedMeta['state']): ObservedMeta => (receiveComponent("method.agent.output.ObservedMeta", {
  source: 'omp_task',
  external: 'x',
  state,
  can_cancel: false,
  cost_rule: 'counted',
  tool_call: 'toolu_1',
  detail: null,
}))
const dot = (r: AgentInfo) => renderToStaticMarkup(createElement(StripDot, { row: r, leaving: false }))

test('a native strip dot is the malachite/warning circle titled by its state', () => {
  const html = dot(row({ beat: 3 }))
  expect(html).toContain('title="NapA · running · anthropic/claude-fable-5-1 · 22s"')
  expect(html).toContain('bg-success')
  expect(html).toContain('subagent-blink')
  expect(html).not.toContain('watching')
  expect(dot(row({ state: { state: 'approval', tool: 'bash' } }))).toContain('title="NapA · approval: bash · anthropic/claude-fable-5-1 · 22s"')
})

test('an observed strip dot is the WatchDot in the harness state’s colour, titled `watching` like the composer strip', () => {
  const running = dot(row({ kind: 'observed', observed: observed('running'), beat: 3 }))
  expect(running).toContain('title="NapA · watching · running · anthropic/claude-fable-5-1 · 22s"')
  expect(running).toContain('bg-success')
  expect(running).toContain('subagent-blink')
  expect(running).toContain('<span class="sr-only">running</span>')
  const action = dot(row({ kind: 'observed', observed: observed('requires_action') }))
  expect(action).toContain('watching · waiting on you')
  expect(action).toContain('bg-warning')
  const failed = dot(row({ kind: 'observed', observed: observed('failed') }))
  expect(failed).toContain('watching · failed')
  expect(failed).toContain('bg-danger')
  const lost = dot(row({ kind: 'observed', observed: observed('unknown') }))
  expect(lost).toContain('watching · lost contact')
  expect(lost).toContain('border-dashed')
  expect(lost).not.toContain('bg-success')
  const done = dot(row({ kind: 'observed', observed: observed('completed') }))
  expect(done).toContain('watching · finished')
  expect(done).toContain('✓')
})
