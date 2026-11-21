import { beforeEach, expect, test, vi } from 'vitest'
import { activeSuffix as resolve, load, useSuffixes, withSuffix } from './suffixes'

const activeSuffix = (chat: string) => resolve(useSuffixes.getState().list, useSuffixes.getState().active[chat])

// No DOM in the test runner: the smallest `Storage` the store needs.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  clear: () => mem.clear(),
})

beforeEach(() => {
  localStorage.clear()
  useSuffixes.setState({ list: [], active: {} })
})

test('the suffix follows the message after a blank line; a blank suffix or none changes nothing', () => {
  expect(withSuffix('fix the bug', 'Link every PR you mention.')).toBe('fix the bug\n\nLink every PR you mention.')
  expect(withSuffix('fix the bug\n\n', '\nBe terse.\n')).toBe('fix the bug\n\nBe terse.')
  expect(withSuffix('multi\nline', 'a\nb')).toBe('multi\nline\n\na\nb')
  expect(withSuffix('fix the bug', '   ')).toBe('fix the bug')
  expect(withSuffix('fix the bug', undefined)).toBe('fix the bug')
  // Files only, no words: the suffix is the whole text.
  expect(withSuffix('', 'Describe the image.')).toBe('Describe the image.')
})

test('create, edit and delete persist; deleting a suffix turns it off in every chat', () => {
  const s = useSuffixes.getState()
  const a = s.add('Links', 'Link everything.')
  const b = s.add('Terse', 'Be terse.')
  s.update(a, { text: 'Link every PR.' })
  s.activate('c1', a, 'always')
  s.activate('c2', b, 'always')
  expect(load()).toEqual({
    list: [
      { id: a, name: 'Links', text: 'Link every PR.' },
      { id: b, name: 'Terse', text: 'Be terse.' },
    ],
    active: { c1: { id: a, scope: 'always' }, c2: { id: b, scope: 'always' } },
  })
  s.remove(a)
  expect(useSuffixes.getState().list.map((x) => x.id)).toEqual([b])
  expect(useSuffixes.getState().active).toEqual({ c2: { id: b, scope: 'always' } })
  expect(load().active).toEqual({ c2: { id: b, scope: 'always' } })
})

test('"this message" is spent by one send; "every message" stays until turned off; chats are independent', () => {
  const s = useSuffixes.getState()
  const id = s.add('Links', 'Link everything.')
  s.activate('c1', id, 'once')
  s.activate('c2', id, 'always')
  expect(activeSuffix('c1')).toEqual({ id, name: 'Links', text: 'Link everything.', scope: 'once' })
  s.sent('c1')
  s.sent('c2')
  s.sent('c2')
  expect(activeSuffix('c1')).toBeNull()
  expect(activeSuffix('c2')?.scope).toBe('always')
  s.clear('c2')
  expect(activeSuffix('c2')).toBeNull()
})

test('a new chat’s suffix moves from the draft key onto the created session', () => {
  const s = useSuffixes.getState()
  const id = s.add('Links', 'Link everything.')
  s.activate('', id, 'always')
  s.sent('')
  s.carry('', 'new-session')
  expect(useSuffixes.getState().active).toEqual({ 'new-session': { id, scope: 'always' } })
  // A "this message" suffix on a draft is spent by the send that created the chat: nothing to carry.
  s.activate('panel:', id, 'once')
  s.sent('panel:')
  s.carry('panel:', 'other')
  expect(useSuffixes.getState().active.other).toBeUndefined()
})

test('a suffix whose entry was deleted elsewhere is not applied', () => {
  useSuffixes.setState({ list: [], active: { c1: { id: 'gone', scope: 'always' } } })
  expect(activeSuffix('c1')).toBeNull()
})
