import { createElement as h, type ReactNode } from 'react'
import { Streamdown } from 'streamdown'
import { expect, test } from 'vitest'
import { FenceContext } from '@/components/chat/surface/JsonRenderFence'
import { classify } from '@/lib/injected'
import { active } from '@/store/engine-scope'
import { useConnection } from '@/store/connection'
import type { Entry } from '@/store/sessions'
import { cellDomId } from './Cell'
import { CELL_LANGUAGES, CellFence, CellOrdinals, cellOrdinals, snippetLines } from './CellFence'
import { plainFences, replyCells, restated } from './fences'
import { cells, refKey, scopeKey } from './store'
import { REPLY, SESSION, chatRef, render, run, state } from './test-fixtures'

const withCells = { renderers: [{ language: CELL_LANGUAGES, component: CellFence }] }
const md = (text: string, plugins?: typeof withCells) => h(Streamdown, { mode: 'static', plugins }, text)
const assistant = (node: ReactNode, o: { session?: string; entry?: string | null; running?: boolean; text?: string; id?: string } = {}) =>
  h(
    FenceContext,
    { value: { running: !!o.running, hasLaterAssistant: false, session: 'session' in o ? o.session : SESSION, entry: o.id ?? 'x' } },
    h(CellOrdinals, { value: cellOrdinals(o.text ?? REPLY, o.entry === undefined ? '42' : o.entry, !!o.running) }, node),
  )

test('a fence without a #| header renders streamdown’s own code block, controls included (171 §4.1)', () => {
  for (const text of ['```bash\necho hi\nls -la\n```\n', '```python\nprint(1)\n```\n', '```sh\n# just a comment\n```\n']) {
    const html = render(assistant(md(text, withCells), { text }))
    expect(html).toContain('data-streamdown="code-block"')
    expect(html).toContain('data-streamdown="code-block-copy-button"')
    expect(html).toContain('data-streamdown="code-block-download-button"')
    expect(html).not.toMatch(/data-cell/)
  }
})

test('the renderer claims exactly the shell and python fence languages', () => {
  expect([...CELL_LANGUAGES].sort()).toEqual(['bash', 'powershell', 'pwsh', 'py', 'python', 'sh', 'shellscript', 'zsh'])
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

test('runnable with a session and a closed, stored reply, whether or not its turn still runs (§4.1, cells-midturn D1)', () => {
  const button = /<button[^>]*>(?:<svg[\s\S]*?<\/svg>)?Preview<\/button>/
  for (const o of [{}, { running: true }]) {
    const live = render(assistant(md(REPLY, withCells), o))
    expect(live).toMatch(button)
    expect(live.match(button)?.[0]).not.toContain('aria-disabled="true"')
    expect(live).toContain('Run all · stops before Apply to ir-prod1')
    expect(live).not.toContain('ready when this message is saved')
  }

  // Not stored yet (the live reply has no stored seq): disabled with the short wait's reason, running or not.
  for (const o of [{ entry: null }, { entry: null, running: true }]) {
    const html = render(assistant(md(REPLY, withCells), o))
    expect(html).toContain('ready when this message is saved')
    expect(html.match(button)?.[0]).toContain('aria-disabled="true"')
  }

  // Archive / search: no session — chips and code, no button.
  const archive = render(assistant(md(REPLY, withCells), { session: undefined }))
  expect(archive).toContain('data-cell')
  expect(archive).toContain('preview-ir-prod1')
  expect(archive).not.toMatch(button)
  expect(archive).not.toContain('ready when this message is saved')
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
  expect(html).toContain('ready when this message is saved')
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

test('220: plain fences are numbered from 0 among closed, runnable, header-less fences; claims go by language and body', () => {
  const text = [
    '```bash\necho a\n```',
    '```bash\n#| cwd: /tmp\necho cell\n```',
    '```js\nconsole.log(1)\n```',
    '```rust\nfn main() {}\n```',
    '```python\nprint(1)\n```',
    '```sh\necho a\n```',
    '```bash\necho a\n```',
    '```bash\necho open',
  ].join('\n\n')
  const o = cellOrdinals(text, '1', false)
  expect(o.plain).toEqual([
    { ordinal: 0, lang: 'bash', body: 'echo a' },
    { ordinal: 1, lang: 'python', body: 'print(1)' },
    { ordinal: 2, lang: 'sh', body: 'echo a' },
    { ordinal: 3, lang: 'bash', body: 'echo a' },
  ])
  expect(o.claimPlain('a', 'bash', 'echo a')).toBe(0)
  expect(o.claimPlain('s', 'sh', 'echo a')).toBe(2)
  // `shellscript` is `bash`: the second identical bash body.
  expect(o.claimPlain('b', 'shellscript', 'echo a')).toBe(3)
  expect(o.claimPlain('a', 'bash', 'echo a')).toBe(0)
  expect(o.claimPlain('c', 'bash', 'echo open')).toBeNull()
  expect(o.claimPlain('d', 'rust', 'fn main() {}')).toBeNull()
})

test('220: the plain-fence scanner is strict CommonMark, as the engine counts', () => {
  const bodies = (text: string) => plainFences(text).map((p) => `${p.lang}:${p.body}`)
  expect(bodies('~~~bash\necho tilde\n~~~')).toEqual(['bash:echo tilde'])
  expect(bodies('~~~bash\necho x\n```\n~~~')).toEqual(['bash:echo x\n```'])
  expect(bodies('    ```bash\n    echo indented\n    ```')).toEqual([])
  expect(bodies('   ```bash\n   echo three\n   ```')).toEqual(['bash:echo three'])
  expect(bodies('\t```bash\necho tab\n```')).toEqual([])
  expect(bodies('> ```bash\n> echo quoted\n> ```')).toEqual([])
  expect(bodies('```bash title=x\necho titled\n```')).toEqual(['bash:echo titled'])
  expect(bodies('```BASH\necho upper\n```')).toEqual(['bash:echo upper'])
  expect(bodies('```shellscript\necho blocknote\n```')).toEqual(['bash:echo blocknote'])
  expect(bodies('````bash\necho x\n```\n````')).toEqual(['bash:echo x\n```'])
  expect(bodies('```bash\necho done\n```\n\n```bash\necho open')).toEqual(['bash:echo done'])
  expect(plainFences('```bash\necho twin\n```\n\n```bash\necho twin\n```')).toEqual([
    { ordinal: 0, lang: 'bash', body: 'echo twin' },
    { ordinal: 1, lang: 'bash', body: 'echo twin' },
  ])
})

test('220: a snippet’s per-line copy maps line N to streamdown’s Nth rendered line, blank lines included', () => {
  const code = '\necho a\n\n  echo <b> && x\n\n'
  const html = render(md(`\`\`\`bash\n${code}\`\`\`\n`, withCells))
  const inner = /<code[^>]*>([\s\S]*?)<\/code>/.exec(html)?.[1] ?? ''
  const decode = (s: string) => s.replace(/<[^>]*>/g, '').replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&').replace(/^\n$/, '')
  const rendered = inner.split('<span class="block').slice(1).map((chunk) => decode(`<span class="block${chunk}`))
  expect(snippetLines(code)).toEqual(rendered)
  expect(snippetLines(code)).toEqual(['', 'echo a', '', '  echo <b> && x'])
  // The keyboard route: the button is always in the tab order, named for the first non-blank line.
  expect(html).toContain('aria-label="Copy line 2"')
})

test('220: Make runnable shows only on plain runnable fences of a stored, settled reply in a live session', () => {
  const text = 'Run this yourself:\n\n```bash\nsudo systemctl restart x\n```\n\n```bash\n#| cwd: /tmp\n#| label: Go\necho go\n```\n\n```python\nprint(1)\n```\n'
  const marks = (html: string) => [...html.matchAll(/data-make-runnable="(\d+)"/g)].map((m) => m[1])
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  try {
    const html = render(assistant(md(text, withCells), { text }))
    expect(marks(html)).toEqual(['0', '1'])
    expect(html).toContain('Make runnable')
    // Not while the turn runs, before the reply is stored, without a session, read-only, or outside an assistant reply.
    expect(marks(render(assistant(md(text, withCells), { text, running: true })))).toEqual([])
    expect(marks(render(assistant(md(text, withCells), { text, entry: null })))).toEqual([])
    expect(marks(render(assistant(md(text, withCells), { text, session: undefined })))).toEqual([])
    const readOnly = h(
      FenceContext,
      { value: { running: false, hasLaterAssistant: false, session: SESSION, entry: 'x', readOnly: true } },
      h(CellOrdinals, { value: cellOrdinals(text, '42', false) }, md(text, withCells)),
    )
    expect(marks(render(readOnly))).toEqual([])
    expect(marks(render(md(text, withCells)))).toEqual([])
  } finally {
    useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'connecting' } })
  }
  // An engine without `cell_promote` never offers it.
  expect(marks(render(assistant(md(text, withCells), { text })))).toEqual([])
})

test('the host chip (§14.2, L11 `CellState.host`) shows only when the record names another engine', () => {
  expect(render(assistant(md(REPLY, withCells)))).not.toContain('on Sky’s Mac mini')
  const ref = chatRef(2)
  cells.setState({ scopes: { [scopeKey({ type: 'session', id: SESSION })]: { states: { [refKey(ref)]: { ...state(ref), host: 'Sky’s Mac mini' } as never }, kernels: [], error: null, notices: {} } } })
  expect(render(assistant(md(REPLY, withCells)))).toContain('on Sky’s Mac mini')
  cells.setState({ scopes: {} })
})

test('CELLAFTER: a disarmed chat cell stays runnable — its own label, enabled, with the engine’s reason', () => {
  const ref = chatRef(3)
  const why = 'after: preview-ir-prod1 — run it first'
  cells.setState({ scopes: { [scopeKey({ type: 'session', id: SESSION })]: { states: { [refKey(ref)]: state(ref, { disarmed: why }) as never }, kernels: [], error: null, notices: {} } } })
  const html = render(assistant(md(REPLY, withCells)))
  cells.setState({ scopes: {} })
  const b = html.match(new RegExp(`<button[^>]*aria-description="${why}"[^>]*>(?:(?!</button>)[\\s\\S])*</button>`))?.[0]
  expect(b).toBeDefined()
  expect(b).toContain('Apply to ir-prod1</button>')
  expect(b).not.toContain('aria-disabled="true"')
  expect(html).not.toMatch(/>Run anyway</)
  expect(html).not.toContain('>after: Preview<')
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

// Item (c): reply 42 (entry `a1`) presented the §3.8 runbook; reply 99 (entry `a2`) presents `preview-ir-prod1` again,
// changed, beside two new cells.
const PREVIEW_V2 = '#| name: preview-ir-prod1\n#| label: Preview v2\n#| cwd: /tmp\npulumi preview -s ir-prod1 --diff'
const LATER = ['Updated preview, then two checks.', PREVIEW_V2, '#| name: verify\n#| label: Verify\necho ok', '#| label: Cleanup\nrm -f /tmp/x']
  .map((b, i) => (i === 0 ? b : `\`\`\`bash\n${b}\n\`\`\``))
  .join('\n\n')
const reply = (id: string, seq: number, text: string) => ({ id, kind: 'assistant', text, open: false, context: { target: { kind: 'message', seq } } }) as unknown as Entry
const TRANSCRIPT = [reply('a1', 42, REPLY), { id: 'u2', kind: 'user', text: 'change the preview' } as Entry, reply('a2', 99, LATER)]
const replaced = chatRef(2)
const newest = chatRef(1, '99')

function withTranscript<T>(f: () => T): T {
  const store = active().sessions
  const before = store.getState()
  store.setState({ sessions: { ...before.sessions, [SESSION]: { ...before.sessions[SESSION], entries: TRANSCRIPT } as never } })
  Object.assign(store, { getInitialState: store.getState })
  cells.setState({
    scopes: {
      [scopeKey({ type: 'session', id: SESSION })]: {
        states: { [refKey(replaced)]: state(replaced, { superseded_by: newest }), [refKey(newest)]: state(newest, { digest: 'd99', latest: run({ cell: newest }) }) } as never,
        kernels: [],
        error: null,
        notices: {},
      },
    },
  })
  try {
    return f()
  } finally {
    store.setState(before, true)
    cells.setState({ scopes: {} })
  }
}

test('item (c): a later reply restates the first cell of each name an earlier loaded reply presented', () => {
  const later = replyCells(LATER)
  expect(restated(TRANSCRIPT, 'a2', later)).toEqual([[1, 'a1']])
  // The first reply restates nothing; an unknown reply (not loaded) or an earlier one missing renders in full.
  expect(restated(TRANSCRIPT, 'a1', replyCells(REPLY))).toEqual([])
  expect(restated(TRANSCRIPT, 'zz', later)).toEqual([])
  expect(restated(TRANSCRIPT.slice(1), 'a2', later)).toEqual([])
})

test('item (c): the original position shows the newest version — its ref (DOM id), spec and run — never "replaced"', () => {
  const html = withTranscript(() => render(assistant(md(REPLY, withCells), { id: 'a1' })))
  expect(html).toContain(`id="${cellDomId(newest)}"`)
  expect(html).not.toContain(`id="${cellDomId(replaced)}"`)
  expect(html).toContain('Preview v2')
  expect(html).not.toContain('Replaced by a newer version')
  // Its slot in the reply's order is kept; the runbook counts the replacement's run (1 of 3 done).
  expect(html.indexOf('Preview v2')).toBeLessThan(html.indexOf('data-cell-ordinal="3"'))
  expect(html).toMatch(/>1(<!-- -->)?\/(<!-- -->)?3</)
})

test('item (c): the later reply shows a one-line stub for the restated cell; its bar has one step per new cell', () => {
  const html = withTranscript(() => render(assistant(md(LATER, withCells), { id: 'a2', entry: '99', text: LATER })))
  expect(html).toContain('↑ updated cell <code class="font-mono">preview-ir-prod1</code> above')
  expect(html).not.toContain('data-cell-name="preview-ir-prod1"')
  expect(html).not.toContain('Preview v2')
  expect(html.match(/data-cell=""/g)).toHaveLength(2)
  expect(html.match(/role="toolbar"/g)).toHaveLength(1)
  expect(html).toMatch(/>0(<!-- -->)?\/(<!-- -->)?2</)
  // With only one cell of its own left, the later reply has no bar.
  const two = LATER.slice(0, LATER.lastIndexOf('```bash'))
  const lone = withTranscript(() => render(assistant(md(two, withCells), { id: 'a2', entry: '99', text: two })))
  expect(lone).toContain('↑ updated cell')
  expect(lone).not.toContain('role="toolbar"')
})
