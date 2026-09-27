import { expect, test } from 'vitest'
import { compose, parse } from './attach'
import { type BoardRef, QUOTE_MAX, composeAsk, formatHeader, parseAsk, parseHeader } from './board-ask'

// Design 71 §2.9: every field shape the reader emits, formatted then parsed back to the same value.
const REFS: [string, BoardRef][] = [
  ['[board d1]', { doc: 'd1', block: null, at: null }],
  ['[board d1 · block b2]', { doc: 'd1', block: 'b2', at: null }],
  ['[board d1 · block b2 · crates/surface/src/fences.rs:74-87]', { doc: 'd1', block: 'b2', at: { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [74, 87] } }],
  ['[board d1 · block b2 · crates/surface/src/fences.rs:83]', { doc: 'd1', block: 'b2', at: { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [83, 83] } }],
  ['[board d1 · block b2 · crates/surface/src/lib.rs]', { doc: 'd1', block: 'b2', at: { kind: 'code', path: 'crates/surface/src/lib.rs', lines: null } }],
  ['[board d1 · block b2 · node closing]', { doc: 'd1', block: 'b2', at: { kind: 'node', id: 'closing' } }],
  ['[board d1 · block b2 · step 6]', { doc: 'd1', block: 'b2', at: { kind: 'step', n: 6 } }],
  ['[board agqobuk5ij52bmuz6lg7i4dmzu · stale 3]', { doc: 'agqobuk5ij52bmuz6lg7i4dmzu', block: null, at: { kind: 'stale', count: 3 } }],
]

test('header grammar round-trips', () => {
  for (const [line, ref] of REFS) {
    expect(formatHeader(ref)).toBe(line)
    expect(parseHeader(line)).toEqual(ref)
  }
})

test('lines that are not headers are body text', () => {
  for (const s of ['[board]', '[board d1 · block]', '[board d1 · block b1 · block b2]', '[board d1 · node x · step 2]', '[board d1 · ../etc:1]', 'board d1', '[board d1 · step -1]', ' [board d1]'])
    expect(parseHeader(s), s).toBeNull()
})

test('a question round-trips through the composer text, behind the pinned marker', () => {
  const ref: BoardRef = { doc: 'd1', block: 'b2', at: { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [40, 41] } }
  const quote = '                Some(j) => {\n                    return j;'
  const text = composeAsk(ref, quote)
  expect(text).toBe('[board d1 · block b2 · crates/surface/src/fences.rs:40-41]\n>                 Some(j) => {\n>                     return j;\n\n')
  // What the composer sends: `[attached note …]` leads, then the header; the transcript parses the body only.
  const sent = compose([{ kind: 'note', id: 'd1', title: 'Fence parsing', meta: '' }], `${text}why return here?`)
  const { refs, body } = parse(sent)
  expect(refs).toHaveLength(1)
  expect(parseAsk(body)).toEqual({ ref, quote, question: 'why return here?' })
  expect(parseAsk('why return here?')).toBeNull()
  expect(parseAsk(composeAsk({ doc: 'd1', block: null, at: null }, null, 'q'))).toEqual({ ref: { doc: 'd1', block: null, at: null }, quote: null, question: 'q' })
})

test('the quote is capped at 4 KiB, the question is not', () => {
  const long = 'x'.repeat(QUOTE_MAX + 100)
  const ask = parseAsk(composeAsk({ doc: 'd1', block: null, at: null }, long, long))!
  expect(ask.quote!.length).toBe(QUOTE_MAX)
  expect(ask.quote!.endsWith('…')).toBe(true)
  expect(ask.question.length).toBe(QUOTE_MAX + 100)
})
