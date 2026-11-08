import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import { digest } from './grammar'
import { confirmText, LineConfirm } from './LineConfirm'
import { type Ask, askSkipped, askStep, lineGuard } from './line-run'
import { pressNote } from './NoteCell'
import { fakeApi } from './test-fixtures'

const noop = () => undefined
const box = (over: Partial<Parameters<typeof LineConfirm>[0]> = {}) =>
  renderToStaticMarkup(h(LineConfirm, { top: 0, what: 'line', lines: [2, 2], title: 'Wipe scratch', reason: null, onRun: noop, onCancel: noop, ...over }))

test('CELLAFTER S1–S3: the confirm is a plain non-modal box — no aria-live, Cancel is the focus target, the question names the lines', () => {
  const html = box({ reason: 'after: wipe — run it first' })
  expect(html).not.toContain('aria-live')
  expect(html).not.toContain('aria-modal')
  expect(html).toContain('role="dialog"')
  // The mount effect focuses `[data-cancel]`: the safe choice, never the destructive Run.
  expect(html).toMatch(/<button[^>]*data-cancel="true">Cancel<\/button>/)
  expect(html.match(/data-cancel/g)).toHaveLength(1)
  expect(html).toContain('after: wipe — run it first')
  expect(confirmText('line', [2, 2], 'Wipe')).toEqual({ question: 'Run this line of Wipe?', action: 'Run line' })
  expect(confirmText('line', [3, 5], 'Wipe')).toEqual({ question: 'Run lines 3–5 of Wipe?', action: 'Run lines' })
  expect(confirmText('selection', [3, 5], 'Wipe')).toEqual({ question: 'Run this selection of Wipe?', action: 'Run selection' })
})

const TEXT_A = '#| name: wipe\n#| danger\necho one\necho two'
const TEXT_B = '#| name: wipe\n#| danger\necho one\nrm -rf two'
const ref: CellRef = { type: 'note', doc: 'aaaaaaaaaaaaaaaaaaaaaaaaaa', key: { kind: 'name', value: 'wipe' } }
const ask = (over: Partial<Ask> = {}): Ask => ({ line: 2, last: 2, mode: { type: 'line', line: 2 }, what: 'line', text: TEXT_A, reason: null, identity: 'doc:x/wipe', ...over })

test('CELLAFTER M1/S4: Cancel, Esc or a press outside never runs; a changed cell closes the ask; Run is confirmed on the press-time text', async () => {
  expect(askStep(ask(), { kind: 'cancel' })).toEqual({ ask: null, run: null })
  // The host feeds every text change: the same text keeps the ask, another closes it without a run.
  const open = ask()
  expect(askStep(open, { kind: 'code', text: TEXT_A })).toEqual({ ask: open, run: null })
  expect(askStep(open, { kind: 'code', text: TEXT_B })).toEqual({ ask: null, run: null })
  const store = new Map<string, string>()
  vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) })
  try {
    const { run } = askStep(open, { kind: 'run', skip: false })
    expect(run).toEqual({ line: 2, mode: { type: 'line', line: 2 }, text: TEXT_A })
    // The host sends that run with `confirmed` and the press-time text: its digest is `seen`, so a cell changed
    // since (TEXT_B on screen now) is refused by the engine as "this cell changed".
    const { api, calls } = fakeApi()
    await pressNote(api, ref, (run as { text: string }).text, (run as { mode: Ask['mode'] }).mode, null, true)
    const args = calls[0]?.[1] ?? []
    expect([args[1], args[6]]).toEqual([await digest(TEXT_A), true])
    expect(args[1]).not.toBe(await digest(TEXT_B))
    // Not ticked: the next ► asks again. Ticked: persisted, the next ► runs at once.
    expect(lineGuard(true, askSkipped(open.identity), null).kind).toBe('confirm')
    askStep(open, { kind: 'run', skip: true })
    expect(lineGuard(true, askSkipped(open.identity), null).kind).toBe('run')
  } finally {
    vi.unstubAllGlobals()
  }
})
