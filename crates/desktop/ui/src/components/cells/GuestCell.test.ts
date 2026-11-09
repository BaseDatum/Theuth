import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { CellDefaults, defaultsLine } from './CellDefaults'
import { cellOf, parseDefaults } from './grammar'
import { GuestCell, latestLine } from './GuestCell'
import { noteRef } from './note-edit'
import { render } from './test-fixtures'

const DOC = 'cccccccccccccccccccccccccc'
const TEXT = '#| name: preview-ir-prod1\n#| label: Preview\npulumi preview -s ir-prod1 --diff'
const spec = cellOf('bash', TEXT) as CellSpec
const row: CellLatest = {
  id: `${DOC}:preview-ir-prod1`,
  doc: DOC,
  cell: 'preview-ir-prod1',
  run: 'r_01JC7Q2M8V0000000000000000',
  state: 'done',
  exit: 0,
  secs: 33,
  expect: 'expect ✓',
  summary: '~3 update · 0 replace · 0 delete',
  code_sha: 'a',
  ended_ms: BigInt(new Date(2026, 10, 3, 14, 31).getTime()),
  text: 'Resources:\n    ~ 3 to update\n    4 unchanged\n',
  engine: '',
}
const guest = (latest: CellLatest | null) => render(h(GuestCell, { cell: spec, code: TEXT, latest, cellRef: noteRef(DOC, spec, 'blk'), editor: h('pre', null, TEXT) }))

test('a guest sees the code, the latest replicated output and "latest run · owner · 14:31" — and never a Run button', () => {
  const html = guest(row)
  expect(html).toContain(`<pre>${TEXT}</pre>`)
  expect(html).toContain('~ 3 to update')
  expect(latestLine(row)).toBe('exit 0 · 33 s · expect ✓ · latest run · owner · 14:31')
  expect(html).toContain('latest run · owner · 14:31')
  expect(html).toContain('Only the owner can run cells')
  expect(html).not.toMatch(/<button[^>]*>(?:<svg[\s\S]*?<\/svg>)?(Preview|Run|Stop|Ask theuth)<\/button>/)
  expect(html).not.toContain('history')
})

test('a private cell replicates no text; no run yet says so', () => {
  expect(guest({ ...row, text: '' })).toContain('output kept private')
  expect(guest(null)).toContain('no run yet')
})

test('the cells defaults block: one muted line, a second block shows the engine warning', () => {
  const d = parseDefaults('#| cwd: ~/dr-t6/frp-integration\n#| shell: /opt/homebrew/bin/bash\n#| python: .venv\n')
  const brew = { kind: 'bash' as const, source: 'homebrew', path: '/opt/homebrew/bin/bash', version: '5.2.37', why: '', auto: true }
  expect(defaultsLine(d, brew)).toBe('Cell defaults · ~/dr-t6/frp-integration · bash 5.2 (Homebrew) · .venv')
  expect(defaultsLine(d, undefined)).toBe('Cell defaults · ~/dr-t6/frp-integration · bash · .venv')
  expect(render(h(CellDefaults, { text: '#| cwd: ~/x', second: true }))).toContain('a second `cells` block is ignored')
})
