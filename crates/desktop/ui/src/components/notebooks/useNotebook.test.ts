import { expect, test, vi } from 'vitest'
import type { Source as NotebookSource } from "@/rpc/bindings/method.document.output"
import { applySourceEvent, firstLine, sourceEventOf } from './useNotebook'

vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/lib/notebooks-rpc', () => ({ notebookKeys: {}, useNotebooksApi: () => ({}) }))

const running = { status: 'running', progress: 40 } as NotebookSource

test('a bus frame becomes a SourceEvent only with the §13.1 fields; the tag case is tolerated', () => {
  expect(sourceEventOf({ type: 'Changed', source: 's1', status: 'completed' })).toEqual({ type: 'Changed', source: 's1', status: 'completed' })
  expect(sourceEventOf({ type: 'progress', source: 's1', percent: 62 })).toEqual({ type: 'Progress', source: 's1', percent: 62 })
  expect(sourceEventOf({ type: 'DocumentChanged', doc: 's1' })).toBeNull()
  expect(sourceEventOf({ type: 'Changed', source: 's1' })).toBeNull()
})

test('Progress keeps running with the percent; a terminal Changed clears it', () => {
  expect(applySourceEvent(running, { type: 'Progress', source: 's1', percent: 62 })).toMatchObject({ status: 'running', progress: 62 })
  expect(applySourceEvent(running, { type: 'Changed', source: 's1', status: 'completed' })).toMatchObject({ status: 'completed', progress: null })
  expect(applySourceEvent(running, { type: 'Changed', source: 's1', status: 'queued' })).toMatchObject({ status: 'queued', progress: 40 })
})

test('firstLine skips blanks and the title heading, strips emphasis', () => {
  expect(firstLine('# Title\n\nEverything about the drill.\n')).toBe('Everything about the drill.')
  expect(firstLine('\n\n# Title\n')).toBe('')
  expect(firstLine('*Prove the DR path.*\nmore')).toBe('Prove the DR path.')
  expect(firstLine(undefined)).toBe('')
})
