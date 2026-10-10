import { afterEach, expect, test, vi } from 'vitest'
import { CHAT_MIN_PX, NOTES_MIN_PX, OPEN, RAIL_PX, SIDE_PX, clampNotesWidth, layout, useNotebookColumns } from './notebook-columns'

afterEach(() => vi.unstubAllGlobals())

test('the phone gets tabs, the desktop the three-column grid with collapsed sides as 12 px rails', () => {
  expect(layout(true, OPEN)).toEqual({ mode: 'tabs' })
  expect(layout(false, OPEN)).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) ${SIDE_PX}px` })
  expect(layout(false, { sources: true, notes: false })).toEqual({ mode: 'columns', template: `${RAIL_PX}px minmax(0,1fr) ${SIDE_PX}px` })
  expect(layout(false, { sources: false, notes: true })).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) ${RAIL_PX}px` })
})

test('157 D7: the remembered notes width is the third track; collapsed it is still the rail', () => {
  expect(layout(false, OPEN, 480)).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) 480px` })
  expect(layout(false, { sources: false, notes: true }, 480)).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) ${RAIL_PX}px` })
})

test('clampNotesWidth: 280 px floor, 60 % ceiling, and the chat column keeps 360 px beside the sources column', () => {
  // 1440 wide, sources open: 60 % = 864 but 1440 - 320 - 360 = 760 binds first.
  expect(clampNotesWidth(900, 1440, SIDE_PX)).toBe(760)
  expect(clampNotesWidth(100, 1440, SIDE_PX)).toBe(NOTES_MIN_PX)
  expect(clampNotesWidth(500, 1440, SIDE_PX)).toBe(500)
  // Sources collapsed to the rail: the 60 % ceiling binds.
  expect(clampNotesWidth(2000, 1440, RAIL_PX)).toBe(864)
  // A page too narrow for both minimums: the pane's minimum wins over the chat's.
  expect(clampNotesWidth(400, NOTES_MIN_PX + CHAT_MIN_PX + SIDE_PX - 100, SIDE_PX)).toBe(NOTES_MIN_PX)
  expect(clampNotesWidth(333.6, 1440, SIDE_PX)).toBe(334)
})

test('toggle flips one side and persists both with the width under notebook-columns; setNotesWidth persists the width with the sides', () => {
  const stored: Record<string, string> = {}
  vi.stubGlobal('localStorage', { getItem: (k: string) => stored[k] ?? null, setItem: (k: string, v: string) => (stored[k] = v) })
  useNotebookColumns.getState().toggle('notes')
  expect(useNotebookColumns.getState().collapsed).toEqual({ sources: false, notes: true })
  expect(JSON.parse(stored['notebook-columns'] ?? '')).toEqual({ sources: false, notes: true, notesWidth: SIDE_PX })
  useNotebookColumns.getState().toggle('notes')
  expect(useNotebookColumns.getState().collapsed.notes).toBe(false)
  useNotebookColumns.getState().setNotesWidth(512.4)
  expect(useNotebookColumns.getState().notesWidth).toBe(512)
  expect(JSON.parse(stored['notebook-columns'] ?? '')).toEqual({ sources: false, notes: false, notesWidth: 512 })
})
