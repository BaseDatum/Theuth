import { beforeEach, describe, expect, test } from 'vitest'
import { freshShellId, terminalKey, terminalName, terminalPath, useUserTerminals } from './terminals'

describe('user terminals store', () => {
  beforeEach(() => {
    useUserTerminals.getState().clear()
  })

  test('freshShellId generates distinct non-"main" shell ids', () => {
    const ids = new Set<string>()
    for (let i = 0; i < 20; i++) {
      const id = freshShellId()
      expect(id).not.toBe('main')
      expect(id.length).toBeGreaterThan(0)
      ids.add(id)
    }
    expect(ids.size).toBe(20)
  })

  test('terminalName and terminalPath format the label and route', () => {
    expect(terminalName('mac-studio')).toBe('mac-studio terminal')
    expect(terminalName('  ')).toBe('This machine terminal')
    expect(terminalKey({ engine: 'node-1', shell: 'term-abc' })).toBe('node-1:term-abc')
    expect(
      terminalPath({ engine: 'node-1', shell: 'term-abc', name: 'mac-studio terminal' }),
    ).toBe('/terminal/node-1/term-abc?name=mac-studio%20terminal')
  })

  test('stores terminals newest-first, dedupes by engine+shell, and removes by engine+shell', () => {
    const a = { engine: 'e1', shell: 'term-1', name: 'box-a terminal', node: 'box-a' }
    const b = { engine: 'e1', shell: 'term-2', name: 'box-a terminal', node: 'box-a' }
    useUserTerminals.getState().add(a)
    useUserTerminals.getState().add(b)
    expect(useUserTerminals.getState().terminals).toEqual([b, a])

    // Re-adding the same engine+shell moves it to the front without duplicating.
    useUserTerminals.getState().add(a)
    expect(useUserTerminals.getState().terminals).toEqual([a, b])

    useUserTerminals.getState().remove('e1', 'term-1')
    expect(useUserTerminals.getState().terminals).toEqual([b])
  })
})
