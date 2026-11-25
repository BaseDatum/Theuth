// 216 §10.1: the install card's lines — one row each, a widen card's new lines marked, the risk beside every line;
// a verified grant read back as lines in the same form.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { GrantLine, Grants } from '@/rpc/bindings/method.plugins.output'
import { GrantLines, grantedLines } from './GrantLines'

const lines: GrantLine[] = [
  { key: 'run', text: 'Runs python3 main.py from ~/.theuth/plugins/turns as you, with your files and network', risk: 'destructive', new: false },
  { key: 'subscribe', text: 'Reads agent.events: TurnFinished — a turn finished', risk: 'read_only', new: false },
  { key: 'tool', text: 'Offers the model 1 tool: turns_reset (mutating)', risk: 'mutating', new: true },
]

test('one row per line, the new line marked, every line with its risk word', () => {
  const html = renderToStaticMarkup(createElement(GrantLines, { lines }))
  const rows = html.match(/<li[^>]*>.*?<\/li>/g) ?? []
  expect(rows).toHaveLength(3)
  expect(rows[0]).toContain('Runs python3 main.py')
  expect(rows[0]).toContain('>destructive<')
  expect(rows[1]).toContain('>read-only<')
  expect(rows[2]).toContain('>mutating<')
  expect(rows.map((r) => r.includes('data-new="true"'))).toEqual([false, false, true])
  expect(rows[2]).toContain('>new<')
  expect(rows[0]).not.toContain('>new<')
})

test('a grant reads back as lines: run first, each power with its risk, input destructive', () => {
  const g: Grants = {
    subscribe: ['agent.events#TurnFinished'],
    emits: ['turns.limit_reached'],
    placements: ['turns'],
    tools: [{ name: 'turns_today', risk: 'read_only' }],
    capabilities: { notify: true, input: true, call: { tools: ['knowledge_search'], max_risk: 'read_only' } },
    secrets: ['github.pat'],
    run: ['python3', 'main.py'],
    cwd: '.',
    env: {},
  }
  expect(grantedLines(g).map((l) => [l.key, l.risk])).toEqual([
    ['run', 'destructive'],
    ['secret', 'destructive'],
    ['subscribe', 'read_only'],
    ['emit', 'read_only'],
    ['placement', 'read_only'],
    ['tool', 'read_only'],
    ['call', 'read_only'],
    ['input', 'destructive'],
    ['notify', 'read_only'],
  ])
  expect(grantedLines(g)[0]?.text).toBe('Runs python3 main.py from .')
  expect(grantedLines(g).every((l) => !l.new)).toBe(true)
})
