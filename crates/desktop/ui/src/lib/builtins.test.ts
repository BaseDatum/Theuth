import { describe, expect, test } from 'vitest'
import {
  type BuiltinCategory,
  type BuiltinTool,
  categoryOps,
  categoryState,
  categorySwitch,
  sessionCategoryOps,
  toolOps,
  toolSwitch,
  withDraftHides,
} from './builtins'

const tool = (name: string, flags: Partial<BuiltinTool> = {}): BuiltinTool => ({
  name,
  description: '',
  risk: 'read_only',
  direct: true,
  disabled: false,
  hidden: false,
  ...flags,
})
const files = (...tools: BuiltinTool[]): BuiltinCategory => ({ id: 'files', label: 'Files', description: '', tools })
const all = (flags: Partial<BuiltinTool>) => files(tool('read_file', flags), tool('write_file', flags), tool('grep', flags))

describe('switch states', () => {
  test('toolSwitch prefers always over session', () => {
    expect(toolSwitch(tool('a'))).toBe('on')
    expect(toolSwitch(tool('a', { hidden: true }))).toBe('session')
    expect(toolSwitch(tool('a', { hidden: true, disabled: true }))).toBe('always')
  })

  test('categorySwitch', () => {
    expect(categorySwitch(all({}))).toBe('on')
    expect(categorySwitch(all({ disabled: true }))).toBe('always')
    expect(categorySwitch(all({ hidden: true }))).toBe('session')
    expect(categorySwitch(files(tool('a', { disabled: true }), tool('b', { hidden: true })))).toBe('session')
    expect(categorySwitch(files(tool('a', { disabled: true }), tool('b')))).toBe('mixed')
  })

  test('categoryState looks only at tools not off always', () => {
    expect(categoryState(all({}))).toBe('on')
    expect(categoryState(all({ hidden: true }))).toBe('off')
    expect(categoryState(files(tool('a', { hidden: true }), tool('b')))).toBe('mixed')
    expect(categoryState(files(tool('a', { disabled: true }), tool('b', { hidden: true })))).toBe('off')
    expect(categoryState(files(tool('a', { disabled: true }), tool('b')))).toBe('on')
    // Every tool off always: nothing to switch for this chat.
    expect(categoryState(all({ disabled: true }))).toBe('off')
  })
})

describe('category ops', () => {
  test('this chat only: the key off, the key and every tool on', () => {
    expect(sessionCategoryOps(all({}), true)).toEqual([{ names: ['cat:files'], hidden: true }])
    expect(sessionCategoryOps(all({}), false)).toEqual([{ names: ['cat:files', 'read_file', 'write_file', 'grep'], hidden: false }])
  })

  test('off here writes the key and clears per-tool hides of the category', () => {
    const c = files(tool('read_file', { hidden: true }), tool('write_file'))
    expect(categoryOps(c, 'session', true)).toEqual({
      always: [],
      session: [
        { names: ['cat:files'], hidden: true },
        { names: ['read_file'], hidden: false },
      ],
    })
  })

  test('off always writes the key, clears per-tool entries and this chat\'s hides', () => {
    const c = files(tool('read_file', { disabled: true }), tool('write_file', { hidden: true }))
    expect(categoryOps(c, 'always', true)).toEqual({
      always: [
        { names: ['cat:files'], hidden: true },
        { names: ['read_file'], hidden: false },
      ],
      session: [{ names: ['cat:files', 'read_file', 'write_file'], hidden: false }],
    })
  })

  test('on lifts both scopes, only where something is off', () => {
    expect(categoryOps(all({}), 'on', true)).toEqual({ always: [], session: [] })
    expect(categoryOps(all({ disabled: true, hidden: true }), 'on', true)).toEqual({
      always: [{ names: ['cat:files', 'read_file', 'write_file', 'grep'], hidden: false }],
      session: [{ names: ['cat:files', 'read_file', 'write_file', 'grep'], hidden: false }],
    })
  })

  test('off here lifts off always', () => {
    expect(categoryOps(all({ disabled: true }), 'session', true)).toEqual({
      always: [{ names: ['cat:files', 'read_file', 'write_file', 'grep'], hidden: false }],
      session: [{ names: ['cat:files'], hidden: true }],
    })
  })

  test('without a session there are never session ops', () => {
    const c = all({ disabled: true, hidden: true })
    for (const to of ['on', 'session', 'always'] as const) expect(categoryOps(c, to, false).session).toEqual([])
    expect(categoryOps(c, 'session', false)).toEqual({ always: [], session: [] })
    expect(categoryOps(all({ hidden: true }), 'always', false)).toEqual({ always: [{ names: ['cat:files'], hidden: true }], session: [] })
  })
})

describe('tool ops', () => {
  test('one tool on inside an always-off category trades the key for the others', () => {
    expect(toolOps(all({ disabled: true }), 'grep', 'on', true)).toEqual({
      always: [
        { names: ['read_file', 'write_file'], hidden: true },
        { names: ['cat:files', 'grep'], hidden: false },
      ],
      session: [],
    })
  })

  test('one tool on inside a category off here trades the key for the others', () => {
    expect(toolOps(all({ hidden: true }), 'grep', 'on', true)).toEqual({
      always: [],
      session: [
        { names: ['read_file', 'write_file'], hidden: true },
        { names: ['cat:files', 'grep'], hidden: false },
      ],
    })
  })

  test('a lone off tool is switched by name', () => {
    const c = files(tool('read_file', { disabled: true }), tool('grep', { hidden: true }), tool('write_file'))
    expect(toolOps(c, 'read_file', 'on', true)).toEqual({ always: [{ names: ['read_file'], hidden: false }], session: [] })
    expect(toolOps(c, 'grep', 'on', true)).toEqual({ always: [], session: [{ names: ['grep'], hidden: false }] })
    expect(toolOps(c, 'write_file', 'session', true)).toEqual({ always: [], session: [{ names: ['write_file'], hidden: true }] })
  })

  test('off here lifts off always; off always clears this chat\'s hide', () => {
    const c = files(tool('read_file', { disabled: true }), tool('grep', { hidden: true }), tool('write_file'))
    expect(toolOps(c, 'read_file', 'session', true)).toEqual({
      always: [{ names: ['read_file'], hidden: false }],
      session: [{ names: ['read_file'], hidden: true }],
    })
    expect(toolOps(c, 'grep', 'always', true)).toEqual({
      always: [{ names: ['grep'], hidden: true }],
      session: [{ names: ['grep'], hidden: false }],
    })
  })

  test('without a session there are never session ops', () => {
    const c = all({ disabled: true, hidden: true })
    for (const to of ['on', 'session', 'always'] as const) expect(toolOps(c, 'grep', to, false).session).toEqual([])
    expect(toolOps(c, 'grep', 'on', false).always).toEqual([
      { names: ['read_file', 'write_file'], hidden: true },
      { names: ['cat:files', 'grep'], hidden: false },
    ])
  })
})

describe('withDraftHides', () => {
  test('a category key hides every tool of it', () => {
    const [c] = withDraftHides([all({})], { 'cat:files': true })
    expect(c!.tools.every((t) => t.hidden)).toBe(true)
  })

  test('a tool name hides that tool; a cleared entry shows it again', () => {
    const [c] = withDraftHides([files(tool('read_file'), tool('grep', { hidden: true }))], { read_file: true, grep: false })
    expect(c!.tools.map((t) => [t.name, t.hidden])).toEqual([
      ['read_file', true],
      ['grep', false],
    ])
  })

  test('no draft hides: unchanged', () => {
    const cats = [all({})]
    expect(withDraftHides(cats, undefined)).toBe(cats)
  })
})
