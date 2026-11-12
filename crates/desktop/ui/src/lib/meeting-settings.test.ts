import { expect, test, vi } from 'vitest'

// No DOM in the test runner: the smallest `Storage` the store needs. Imported dynamically so each `load()` sees the storage set up for it.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
})

test('the meeting page split is remembered next to the other meeting settings; a value off the divider range reads as the default', async () => {
  mem.set('meeting-settings', JSON.stringify({ split: 0.3, model: 'gpt' }))
  const a = await import('./meeting-settings')
  expect(a.useMeetingSettings.getState().split).toBe(0.3)
  a.useMeetingSettings.getState().setSplit(0.7)
  expect(a.useMeetingSettings.getState().split).toBe(0.7)
  // Written whole: the other settings ride along untouched.
  expect(JSON.parse(mem.get('meeting-settings') ?? '')).toMatchObject({ split: 0.7, model: 'gpt' })

  vi.resetModules()
  mem.set('meeting-settings', JSON.stringify({ split: 3 }))
  const b = await import('./meeting-settings')
  expect(b.useMeetingSettings.getState().split).toBe(b.DEFAULT_SPLIT)

  vi.resetModules()
  mem.delete('meeting-settings')
  const c = await import('./meeting-settings')
  expect(c.useMeetingSettings.getState().split).toBe(c.DEFAULT_SPLIT)
})

test('156 D3: the brief column width and collapse persist beside split; a saved width is clamped to the handle range, the default when absent', async () => {
  vi.resetModules()
  mem.set('meeting-settings', JSON.stringify({ split: 0.3, briefWidth: 2000, briefsOpen: false }))
  const a = await import('./meeting-settings')
  expect(a.useMeetingSettings.getState().briefWidth).toBe(a.MAX_BRIEF_WIDTH)
  expect(a.useMeetingSettings.getState().briefsOpen).toBe(false)
  a.useMeetingSettings.getState().setBriefWidth(120)
  expect(a.useMeetingSettings.getState().briefWidth).toBe(a.MIN_BRIEF_WIDTH)
  a.useMeetingSettings.getState().setBriefWidth(300.6)
  a.useMeetingSettings.getState().setBriefsOpen(true)
  expect(JSON.parse(mem.get('meeting-settings') ?? '')).toMatchObject({ split: 0.3, briefWidth: 301, briefsOpen: true })

  vi.resetModules()
  mem.set('meeting-settings', JSON.stringify({ briefWidth: 'wide' }))
  const b = await import('./meeting-settings')
  expect(b.useMeetingSettings.getState().briefWidth).toBe(b.DEFAULT_BRIEF_WIDTH)
  expect(b.useMeetingSettings.getState().briefsOpen).toBe(true)
  expect(b.clampBriefWidth(NaN)).toBe(b.DEFAULT_BRIEF_WIDTH)
})

test('the centre/note split persists beside the others: 5/9 when absent or off-range, setNoteSplit clamps, and a later setSplit/setBriefWidth keeps it', async () => {
  vi.resetModules()
  mem.delete('meeting-settings')
  const a = await import('./meeting-settings')
  expect(a.useMeetingSettings.getState().noteSplit).toBe(5 / 9)
  a.useMeetingSettings.getState().setNoteSplit(0.95)
  expect(a.useMeetingSettings.getState().noteSplit).toBe(0.85)
  a.useMeetingSettings.getState().setNoteSplit(0.05)
  expect(a.useMeetingSettings.getState().noteSplit).toBe(0.15)
  a.useMeetingSettings.getState().setNoteSplit(0.4)
  // The explicit-field-list trap: every other setter rewrites the whole record and must carry noteSplit along.
  a.useMeetingSettings.getState().setSplit(0.5)
  a.useMeetingSettings.getState().setBriefWidth(300)
  expect(JSON.parse(mem.get('meeting-settings') ?? '')).toMatchObject({ noteSplit: 0.4, split: 0.5, briefWidth: 300 })

  vi.resetModules()
  mem.set('meeting-settings', JSON.stringify({ noteSplit: 0.3 }))
  expect((await import('./meeting-settings')).useMeetingSettings.getState().noteSplit).toBe(0.3)
  for (const bad of [0.9, 0.1, 'half', null]) {
    vi.resetModules()
    mem.set('meeting-settings', JSON.stringify({ noteSplit: bad }))
    expect((await import('./meeting-settings')).useMeetingSettings.getState().noteSplit).toBe(5 / 9)
  }
})

test('the note handle drag keeps the line under the pointer: zero move = start ratio, +pairWidth/10 = +0.1, clamped at both ends', async () => {
  const { noteSplitAt } = await import('./meeting-settings')
  const start = { ratio: 0.5, x: 700, pairWidth: 800 }
  expect(noteSplitAt(start, 700)).toBe(0.5)
  expect(noteSplitAt(start, 780)).toBeCloseTo(0.6, 10)
  expect(noteSplitAt(start, 620)).toBeCloseTo(0.4, 10)
  expect(noteSplitAt(start, 1500)).toBe(0.85)
  expect(noteSplitAt(start, -100)).toBe(0.15)
})

test('194 row 17: the start form remembers an effort beside its model; a new model brings its own (none by default), no model clears it', async () => {
  vi.resetModules()
  mem.set('meeting-settings', JSON.stringify({ model: 'a/b' }))
  const a = await import('./meeting-settings')
  expect(a.useMeetingSettings.getState().effort).toBe(null)
  a.useMeetingSettings.getState().setModel('a/b', 'high')
  expect(JSON.parse(mem.get('meeting-settings') ?? '')).toMatchObject({ model: 'a/b', effort: 'high' })
  a.useMeetingSettings.getState().setSplit(0.5)
  expect(JSON.parse(mem.get('meeting-settings') ?? '')).toMatchObject({ model: 'a/b', effort: 'high' })
  a.useMeetingSettings.getState().setModel('c/d')
  expect(a.useMeetingSettings.getState().effort).toBe(null)
  a.useMeetingSettings.getState().setModel(null, 'high')
  expect(a.useMeetingSettings.getState()).toMatchObject({ model: null, effort: null })
})
