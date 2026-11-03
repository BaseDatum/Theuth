import { createElement as h } from 'react'
import { afterEach, expect, test } from 'vitest'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { KernelStrip } from './KernelStrip'
import { cells, scopeKey } from './store'
import { SESSION, render } from './test-fixtures'

const scope = { type: 'session', id: SESSION } as const
const kernel = (kind: 'bash' | 'zsh', phase: KernelState['phase'], cwd: string | null = null): KernelState => ({
  id: { scope, spec: { type: 'shell', kind, path: `/bin/${kind}` } },
  phase,
  label: kind === 'bash' ? 'bash 5.2 (Homebrew)' : 'zsh 5.9',
  cwd,
})
const strip = (kernels: KernelState[]) => {
  cells.setState({ scopes: { [scopeKey(scope)]: { states: {}, kernels, error: null, notices: {} } } })
  return render(h(KernelStrip, { session: SESSION }))
}
afterEach(() => cells.setState({ scopes: {} }))

test('no live kernel, no strip (§11.4)', () => {
  expect(strip([])).toBe('')
  expect(strip([kernel('bash', 'closed'), kernel('zsh', 'dead')])).toBe('')
})

test('one kernel: its label, phase, live cwd and Restart / Close', () => {
  const html = strip([kernel('bash', 'ready', '/Users/sky/dr-t6/sa-frp-integration')])
  expect(html).toContain('bash 5.2 (Homebrew) · idle')
  expect(html).toContain('/Users/sky/dr-t6/sa-frp-integration')
  expect(html).toContain('>Restart<')
  expect(html).toContain('>Close<')
})

test('two or more kernels collapse to "N kernels ▾"', () => {
  const html = strip([kernel('bash', 'busy'), kernel('zsh', 'ready')])
  expect(html).toContain('2 kernels')
  expect(html).toContain('aria-expanded="false"')
  expect(html).not.toContain('>Restart<')
})
