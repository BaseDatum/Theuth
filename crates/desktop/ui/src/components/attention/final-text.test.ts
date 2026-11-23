import { expect, test } from 'vitest'
import type { HistoryEntry } from '@/rpc/bindings/method.agent.output'
import { afterFirstLine, finalResponse, firstLine } from './final-text'

const a = (text: string): HistoryEntry => ({ role: 'assistant', text, turn: null, context: null }) as HistoryEntry
const u = (text: string): HistoryEntry => ({ role: 'user', text, turn: 1, context: null, peer: null }) as HistoryEntry

test('first line skips blanks and strips a heading', () => {
  expect(firstLine('\n\n## Done: shipped\nmore')).toBe('Done: shipped')
})

test('the rest is everything below the first line, trimmed', () => {
  expect(afterFirstLine('Shipped it.\n\nDetails here.\n## Next\n- push')).toBe('Details here.\n## Next\n- push')
  expect(afterFirstLine('Only one line')).toBe('')
})

test('picks the row the summary came from, even when a newer turn followed', () => {
  const h = [u('go'), a('Merged the fix.\nAll green.'), u('again'), a('Working on it')]
  expect(finalResponse(h, 'Merged the fix.')).toBe('Merged the fix.\nAll green.')
})

test('a clipped summary still matches', () => {
  const long = 'x'.repeat(200)
  expect(finalResponse([a(`${long}\nbody`)], `${'x'.repeat(159)}\u2026`)).toBe(`${long}\nbody`)
})

test('no match falls back to the newest assistant text; none at all is null', () => {
  expect(finalResponse([a('one'), a('two')], 'zzz')).toBe('two')
  expect(finalResponse([u('hi')], 'hi')).toBeNull()
})
