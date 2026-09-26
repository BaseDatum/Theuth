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
