import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import type { Interpreter } from '@/rpc/bindings/Interpreter'
import { Cell, CellExtras, trimBlankTail } from './Cell'
import { hoverAfter, keyRun } from './CellEditor'
import { cellOf } from './grammar'
import { refKey } from './store'
import { chatRef, render, run, spec, state } from './test-fixtures'
import type { CellViewState } from './types'
import { chatEditable, interpreterText, viewOf } from './view'

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
        expect(button(x, 'Kill')).toContain('title="Kill — closes the kernel; the next run starts fresh"')
        // 171F B12: the live shell box sizes to its rows, no fixed 32rem.
        expect(x).toContain('max-h-[60vh]')
        expect(x).not.toContain('h-[32rem]')
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
      'stopped',
      cell(1, state(ref, { latest: run({ state: 'timeout', exit: { type: 'code', value: 130 }, secs: 5.3 }) })),
      (x) => {
        expect(x).toContain('timed out after 5.3 s')
        expect(x).toContain('✗ timed out')
        expect(x).not.toContain('stopped by you')
        expect(x).not.toContain('exit 130')
      },
    ],
    [
      'stopped',
      cell(1, state(ref, { latest: run({ state: 'killed', exit: { type: 'signal', value: 'SIGKILL' } }) })),
      (x) => {
        expect(x).toContain('killed — the kernel was closed; the next run starts fresh')
        expect(x).not.toContain('stopped by you')
      },
    ],
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

  const masked = cell(1, state(ref, { latest: { ...run(), masked: 2, private: true } }), { extras: { open: true } }).html
  expect(masked).toContain('2 masked for theuth')
  expect(masked).toContain('theuth got exit and expect only')

  const asking = (password: boolean) =>
    cell(1, state(ref, { latest: { ...run({ state: 'running', exit: null, secs: null, ended_ms: null }), kernel: { ...run().kernel, spec: { type: 'python', path: '/usr/bin/python3' } }, input: { prompt: 'Token: ', password } } })).html
  expect(asking(true)).toMatch(/Token: [\s\S]*type="password"/)
  expect(asking(false)).toContain('type="text"')
})

test('171F B3: a stored screen loses its trailing blank rows, keeping the colour reset; inner blank lines stay', () => {
  expect(trimBlankTail('a\nb\n\n\n\n')).toBe('a\nb')
  expect(trimBlankTail('\x1b[32mok\x1b[0m\n\x1b[0m\n   \n\r\n')).toBe('\x1b[32mok\x1b[0m\x1b[0m')
  expect(trimBlankTail('a\n\nb')).toBe('a\n\nb')
  expect(trimBlankTail('')).toBe('')
})

test('171F B27: an auto (or unset) python pick says so; an explicit pick and shell cells do not', () => {
  const py = (header: string) => cellOf('python', `#| label: Sum\n${header}print(1)`) as CellSpec
  const system: Interpreter = { kind: 'python', source: 'system', path: '/usr/bin/python3', version: '3.14.8', why: '', auto: true }
  expect(interpreterText(py(''), system)).toBe('auto → python 3.14.8 (system)')
  expect(interpreterText(py('#| python: auto\n'), system)).toBe('auto → python 3.14.8 (system)')
  expect(interpreterText(py('#| python: .venv\n'), { ...system, source: '.venv' })).toBe('python 3.14.8 (.venv)')
  expect(interpreterText(py(''), null)).toBe('python')
  expect(interpreterText(spec(1), { kind: 'bash', source: 'homebrew', path: '/opt/homebrew/bin/bash', version: '5.2', why: '', auto: true })).toBe('bash 5.2 (homebrew)')
})

test('171F B21: a runnable chat cell takes edits until a run, a newer version or an error holds it', () => {
  for (const v of ['ready', 'done', 'failed', 'stopped', 'stale', 'edited', 'armed', 'disarmed', 'lost'] as const) expect(chatEditable(v, true), v).toBe(true)
  for (const v of ['running', 'queued', 'superseded', 'invalid', 'guest'] as const) expect(chatEditable(v, true), v).toBe(false)
  // Not runnable: the reply is still being written, or a read-only view.
  expect(chatEditable('ready', false)).toBe(false)
})

test('171F B2: moving from the text onto the ► keeps the hovered line; only leaving the editor clears it', () => {
  let line = hoverAfter(null, { kind: 'move', line: 3 })
  expect(line).toBe(3)
  line = hoverAfter(line, { kind: 'leave', inEditor: true })
  expect(line).toBe(3)
  line = hoverAfter(line, { kind: 'move', line: 4 })
  expect(line).toBe(4)
  expect(hoverAfter(line, { kind: 'leave', inEditor: false })).toBeNull()
})

test('171F B13: what the reader toggled lives in the store, so a remounted cell shows it again', () => {
  const done = state(ref, { latest: run() })
  const k = refKey(ref)
  useToolRuns.setState({ disclosed: { [disclosureKey(k, 'raw')]: true, [disclosureKey(k, 'fold')]: false, [disclosureKey(k, 'output')]: false } })
  const html = cell(1, done).html
  expect(html).toContain('#| name: preview-ir-prod1')
  expect(html).toContain('aria-pressed="true"')
  expect(html).toContain('show output')
  expect(html).toContain('>fold</button>')
  useToolRuns.setState({ disclosed: {} })
  expect(cell(1, done).html).not.toContain('#| name: preview-ir-prod1')
})
