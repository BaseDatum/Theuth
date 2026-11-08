import { expect, test } from 'vitest'
import { BOARD_KEY, type BoardStore, boardEntryRedirect, parseBoardSearch, recallBoard, rememberBoard, rememberOpenedTask, takeOpenedTask } from './boardMemory'

const store = (): BoardStore & { map: Map<string, string> } => {
  const map = new Map<string, string>()
  return { map, getItem: (k) => map.get(k) ?? null, setItem: (k, v) => void map.set(k, v) }
}

test('parseBoardSearch keeps the known keys and drops the rest', () => {
  expect(parseBoardSearch({ project: 'launch', agent: 'agent:ops', engine: 'mini', state: 'review', cancelled: true })).toEqual({
    project: 'launch', agent: 'agent:ops', engine: 'mini', state: 'review', cancelled: true,
  })
  expect(parseBoardSearch({ project: '' })).toEqual({ project: '' })
  expect(parseBoardSearch({ agent: '', engine: '' })).toEqual({})
  expect(parseBoardSearch({ state: 'nope' })).toEqual({})
  expect(parseBoardSearch({ cancelled: 'true' })).toEqual({})
  expect(parseBoardSearch({ cancelled: false })).toEqual({})
  expect(parseBoardSearch({ project: 1, agent: ['x'], engine: {}, state: 2, mock: '1' })).toEqual({})
  expect(parseBoardSearch({})).toEqual({})
})

test('182: `machine` (a home engine id) is kept like the other filters, dropped when empty or not text', () => {
  expect(parseBoardSearch({ machine: 'X', engine: 'mini' })).toEqual({ machine: 'X', engine: 'mini' })
  expect(parseBoardSearch({ machine: '' })).toEqual({})
  expect(parseBoardSearch({ machine: 3 })).toEqual({})
  const s = store()
  rememberBoard({ machine: 'X' }, s)
  expect(recallBoard(s)).toEqual({ machine: 'X' })
})

test('rememberBoard / recallBoard round-trip through storage, {} included', () => {
  const s = store()
  rememberBoard({ project: 'launch', state: 'review' }, s)
  expect(s.map.get(BOARD_KEY)).toBe('{"project":"launch","state":"review"}')
  expect(recallBoard(s)).toEqual({ project: 'launch', state: 'review' })
  rememberBoard({}, s)
  expect(s.map.get(BOARD_KEY)).toBe('{}')
  expect(recallBoard(s)).toEqual({})
})

test('recallBoard re-validates and survives junk', () => {
  const s = store()
  expect(recallBoard(s)).toEqual({})
  s.map.set(BOARD_KEY, '{not json')
  expect(recallBoard(s)).toEqual({})
  s.map.set(BOARD_KEY, '[1,2]')
  expect(recallBoard(s)).toEqual({})
  s.map.set(BOARD_KEY, 'null')
  expect(recallBoard(s)).toEqual({})
  s.map.set(BOARD_KEY, '{"project":"sre","state":"bogus","agent":"","cancelled":"yes"}')
  expect(recallBoard(s)).toEqual({ project: 'sre' })
  const broken: BoardStore = {
    getItem: () => {
      throw new Error('private mode')
    },
    setItem: () => {
      throw new Error('private mode')
    },
  }
  expect(recallBoard(broken)).toEqual({})
  expect(() => rememberBoard({ project: 'x' }, broken)).not.toThrow()
  expect(recallBoard(null)).toEqual({})
  expect(() => rememberBoard({}, null)).not.toThrow()
})

test('boardEntryRedirect restores only a bare entry with something remembered', () => {
  const remembered = { project: 'launch', state: 'review' as const }
  expect(boardEntryRedirect('enter', {}, remembered)).toEqual(remembered)
  expect(boardEntryRedirect('enter', {}, {})).toBeNull()
  expect(boardEntryRedirect('enter', { project: '' }, remembered)).toBeNull()
  expect(boardEntryRedirect('enter', { agent: 'agent:ops' }, remembered)).toBeNull()
  expect(boardEntryRedirect('stay', {}, remembered)).toBeNull()
  expect(boardEntryRedirect('stay', { project: 'sre' }, remembered)).toBeNull()
  expect(boardEntryRedirect('preload', {}, remembered)).toBeNull()
})

test('takeOpenedTask returns the last opened task once', () => {
  expect(takeOpenedTask()).toBeNull()
  rememberOpenedTask('a')
  rememberOpenedTask('b')
  expect(takeOpenedTask()).toBe('b')
  expect(takeOpenedTask()).toBeNull()
})
