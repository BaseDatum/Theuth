import { expect, test } from 'vitest'
import type { PtyPage } from '@/rpc/bindings/PtyPage'
import { EMPTY, LIVE, type Origin, type State, applyReply, keyToInput, nextViewport, palette, reduce } from './terminal'

const key = (job: string) => ({ harness: 'local', session: 's', job })
const page = (over: Partial<PtyPage>): PtyPage => ({
  session: 's',
  id: 'j1',
  revision: 1n,
  epoch: null,
  viewport: LIVE,
  origin: 0,
  retained: 0,
  rows: 40,
  cols: 120,
  alternate: false,
  cursor: null,
  wrapped: [],
  lines: [],
  truncated: false,
  redaction_complete: true,
  exit: null,
  ...over,
})
const updated = (over: Partial<PtyPage>) => ({ type: 'updated' as const, ...page(over) })
const at = (s: State): Origin => ({ queryKey: { key: s.key!, viewport: s.viewport }, epoch: s.epoch })

test('stale epoch or old key never paints; same key needs a newer revision', () => {
  const j1 = reduce(EMPTY, { type: 'select', key: key('j1') })
  const painted = applyReply(j1, updated({ revision: 5n }), at(j1))
  expect(painted.revision).toBe(5)
  const j2 = reduce(painted, { type: 'select', key: key('j2') })
  expect(j2.page).toBeNull()
  // Higher revision, but from j1's request: dropped.
  expect(applyReply(j2, updated({ revision: 9n }), at(j1))).toBe(j2)
  const fresh = applyReply(j2, updated({ id: 'j2', revision: 2n }), at(j2))
  expect(fresh.revision).toBe(2)
  expect(applyReply(fresh, updated({ id: 'j2', revision: 1n }), at(fresh))).toBe(fresh)
  // Reselecting the same job keeps the epoch.
  expect(reduce(fresh, { type: 'select', key: key('j2') })).toBe(fresh)
})

test('unchanged is trusted only for the viewport asked for', () => {
  const j1 = reduce(EMPTY, { type: 'select', key: key('j1') })
  const s = applyReply(j1, updated({ revision: 1n }), at(j1))
  expect(applyReply(s, { type: 'unchanged', viewport: LIVE, revision: 1n }, at(s))).toBe(s)
  const dirty = applyReply(s, { type: 'unchanged', viewport: { anchor: 'history', offset: 3, height: 40 }, revision: 1n }, at(s))
  expect(dirty.dirty).toBe(true)
  expect(applyReply(dirty, updated({ revision: 1n }), at(dirty)).dirty).toBe(false)
})

test('unavailable and daemon restart flags', () => {
  const s = reduce(EMPTY, { type: 'select', key: key('j1') })
  expect(applyReply(s, { type: 'unavailable' }, at(s)).unavailable).toBe(true)
  const a = applyReply(s, updated({ revision: 1n, epoch: 'e1' }), at(s))
  expect(a.daemonRestarted).toBe(false)
  const b = applyReply(a, updated({ revision: 2n, epoch: 'e2' }), at(a))
  expect(b.daemonRestarted).toBe(true)
  expect(applyReply(a, updated({ revision: 2n, epoch: null }), at(a)).daemonRestarted).toBe(false)
})

test('history navigation clamps to what is retained', () => {
  const p = page({ retained: 100, rows: 40 })
  expect(nextViewport(LIVE, 'down', p)).toEqual(LIVE)
  expect(nextViewport(LIVE, 'up', null)).toEqual(LIVE)
  expect(nextViewport(LIVE, 'up', page({ retained: 0 }))).toEqual(LIVE)
  const top = nextViewport(LIVE, 'up', p)
  expect(top).toEqual({ anchor: 'history', offset: 0, height: 40 })
  expect(nextViewport(top, 'pageUp', p)).toEqual({ anchor: 'history', offset: 40, height: 40 })
  expect(nextViewport({ anchor: 'history', offset: 59, height: 40 }, 'up', p)).toEqual({ anchor: 'history', offset: 60, height: 40 })
  expect(nextViewport({ anchor: 'history', offset: 60, height: 40 }, 'up', p)).toEqual({ anchor: 'history', offset: 60, height: 40 })
  expect(nextViewport({ anchor: 'history', offset: 5, height: 40 }, 'pageDown', p)).toEqual(LIVE)
  expect(nextViewport(top, 'down', p)).toEqual(LIVE)
  expect(nextViewport(top, 'up', page({ retained: 500, rows: 200 }))).toEqual({ anchor: 'history', offset: 1, height: 80 })
})

test('palette: ansi, cube, greys', () => {
  expect(palette(1)).toBe('#cd0000')
  expect(palette(16)).toBe('rgb(0,0,0)')
  expect(palette(196)).toBe('rgb(255,0,0)')
  expect(palette(231)).toBe('rgb(255,255,255)')
  expect(palette(232)).toBe('rgb(8,8,8)')
  expect(palette(255)).toBe('rgb(238,238,238)')
})

test('keyToInput: text, modifiers, named keys; browser and IME chords pass through', () => {
  const k = (key: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; isComposing: boolean }> = {}) =>
    keyToInput({ key, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, ...mods })
  expect(k('a')).toEqual({ text: 'a', keys: [] })
  expect(k('é')).toEqual({ text: 'é', keys: [] })
  expect(k('c', { ctrlKey: true })).toEqual({ text: null, keys: [{ ctrl: 'c' }] })
  expect(k('x', { altKey: true })).toEqual({ text: null, keys: [{ alt: 'x' }] })
  expect(k('Tab', { shiftKey: true })).toEqual({ text: null, keys: ['shift_tab'] })
  expect(k('ArrowUp')).toEqual({ text: null, keys: ['up'] })
  expect(k('F5')).toEqual({ text: null, keys: [{ f: 5 }] })
  expect(k('Shift')).toBeNull()
  expect(k('c', { metaKey: true })).toBeNull()
  expect(k('a', { isComposing: true })).toBeNull()
})
