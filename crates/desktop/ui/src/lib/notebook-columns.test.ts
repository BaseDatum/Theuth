import { afterEach, expect, test, vi } from 'vitest'
import { OPEN, RAIL_PX, SIDE_PX, layout, useNotebookColumns } from './notebook-columns'

afterEach(() => vi.unstubAllGlobals())

test('the phone gets tabs, the desktop the three-column grid with collapsed sides as 12 px rails', () => {
  expect(layout(true, OPEN)).toEqual({ mode: 'tabs' })
  expect(layout(false, OPEN)).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) ${SIDE_PX}px` })
  expect(layout(false, { sources: true, notes: false })).toEqual({ mode: 'columns', template: `${RAIL_PX}px minmax(0,1fr) ${SIDE_PX}px` })
  expect(layout(false, { sources: false, notes: true })).toEqual({ mode: 'columns', template: `${SIDE_PX}px minmax(0,1fr) ${RAIL_PX}px` })
})

test('toggle flips one side and persists both under notebook-columns', () => {
  const stored: Record<string, string> = {}
  vi.stubGlobal('localStorage', { getItem: (k: string) => stored[k] ?? null, setItem: (k: string, v: string) => (stored[k] = v) })
  useNotebookColumns.getState().toggle('notes')
  expect(useNotebookColumns.getState().collapsed).toEqual({ sources: false, notes: true })
  expect(JSON.parse(stored['notebook-columns'])).toEqual({ sources: false, notes: true })
  useNotebookColumns.getState().toggle('notes')
  expect(useNotebookColumns.getState().collapsed.notes).toBe(false)
})
