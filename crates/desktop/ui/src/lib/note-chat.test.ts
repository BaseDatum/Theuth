import { afterEach, expect, test, vi } from 'vitest'
import { DEFAULT_HEIGHT, EMPTY, MIN_HEIGHT, panelHeight, useNoteChat } from './note-chat'

afterEach(() => vi.unstubAllGlobals())

test('desktop: the saved height, kept between MIN_HEIGHT and pane - 120', () => {
  expect(panelHeight(DEFAULT_HEIGHT, 1000, false)).toBe(320)
  expect(panelHeight(500, 1000, false)).toBe(500)
  expect(panelHeight(40, 1000, false)).toBe(MIN_HEIGHT)
  expect(panelHeight(2000, 1000, false)).toBe(880)
  // A pane shorter than MIN_HEIGHT + 120: the panel still gets MIN_HEIGHT; a pane shorter than that, the whole pane.
  expect(panelHeight(320, 200, false)).toBe(MIN_HEIGHT)
  expect(panelHeight(320, 100, false)).toBe(100)
})

test('mobile: capped at 45 % of the pane; a never-dragged panel starts at min(320, 40 %)', () => {
  // 844 px phone: 40 % = 337.6 → 320 wins; a dragged 500 is cut to 45 % = 379.8.
  expect(panelHeight(DEFAULT_HEIGHT, 844, true)).toBe(320)
  expect(panelHeight(500, 844, true)).toBeCloseTo(379.8)
  // 600 px pane (keyboard up): 40 % = 240 is the start, 45 % = 270 the cap.
  expect(panelHeight(DEFAULT_HEIGHT, 600, true)).toBe(240)
  expect(panelHeight(400, 600, true)).toBe(270)
  // Never below MIN_HEIGHT unless the pane is smaller than that.
  expect(panelHeight(DEFAULT_HEIGHT, 300, true)).toBe(MIN_HEIGHT)
  expect(panelHeight(40, 844, true)).toBe(MIN_HEIGHT)
  expect(panelHeight(DEFAULT_HEIGHT, 120, true)).toBe(120)
})

test('a note never touched opens with the chat; a saved open:false wins through set', () => {
  expect(EMPTY.open).toBe(true)
  const stored: Record<string, string> = {}
  vi.stubGlobal('localStorage', { getItem: (k: string) => stored[k] ?? null, setItem: (k: string, v: string) => (stored[k] = v) })
  const st = useNoteChat.getState()
  st.set('n1', { open: false })
  expect(useNoteChat.getState().notes.n1).toEqual({ open: false, height: DEFAULT_HEIGHT, session: null })
  // A later patch (the first message binds a session) keeps the user's close.
  useNoteChat.getState().set('n1', { session: 's1' })
  expect(useNoteChat.getState().notes.n1).toEqual({ open: false, height: DEFAULT_HEIGHT, session: 's1' })
  expect(JSON.parse(stored['note-chat'] ?? '{}').n1.open).toBe(false)
  expect(useNoteChat.getState().notes.n2 ?? EMPTY).toMatchObject({ open: true, session: null })
})
