import { expect, test } from 'vitest'
import { memory, setEdit, setPicked } from './edits'
import { run } from './test-fixtures'

test('171F B21: an edit is kept by refKey until the text is typed back to the original', () => {
  setEdit('chat:s/42/1', 'echo two', 'echo one')
  setEdit('chat:s/42/2', 'ls -la', 'ls')
  expect(memory.getState().edits).toEqual({ 'chat:s/42/1': 'echo two', 'chat:s/42/2': 'ls -la' })
  setEdit('chat:s/42/1', 'echo one', 'echo one')
  expect(memory.getState().edits).toEqual({ 'chat:s/42/2': 'ls -la' })
})

test('171F B13: a picked history run is kept per cell; null drops it', () => {
  setPicked('chat:s/42/1', run())
  expect(memory.getState().picked['chat:s/42/1']?.id).toBe(run().id)
  setPicked('chat:s/42/1', null)
  expect(memory.getState().picked).toEqual({})
})
