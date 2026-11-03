import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import type { CellState } from '@/rpc/bindings/CellState'
import { Cell, CellExtras } from './Cell'
import { keyRun } from './CellEditor'
import { chatRef, render, run, spec, state } from './test-fixtures'
import type { CellViewState } from './types'
import { viewOf } from './view'
import type { CellRunWire } from './wire'

const noop = () => undefined
const callbacks = { onRun: noop, onStop: noop, onInput: noop, onAsk: noop, onEdit: noop, onAttach: () => noop }
const ref = chatRef(2)

function cell(i: number, s: CellState | null, o: { edited?: boolean; extras?: CellExtras; guest?: boolean } = {}): { view: CellViewState; html: string } {
  const c = spec(i)
  const view = viewOf(s, c, !!o.edited, o.guest)
  const html = render(
    h(CellExtras, { value: o.extras ?? {} }, h(Cell, { cellRef: ref, cell: c, code: c.code, runnable: true, view, state: s, latest: s?.latest ?? null, host: null, callbacks, editor: h('pre', null, c.code) })),
  )
  return { view, html }
}
/** The enabled / disabled button whose text (and accessible name) is `text`. */
const button = (html: string, text: string) => html.match(new RegExp(`<button[^>]*>(?:<svg[\\s\\S]*?</svg>)?${text}</button>`))?.[0]

test('every §11.2 state renders its look and button', () => {
  const cases: [CellViewState, { view: CellViewState; html: string }, (html: string) => void][] = [
    ['ready', cell(1, state(ref)), (x) => expect(button(x, 'Preview')).not.toContain('aria-disabled="true"')],
    [
      'queued',
      cell(1, state(ref, { latest: run({ state: 'queued', exit: null, secs: null, ended_ms: null, started_ms: null }) }), { extras: { queuePos: 2 } }),
      (x) => {
        expect(x).toContain('queued · 2nd')
        expect(button(x, 'Stop')).toBeDefined()
      },
    ],
    [
      'running',
      cell(1, state(ref, { latest: run({ state: 'running', exit: null, secs: null, ended_ms: null }) })),
      (x) => {
        expect(x).toMatch(/running \d/)
        expect(button(x, 'Stop')).toBeDefined()
        expect(button(x, 'Kill')).toContain('disabled')
      },
    ],
    ['done', cell(1, state(ref, { latest: run() })), (x) => expect(x).toMatch(/exit 0 · 41 s · expect ✓.*~3 update/)],
    [
      'failed',
      cell(1, state(ref, { latest: run({ state: 'failed', exit: { type: 'code', value: 1 }, expect: [{ item: { kind: 'text', value: '3 to update' }, met: false }] }) })),
      (x) => {
        expect(x).toContain('✗ exit 1')
        expect(x).toContain('expect ✗ 1/1 missed')
        expect(button(x, 'Preview')).toBeDefined()
      },
    ],
    ['stopped', cell(1, state(ref, { latest: run({ state: 'stopped', exit: { type: 'signal', value: 'SIGINT' } }) })), (x) => expect(x).toContain('stopped by you')],
    [
      'lost',
      cell(1, state(ref, { latest: run({ state: 'lost', exit: null }) })),
      (x) => {
        expect(x).toContain('lost — engine restarted')
        expect(button(x, 'Run again')).toBeDefined()
      },
    ],
    [
      'superseded',
      cell(1, state(ref, { superseded_by: chatRef(1, '43') })),
      (x) => {
        expect(x).toContain('Replaced by a newer version ↓')
        expect(button(x, 'Preview')).toBeUndefined()
      },
    ],
    ['stale', cell(1, state(ref, { stale: true, latest: run() }), { extras: { open: true } }), (x) => expect(x).toContain('ran an earlier version')],
    ['edited', cell(1, state(ref), { edited: true }), (x) => expect(x).toContain('theuth will see the code that actually ran.')],
    [
      'invalid',
      cell(1, state(ref, { problems: [{ code: 'after_unknown', severity: 'error', message: '`after: preview` names no cell', key: 'after' }] })),
      (x) => {
        expect(x).toContain('names no cell')
        expect(button(x, 'Preview')).toBeUndefined()
      },
    ],
    [
      'armed',
      cell(2, state(ref, { armed: { by: 'r_x', at_ms: Date.UTC(2026, 10, 3, 14, 31), summary: '~3 update · 0 replace · 0 delete' } })),
      (x) => {
        expect(x).toMatch(/armed by preview-ir-prod1 ✓ \d\d:\d\d · ~3 update/)
        expect(button(x, 'Apply to ir-prod1')).not.toContain('aria-disabled="true"')
      },
    ],
    ['disarmed', cell(2, state(ref), { extras: { afterLabel: 'Preview' } }), (x) => expect(button(x, 'after: Preview')).toContain('aria-disabled="true"')],
    [
      'guest',
      cell(1, state(ref, { latest: run() }), { guest: true, extras: { open: true } }),
      (x) => {
        expect(x).toContain('Only the owner can run cells')
        expect(button(x, 'Preview')).toBeUndefined()
      },
    ],
  ]
  expect(new Set(cases.map(([v]) => v)).size).toBe(14)
  for (const [want, { view, html }, check] of cases) {
    expect(view, want).toBe(want)
    check(html)
  }
})

test('danger is never colour-only and the Run button is named by the label (§11.9)', () => {
  const { html } = cell(2, state(ref, { armed: { by: 'r_x', at_ms: 0, summary: 's' } }))
  expect(html).toContain('>danger<')
  expect(button(html, 'Apply to ir-prod1')).toBeDefined()
})

test('danger cells refuse keyboard runs and explain; other cells run (§7.1)', () => {
  const calls: string[] = []
  const cellOf = (danger: boolean, runnable = true) => ({ runnable, danger, onRun: () => calls.push('run'), onRefused: () => calls.push('refused') })
  keyRun(cellOf(true), { type: 'cell' })
  keyRun(cellOf(true), { type: 'line', line: 1 })
  keyRun(cellOf(false), { type: 'line', line: 1 })
  keyRun(cellOf(false, false), { type: 'cell' })
  expect(calls).toEqual(['refused', 'refused', 'run'])
})

test('L11 fields render when present and nothing when absent', () => {
  const plain = cell(1, state(ref, { latest: run() }), { extras: { open: true } }).html
  expect(plain).not.toMatch(/masked for theuth|theuth got exit and expect only|type="password"/)

  const masked = cell(1, state(ref, { latest: { ...run(), masked: 2, private: true } as CellRunWire }), { extras: { open: true } }).html
  expect(masked).toContain('2 masked for theuth')
  expect(masked).toContain('theuth got exit and expect only')

  const asking = (password: boolean) =>
    cell(1, state(ref, { latest: { ...run({ state: 'running', exit: null, secs: null, ended_ms: null }), kernel: { ...run().kernel, spec: { type: 'python', path: '/usr/bin/python3' } }, input: { prompt: 'Token: ', password } } as CellRunWire })).html
  expect(asking(true)).toMatch(/Token: [\s\S]*type="password"/)
  expect(asking(false)).toContain('type="text"')
})
