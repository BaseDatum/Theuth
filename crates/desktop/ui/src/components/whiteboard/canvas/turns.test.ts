import { describe, expect, it } from 'vitest'
import { lastTurns, TURN_CHARS } from './turns'

describe('session card turns', () => {
  it('keeps the newest user/assistant messages with text, oldest first, from store entries or history rows', () => {
    const store = [
      { kind: 'user', text: 'one' },
      { kind: 'tool', name: 'x' },
      { kind: 'assistant', text: 'two' },
      { kind: 'reasoning', text: 'hidden' },
      { kind: 'assistant', text: '   ' },
      { kind: 'user', text: 'three' },
    ]
    expect(lastTurns(store, 2)).toEqual([{ role: 'assistant', text: 'two' }, { role: 'user', text: 'three' }])
    const history = [{ role: 'user', text: 'a' }, { role: 'error', text: 'boom' }, { role: 'assistant', text: 'b' }]
    expect(lastTurns(history)).toEqual([{ role: 'user', text: 'a' }, { role: 'assistant', text: 'b' }])
  })

  it('cuts long messages', () => {
    const [t] = lastTurns([{ kind: 'assistant', text: 'x'.repeat(1000) }])
    expect(t!.text).toHaveLength(TURN_CHARS)
    expect(t!.text.endsWith('…')).toBe(true)
  })
})
