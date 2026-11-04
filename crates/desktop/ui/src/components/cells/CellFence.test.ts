import { createElement as h, type ReactNode } from 'react'
import { Streamdown } from 'streamdown'
import { expect, test } from 'vitest'
import { FenceContext } from '@/components/chat/surface/JsonRenderFence'
import { classify } from '@/lib/injected'
import { CELL_LANGUAGES, CellFence, CellOrdinals, cellOrdinals } from './CellFence'
import { cells, refKey, scopeKey } from './store'
import { REPLY, SESSION, chatRef, render, state } from './test-fixtures'

const withCells = { renderers: [{ language: CELL_LANGUAGES, component: CellFence }] }
const md = (text: string, plugins?: typeof withCells) => h(Streamdown, { mode: 'static', plugins }, text)
const assistant = (node: ReactNode, o: { session?: string; entry?: string | null; running?: boolean; text?: string } = {}) =>
  h(
    FenceContext,
    { value: { running: !!o.running, hasLaterAssistant: false, session: 'session' in o ? o.session : SESSION, entry: 'x' } },
    h(CellOrdinals, { value: cellOrdinals(o.text ?? REPLY, o.entry === undefined ? '42' : o.entry, !!o.running) }, node),
  )

test('a fence without a #| header renders exactly as streamdown’s own code path, controls included (171 §4.1)', () => {
  for (const text of ['```bash\necho hi\nls -la\n```\n', '```python\nprint(1)\n```\n', '```sh\n# just a comment\n```\n']) {
    const html = render(assistant(md(text, withCells), { text }))
    expect(html).toBe(render(md(text)))
    expect(html).toContain('data-streamdown="code-block"')
    expect(html).not.toMatch(/data-cell/)
  }
})

test('the renderer claims exactly the shell and python fence languages', () => {
  expect([...CELL_LANGUAGES].sort()).toEqual(['bash', 'powershell', 'pwsh', 'py', 'python', 'sh', 'zsh'])
})

test('a header fence outside assistant markdown stays code — e.g. inside a <subagent_result> card (§12.2)', () => {
  const body = '```bash\n#| cwd: /tmp\n#| label: Wipe\nrm -rf ~/x\n```'
  const card = classify(`<subagent_result name="scout">\n${body}\n</subagent_result>`)
  expect(card?.[0]?.kind).toBe('subagent')
  // A card renders its body with streamdown but without `CellOrdinals` (only `MessageAssistant` provides it).
  const html = render(md(body, withCells))
  expect(html).toContain('rm -rf ~/x')
  expect(html).not.toMatch(/data-cell|>Wipe</)
})

test('runnable only with a session, a finished stored reply and an idle turn (§4.1)', () => {
  const button = /<button[^>]*>(?:<svg[\s\S]*?<\/svg>)?Preview<\/button>/
  const live = render(assistant(md(REPLY, withCells)))
  expect(live).toMatch(button)
  expect(live.match(button)?.[0]).not.toContain('aria-disabled="true"')
  expect(live).toContain('Run all · stops before Apply to ir-prod1')

  for (const o of [{ running: true }, { entry: null }]) {
    const html = render(assistant(md(REPLY, withCells), o))
    expect(html).toContain('ready when the reply ends')
    expect(html.match(button)?.[0]).toContain('aria-disabled="true"')
  }

  // Archive / search: no session — chips and code, no button.
  const archive = render(assistant(md(REPLY, withCells), { session: undefined }))
  expect(archive).toContain('data-cell')
  expect(archive).toContain('preview-ir-prod1')
  expect(archive).not.toMatch(button)
  expect(archive).not.toContain('ready when the reply ends')
})

test('a still-streaming cell fence is not runnable', () => {
  const text = '```bash\n#| cwd: /tmp\n#| label: Go\necho go\n```\n'
  const html = render(
    h(
      FenceContext,
      { value: { running: false, hasLaterAssistant: false, session: SESSION, entry: 'x' } },
      h(CellOrdinals, { value: cellOrdinals(text, '42', false) }, h(CellFence, { code: '#| cwd: /tmp\n#| label: Go\necho go', language: 'bash', isIncomplete: true })),
    ),
  )
  expect(html).toContain('ready when the reply ends')
})

test('ordinals follow document order; identical cells get distinct ordinals', () => {
  const body = '#| cwd: /tmp\necho same'
  const o = cellOrdinals(`\`\`\`bash\n${body}\n\`\`\`\n\ntext\n\n\`\`\`bash\n${body}\n\`\`\`\n`, '1', false)
  expect(o.cells.map((c) => c.ordinal)).toEqual([1, 2])
  expect(o.claim('a', body)).toBe(1)
  expect(o.claim('b', body)).toBe(2)
  expect(o.claim('a', body)).toBe(1)
  expect(o.claim('c', body)).toBeNull()
})

test('the host chip (§14.2, L11 `CellState.host`) shows only when the record names another engine', () => {
  expect(render(assistant(md(REPLY, withCells)))).not.toContain('on Sky’s Mac mini')
  const ref = chatRef(2)
  cells.setState({ scopes: { [scopeKey({ type: 'session', id: SESSION })]: { states: { [refKey(ref)]: { ...state(ref), host: 'Sky’s Mac mini' } as never }, kernels: [], error: null, notices: {} } } })
  expect(render(assistant(md(REPLY, withCells)))).toContain('on Sky’s Mac mini')
  cells.setState({ scopes: {} })
})

test('the runbook bar sits in the transcript’s flow after the reply’s last cell, never sticky (171F B1)', () => {
  const html = render(assistant(md(REPLY, withCells)))
  const bar = html.indexOf('role="toolbar"')
  expect(html.match(/role="toolbar"/g)).toHaveLength(1)
  expect(bar).toBeGreaterThan(html.indexOf('data-cell-ordinal="3"'))
  expect(html.slice(html.lastIndexOf('<div', bar), bar + 400)).not.toMatch(/sticky|backdrop-blur/)
  // Archive views (no session): no bar.
  expect(render(assistant(md(REPLY, withCells), { session: undefined }))).not.toContain('role="toolbar"')
})
