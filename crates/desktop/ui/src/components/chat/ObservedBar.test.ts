import { receiveComponent } from '@/rpc/bindings/receive'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ObservedMeta } from "@/rpc/bindings/method.agent.output"
import { ObservedBarView, WatchDot, observedNote } from './ObservedBar'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, params }: { children: unknown; params: { session: string } }) => createElement('a', { href: `/c/${params.session}` }, children as never),
}))

const meta = (over: Partial<ObservedMeta> = {}): ObservedMeta => (receiveComponent("method.agent.output.ObservedMeta", {
  source: 'omp_task',
  external: 'ReadA',
  state: 'running',
  can_cancel: false,
  cost_rule: 'counted',
  detail: null,
  ...over,
}))
const bar = (observed: ObservedMeta, harness = 'omp') => renderToStaticMarkup(createElement(ObservedBarView, { observed, harness, parent: 'harness-sess', onStop: () => {} }))

test('Stop subagent is enabled only when the harness advertises per-child cancel; Open parent links to the parent', () => {
  const omp = bar(meta())
  expect(omp).toContain('href="/c/harness-sess"')
  expect(omp).toMatch(/<button[^>]*disabled[^>]*>Stop subagent/)
  expect(omp).toMatch(/aria-disabled="true"[^>]*title="omp cannot stop one subagent — stop the parent"/)
  const rfd = bar(meta({ source: 'rfd', can_cancel: true, cost_rule: 'display_only' }), 'claude')
  expect(rfd).toMatch(/<button[^>]*aria-disabled="false"[^>]*>Stop subagent/)
  expect(rfd).not.toMatch(/<button[^>]*disabled=""[^>]*>Stop subagent/)
  expect(rfd).toContain('role="status"')
})

test('the note names the harness and adds the one caveat a source or state carries', () => {
  expect(observedNote(meta(), 'omp')).toBe('omp runs this subagent, so you can watch it but not message it.')
  expect(observedNote(meta({ source: 'opencode_task', cost_rule: 'unreported' }), 'opencode')).toMatch(/opencode 1\.x does not stream subagent activity\.$/)
  expect(observedNote(meta({ state: 'unknown', stop: 'lost contact' }), 'omp')).toMatch(/theuth lost the harness; the last known state is shown\.$/)
  expect(observedNote(meta({ source: 'rfd', state: 'idle' }), 'claude')).toMatch(/idle — claude may resume it\.$/)
  // The bar leads with the state word, never "observed".
  const html = bar(meta({ state: 'requires_action' }))
  expect(html).toContain('waiting on you')
  expect(html).not.toMatch(/observed/i)
})

test('WatchDot: a check only when completed; live states blink on a beat, resting ones never do', () => {
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'completed' }))).toContain('✓')
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'idle' }))).not.toContain('✓')
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'running', beat: 5 }))).toContain('subagent-blink')
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'idle', beat: 5 }))).not.toContain('subagent-blink')
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'unknown' }))).toContain('border-dashed')
  expect(renderToStaticMarkup(createElement(WatchDot, { state: 'unknown' }))).toContain('lost contact')
})
