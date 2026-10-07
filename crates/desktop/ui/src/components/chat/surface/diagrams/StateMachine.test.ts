import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { fixedLayout } from '@/lib/graph-layout'
import { dropDangling, type StateMachineData, StateMachineSvg, stateMachineText, structuralHash, toElk, transitionText } from './StateMachine'

const RUST = new URL('../../../../../../../surface/', import.meta.url)
const TOKENS: DiagramTokens = {
  background: '#fff',
  foreground: '#111',
  primary: '#111',
  muted: '#ddd',
  mutedForeground: '#666',
  border: '#ccc',
  card: '#fafafa',
  accent: '#b4531e',
  success: '#2f5d4a',
  danger: '#9b2d1f',
  font: 'sans-serif',
  series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}
const fixture = (name: string): StateMachineData => {
  const j = JSON.parse(readFileSync(new URL(`fixtures/json-render/${name}.json`, RUST), 'utf8'))
  return Object.values(j.elements as Record<string, { type: string; props: StateMachineData }>).find((e) => e.type === 'StateMachine')!.props
}
const render = (p: StateMachineData) => renderToStaticMarkup(createElement(StateMachineSvg, { props: p, layout: fixedLayout(toElk(p)), tokens: TOKENS, markerId: 'sm' }))

test('valid-state-machine at a fixed layout matches its snapshot', () => {
  expect(render(fixture('valid-state-machine'))).toMatchSnapshot()
})

test('kinds draw as dot / double circle / diamond, guards join the label mask, states and transitions are units', () => {
  const p = fixture('valid-state-machine')
  const html = render(p)
  expect(html.match(/data-unit="/g)?.length).toBe(p.states.length + p.transitions.length)
  expect(html).toMatch(/data-kind="initial"[^>]*>.*?<circle [^>]*r="8" fill="#111"/)
  expect(html).toMatch(/data-kind="final"[^>]*>.*?<circle [^>]*r="10"[^>]*><\/circle><circle [^>]*r="6" fill="#111"/)
  expect(html).toMatch(/data-kind="choice"[^>]*>.*?<polygon /)
  expect(html).toContain('<title>wait [dependency open]</title>')
  expect(html).toContain('>wait [depende…</text>')
  expect(html).toContain('stroke-dasharray="6 4"')
  expect(html).toContain(`stroke="${TOKENS.danger}"`)
  expect(html).toContain('marker-end="url(#sm-danger)"')
  expect(html).toMatch(/<title id="sm-title">Task lifecycle<\/title><desc id="sm-desc">StateMachine: Task lifecycle — 6 states<\/desc>/)
  expect(html).not.toContain('foreignObject')
})

test('a self-loop lays out and draws (ELK layered allows them); streaming drops dangling transitions', () => {
  const p = fixture('valid-state-machine-self-loop')
  const html = render(p)
  expect(html.match(/data-unit="edge:1"/g)?.length).toBe(1)
  const open = { ...p, transitions: [...p.transitions, { from: 'poll', to: 'ghost' }] }
  expect(dropDangling(open).transitions).toHaveLength(2)
  expect(structuralHash(dropDangling(open))).toBe(structuralHash(p))
  expect(structuralHash({ ...p, transitions: [{ from: 'idle', to: 'poll', label: 'go' }] })).not.toBe(structuralHash(p))
})

test('the text twin equals the Rust lowering (w100 golden)', () => {
  expect(transitionText({ label: 'a', guard: 'b' })).toBe('a [b]')
  expect(transitionText({ guard: 'b' })).toBe('[b]')
  const golden = readFileSync(new URL('tests/golden/valid-state-machine-w100.txt', RUST), 'utf8')
  expect(`${stateMachineText(fixture('valid-state-machine'))}\n`).toBe(golden)
})
