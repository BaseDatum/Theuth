import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { digest } from './grammar'
import { NoteKernelPill, pillText, runAllNote } from './NoteKernelPill'
import { cells, scopeKey } from './store'
import { fakeApi, render } from './test-fixtures'

const DOC = 'bbbbbbbbbbbbbbbbbbbbbbbbbb'
const scope = { type: 'doc', id: DOC } as const
const kernel = (spec: KernelState['id']['spec'], label: string, phase: KernelState['phase']): KernelState => ({ id: { scope, spec }, phase, label, cwd: null })
const python = kernel({ type: 'python', path: '/Users/sky/Develop/costs/.venv/bin/python' }, '.venv 3.12', 'ready')
const bash = kernel({ type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' }, 'bash 5.2 (Homebrew)', 'busy')

function pill(kernels: KernelState[], states = 0) {
  const st = Object.fromEntries(Array.from({ length: states }, (_, i) => [`note:${DOC}/block:b${i}`, {} as never]))
  cells.setState({ scopes: { [scopeKey(scope)]: { states: st, kernels, error: null, notices: {} } } })
  return render(h(NoteKernelPill, { doc: DOC }))
}

test('one pill per live kernel in the strip words, each with Run all; dead kernels and an empty note show nothing', () => {
  expect(pillText(python)).toBe('python · .venv 3.12 · idle')
  expect(pillText(bash)).toBe('bash · bash 5.2 (Homebrew) · busy')
  const html = pill([python, bash, kernel({ type: 'monty' }, 'Monty', 'closed')])
  expect(html).toContain('python · .venv 3.12 · idle')
  expect(html).toContain('bash · bash 5.2 (Homebrew) · busy')
  expect(html).not.toContain('Monty')
  expect(html.match(/Run all<\/button>/g)).toHaveLength(2)
  expect(pill([])).toBe('')
  // Cells on the page but no kernel yet: one quiet Run all.
  const quiet = pill([], 2)
  expect(quiet).toContain('cells')
  expect(quiet.match(/Run all<\/button>/g)).toHaveLength(1)
})

test('Run all is one engine pass: every cell top to bottom with its digest, a p_ pass id', async () => {
  const { api, calls } = fakeApi()
  const a: CellRef = { type: 'note', doc: DOC, key: { kind: 'name', value: 'build' } }
  const b: CellRef = { type: 'note', doc: DOC, key: { kind: 'block', value: 'blk9' } }
  await runAllNote(api, scope, [
    { ref: a, text: '#| name: build\nmake' },
    { ref: b, text: '#| label: Test\nmake test' },
  ])
  expect(calls).toHaveLength(1)
  const [method, [s, pairs, pass]] = calls[0] as [string, unknown[]]
  expect(method).toBe('cell_run_all')
  expect(s).toEqual(scope)
  expect(pairs).toEqual([
    [a, await digest('#| name: build\nmake')],
    [b, await digest('#| label: Test\nmake test')],
  ])
  expect(pass).toMatch(/^p_[0-9A-HJKMNP-TV-Z]{26}$/)
})
