import { beforeEach, describe, expect, test } from 'vitest'
import type { Entry } from '@/store/sessions'
import type { Row } from './rows'
import { claimSurfaceEntrance, LiveRowTracker, resetSurfaceEntrances, shouldAnimateRow } from './entrance'

const entryRow = (id: string, kind: Entry['kind']): Row => ({
  key: id,
  entry: { id, kind } as Entry,
  prompts: null,
})

const runRow = (id: string): Row => ({
  key: id,
  run: [{ id, kind: 'tool' } as Entry],
  prompts: null,
})

const gapRow = (key: string): Row => ({ key, gap: '1 h later' })

describe('shouldAnimateRow', () => {
  test('animates tool runs, tool cards, assistant and info blocks; skips user bubbles and gaps', () => {
    expect(shouldAnimateRow(runRow('r1'))).toBe(true)
    expect(shouldAnimateRow(entryRow('t1', 'tool'))).toBe(true)
    expect(shouldAnimateRow(entryRow('a1', 'assistant'))).toBe(true)
    expect(shouldAnimateRow(entryRow('u1', 'user'))).toBe(false)
    expect(shouldAnimateRow(gapRow('g1'))).toBe(false)
  })
})

describe('LiveRowTracker', () => {
  test('seeds existing history without animating, then animates live tail arrivals once', () => {
    const t = new LiveRowTracker()
    const history = [entryRow('u1', 'user'), runRow('r1'), entryRow('a1', 'assistant')]
    t.sync('s1', history, false)
    for (const r of history) expect(t.isEntering(r.key)).toBe(false)

    // Prepending an older page never animates.
    const withOlder = [entryRow('u0', 'user'), entryRow('a0', 'assistant'), ...history]
    t.sync('s1', withOlder, false)
    expect(t.isEntering('u0')).toBe(false)
    expect(t.isEntering('a0')).toBe(false)

    // A live tool run arrives at the tail.
    const withLive = [...withOlder, entryRow('u2', 'user'), runRow('r2')]
    t.sync('s1', withLive, true)
    expect(t.isEntering('u2')).toBe(false)
    expect(t.isEntering('r2')).toBe(true)

    // Finishing clears active so virtualizer remounts never replay.
    t.finish('r2')
    expect(t.isEntering('r2')).toBe(false)
    t.sync('s1', withLive, true)
    expect(t.isEntering('r2')).toBe(false)
  })

  test('animates the very first live block in a brand-new running session, but stays quiet when opening a busy existing session', () => {
    const fresh = new LiveRowTracker()
    fresh.sync('new', [], false)
    fresh.sync('new', [entryRow('u1', 'user'), runRow('r1')], true)
    expect(fresh.isEntering('u1')).toBe(false)
    expect(fresh.isEntering('r1')).toBe(true)

    const existingBusy = new LiveRowTracker()
    existingBusy.sync('busy', [entryRow('u1', 'user'), runRow('r1'), entryRow('a1', 'assistant')], true)
    expect(existingBusy.isEntering('r1')).toBe(false)
    expect(existingBusy.isEntering('a1')).toBe(false)
  })

  test('skips animation on large bursts (e.g. un-collapsing tool history) and resets on session switch', () => {
    const t = new LiveRowTracker()
    t.sync('s1', [entryRow('u1', 'user')], false)
    t.sync('s1', [entryRow('u1', 'user'), runRow('r1'), runRow('r2'), runRow('r3'), runRow('r4'), runRow('r5')], false)
    expect(t.isEntering('r1')).toBe(false)
    expect(t.isEntering('r5')).toBe(false)

    // Switching sessions clears previous state and hydrates quietly.
    t.sync('s2', [entryRow('x1', 'user'), runRow('x2'), entryRow('x3', 'assistant')], false)
    expect(t.isEntering('x2')).toBe(false)
  })
})

describe('claimSurfaceEntrance', () => {
  beforeEach(() => resetSurfaceEntrances())

  test('claims once per surface key', () => {
    expect(claimSurfaceEntrance('jr:s1:e1:root')).toBe(true)
    expect(claimSurfaceEntrance('jr:s1:e1:root')).toBe(false)
    expect(claimSurfaceEntrance('jr:s1:e1:other')).toBe(true)
  })
})
