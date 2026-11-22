import { beforeEach, expect, test, vi } from 'vitest'
import { activeFor, activeSuffix as resolve, load, type Suffix, type SuffixEngine, useSuffixes, withSuffix } from './suffixes'

const activeSuffix = (chat: string) => {
  const s = useSuffixes.getState()
  return resolve(s.list, activeFor(s.active[chat], s.always[chat]))
}

// No DOM in the test runner: the smallest `Storage` the store needs.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  clear: () => mem.clear(),
})

beforeEach(() => {
  localStorage.clear()
  useSuffixes.setState({ list: [], active: {}, engine: null, always: {} })
})

/** An engine keeping suffixes in memory, as `list_suffixes`/`put_suffix`/… do: a deleted id never comes back. */
function fakeEngine(seed: Suffix[] = []) {
  const list = new Map(seed.map((s) => [s.id, s]))
  const deleted = new Set<string>()
  const always = new Map<string, string | null>()
  const calls: string[] = []
  const engine: SuffixEngine = {
    list_suffixes: async () => (calls.push('list'), [...list.values()]),
    put_suffix: async (s) => {
      calls.push(`put ${s.id}`)
      if (deleted.has(s.id)) return false
      list.set(s.id, s)
      return true
    },
    delete_suffix: async (id) => (calls.push(`delete ${id}`), deleted.add(id), list.delete(id)),
    session_suffix: async (session) => {
      const id = always.get(session)
      return id && list.has(id) ? id : null
    },
    set_session_suffix: async (session, id) => void always.set(session, id),
  }
  return { engine, list, always, calls }
}

const SESSION = '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b'
// The fake's async bodies take effect synchronously at the call, so the store's fire-and-forget writes are visible at once.

test('with a supporting engine the store shows the engine’s list, not this device’s; without one, localStorage', async () => {
  localStorage.setItem('prompt-suffixes-migrated', '1')
  localStorage.setItem('prompt-suffixes', JSON.stringify({ list: [{ id: 'local', name: 'Local', text: 'here' }], active: {} }))
  useSuffixes.setState(load())
  const { engine, list } = fakeEngine([{ id: 'mesh', name: 'Mesh', text: 'everywhere' }])
  await useSuffixes.getState().connect(engine)
  expect(useSuffixes.getState().list.map((s) => s.id)).toEqual(['mesh'])
  // Edits go to the engine; this device's stored list is left as it was for an older engine.
  const id = useSuffixes.getState().add('New', 'text')
  expect(list.get(id)).toEqual({ id, name: 'New', text: 'text' })
  expect(load().list.map((s) => s.id)).toEqual(['local'])
  // Back on an engine without the methods: this device's list again.
  await useSuffixes.getState().connect(null)
  expect(useSuffixes.getState().list.map((s) => s.id)).toEqual(['local'])
})

test('an engine that cannot answer (a legacy probe) leaves the store on this device’s list', async () => {
  useSuffixes.getState().add('Local', 'here')
  const { engine } = fakeEngine()
  engine.list_suffixes = async () => {
    throw new Error('method not found')
  }
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
  await useSuffixes.getState().connect(engine)
  warn.mockRestore()
  expect(useSuffixes.getState().engine).toBeNull()
  expect(useSuffixes.getState().list.map((s) => s.name)).toEqual(['Local'])
})

test('the local list is uploaded to the first supporting engine once, by id', async () => {
  const s = useSuffixes.getState()
  const a = s.add('Links', 'Link everything.')
  const b = s.add('Terse', 'Be terse.')
  const first = fakeEngine([{ id: a, name: 'Links', text: 'Link everything.' }])
  await useSuffixes.getState().connect(first.engine)
  expect(first.calls).toEqual([`put ${a}`, `put ${b}`, 'list'])
  expect([...first.list.keys()]).toEqual([a, b])
  expect(useSuffixes.getState().list.map((x) => x.id)).toEqual([a, b])
  // Another connection (a reconnect, a restart): nothing goes up again, so a suffix deleted meanwhile stays deleted.
  await useSuffixes.getState().connect(null)
  const again = fakeEngine()
  await useSuffixes.getState().connect(again.engine)
  expect(again.calls).toEqual(['list'])
})

test('an "every message" suffix lives with the session on the engine; the composer appends the engine’s', async () => {
  const { engine, always } = fakeEngine([{ id: 'x', name: 'Terse', text: 'Be terse.' }])
  localStorage.setItem('prompt-suffixes-migrated', '1')
  await useSuffixes.getState().connect(engine)
  // Turned on from another device: this one reads it from the engine.
  always.set(SESSION, 'x')
  await useSuffixes.getState().watch(SESSION)
  expect(activeSuffix(SESSION)).toEqual({ id: 'x', name: 'Terse', text: 'Be terse.', scope: 'always' })
  expect(withSuffix('fix it', activeSuffix(SESSION)?.text)).toBe('fix it\n\nBe terse.')
  // Sends do not spend it; the chip's × turns it off on the engine.
  useSuffixes.getState().sent(SESSION)
  expect(activeSuffix(SESSION)?.scope).toBe('always')
  useSuffixes.getState().clear(SESSION)
  expect(always.get(SESSION)).toBeNull()
  expect(activeSuffix(SESSION)).toBeNull()
  // A new chat's draft activation stays here until `carry` hands it to the created session's engine.
  useSuffixes.getState().activate('', 'x', 'always')
  expect(always.has('')).toBe(false)
  useSuffixes.getState().carry('', SESSION)
  expect(always.get(SESSION)).toBe('x')
  expect(useSuffixes.getState().active['']).toBeUndefined()
  expect(activeSuffix(SESSION)?.scope).toBe('always')
  // "This message" stays on this device and replaces the engine's "every message".
  useSuffixes.getState().activate(SESSION, 'x', 'once')
  expect(always.get(SESSION)).toBeNull()
  expect(useSuffixes.getState().active[SESSION]).toEqual({ id: 'x', scope: 'once' })
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
