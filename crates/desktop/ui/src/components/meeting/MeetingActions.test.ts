import { expect, test, vi } from 'vitest'
// The page embeds the chat pane, whose `nudges` store reads `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { STEPS, lifecycle } from './ControlCenter'
import { actionsFor, deletable, layout } from './MeetingActions'

const review = lifecycle('Review', null, null, true)
const done = lifecycle('Done', null, null, true)

test('Review: Done reviewing then a disabled Summarize; once reviewed, the muted time and an enabled Summarize', () => {
  expect(actionsFor('Review', null, review)).toEqual([{ kind: 'done_reviewing' }, { kind: 'summarize', enabled: false }])
  expect(actionsFor('Review', '2026-10-09T21:30:00Z', review)).toEqual([{ kind: 'reviewed', at: '2026-10-09T21:30:00Z' }, { kind: 'summarize', enabled: true }])
})

test('Done re-runs the pipeline; Failed offers the strip fix (post retry, or the summary retry); nothing while Post, Summarizing or live', () => {
  expect(actionsFor('Done', null, done)).toEqual([{ kind: 'rerun' }])
  expect(actionsFor('Failed', null, lifecycle('Failed', null, 'align: no words', true))).toEqual([{ kind: 'rerun' }])
  expect(actionsFor('Failed', null, { current: STEPS.indexOf('Review'), busy: false, failure: 'summary failed', action: 'summarize' })).toEqual([{ kind: 'retry_summary' }])
  // A start that failed has no strip and nothing to fix from the row.
  expect(actionsFor('Failed', null, null)).toEqual([])
  expect(actionsFor({ Post: 'Diarizing' }, null, lifecycle({ Post: 'Diarizing' }, null, null, true))).toEqual([])
  expect(actionsFor('Summarizing', null, lifecycle('Summarizing', null, null, true))).toEqual([])
  expect(actionsFor('Recording', null, null)).toEqual([])
  expect(actionsFor(undefined, null, null)).toEqual([])
})

test('Delete is offered in every lifecycle state and for a failed start, never while live', () => {
  expect(deletable('Done', done)).toBe(true)
  expect(deletable({ Post: 'Aligning' }, lifecycle({ Post: 'Aligning' }, null, null, true))).toBe(true)
  expect(deletable('Failed', null)).toBe(true)
  expect(deletable('Recording', null)).toBe(false)
  expect(deletable(undefined, null)).toBe(false)
})

test('compact keeps the current step in the row and overflows the rest into the menu; desktop keeps the row whole', () => {
  const before = actionsFor('Review', null, review)
  expect(layout(before, false)).toEqual({ row: before, menu: [] })
  expect(layout(before, true)).toEqual({ row: [{ kind: 'done_reviewing' }], menu: [{ kind: 'summarize', enabled: false }] })
  const after = actionsFor('Review', '2026-10-09T21:30:00Z', review)
  expect(layout(after, true)).toEqual({ row: [{ kind: 'summarize', enabled: true }], menu: [{ kind: 'reviewed', at: '2026-10-09T21:30:00Z' }] })
  expect(layout([], true)).toEqual({ row: [], menu: [] })
})
