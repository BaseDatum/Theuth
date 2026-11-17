import { expect, test } from 'vitest'
import type { ContextEntry, HistoryEntry } from '@/rpc/bindings/method.agent.output'
import { type StampTries, stampStored, stampTried, stampWanted, unstampedKey } from './reduce'
import type { Entry } from './sessions'

const ctx = (seq: number) => ({ target: { kind: 'message', seq }, active: true }) as unknown as ContextEntry
const user = (text: string, turn: number, seq = turn) => ({ role: 'user', text, turn, context: ctx(seq) }) as unknown as HistoryEntry
const reply = (text: string, turn: number, seq: number) => ({ role: 'assistant', text, turn, context: ctx(seq) }) as unknown as HistoryEntry
const reasoning = (turn: number, seq: number) => ({ role: 'reasoning', text: 'hm', turn, context: ctx(seq) }) as unknown as HistoryEntry
const tool = (seq: number) => ({ role: 'tool', call_id: `c${seq}`, name: 'bash', args: {}, result: ['ok', false], context: ctx(seq) }) as unknown as HistoryEntry
const live = (id: string, text: string, turn: number, more: Partial<Entry> = {}): Entry => ({ id, kind: 'assistant', text, open: false, turn, ...more }) as Entry
const seqs = (entries: Entry[]) => entries.map((e) => (e.context?.target.kind === 'message' ? e.context.target.seq : null))

const A = 'Checking.\n\n```bash\n#| label: Disk\ndf -h /\n```\n'
const B = 'Now the logs.'
const C = 'Done.'
// Turn 1 settled (reloaded, so stamped); turn 5 running and drawn live.
const entries: Entry[] = [
  { id: 'u1', kind: 'user', text: 'old', turn: 1, context: ctx(1) },
  live('r1', 'old reply', 1, { context: ctx(2) }),
  { id: 'u5', kind: 'user', text: 'disk?', turn: 5 },
  live('a', A, 5),
  { id: 't6', kind: 'tool', callId: 'c7', name: 'bash', args: {}, pending: false, child: false },
  live('b', B, 5),
  live('c', C, 5, { open: true }),
]

test('the running turn’s stored replies stamp the live replies lacking context, in order; nothing else changes', () => {
  const page = [user('disk?', 5), reasoning(5, 6), reply(A, 5, 6), tool(7), reply(B, 5, 8), tool(9)]
  const out = stampStored(entries, page, 5)
  expect(seqs(out)).toEqual([1, 2, null, 6, null, 8, null])
  // Only `context` is added: ids, order, text, `open` stay; untouched entries keep their identity.
  expect(out.map((e) => e.id)).toEqual(entries.map((e) => e.id))
  expect(out[3]).toEqual({ ...entries[3], context: ctx(6) })
  for (const i of [0, 1, 2, 4, 6]) expect(out[i]).toBe(entries[i])
})

test('a later page needs only the rows after the last stamped reply, not the opener', () => {
  const first = stampStored(entries, [user('disk?', 5), reply(A, 5, 6), tool(7)], 5)
  expect(seqs(first)).toEqual([1, 2, null, 6, null, null, null])
  // The opener scrolled out of a small page; the anchor (seq 6, after its reasoning row) is enough.
  const out = stampStored(first, [reasoning(5, 6), reply(A, 5, 6), tool(7), reply(B, 5, 8)], 5)
  expect(seqs(out)).toEqual([1, 2, null, 6, null, 8, null])
})

test('the round not stored yet (the engine’s `ContextUsed` precedes its row) stamps nothing and changes nothing', () => {
  expect(stampStored(entries, [user('disk?', 5)], 5)).toBe(entries)
})

test('any doubt stamps nothing: a text differs, more stored replies than live ones, a page that does not reach back', () => {
  const cases: HistoryEntry[][] = [
    [user('disk?', 5), reply(A, 5, 6), reply(`${B} (edited)`, 5, 8)],
    // Three stored, two closed live: the open reply `c` never pairs, even with the right text.
    [user('disk?', 5), reply(A, 5, 6), reply(B, 5, 8), reply(C, 5, 10)],
    [tool(7), reply(B, 5, 8)],
  ]
  for (const page of cases) expect(stampStored(entries, page, 5)).toBe(entries)
})

test('other turns are untouched; no turn → nothing', () => {
  // Turn 1's reply lacks context here, but the page is turn 5's.
  const mixed = entries.map((e) => (e.id === 'r1' ? live('r1', 'old reply', 1) : e))
  const out = stampStored(mixed, [user('disk?', 5), reply(A, 5, 6)], 5)
  expect(out[1]).toBe(mixed[1])
  expect(seqs(out)[3]).toBe(6)
  expect(stampStored(entries, [user('disk?', 5), reply(A, 5, 6)], undefined)).toBe(entries)
})

test('`unstampedKey`: the running turn’s closed live replies without context, by id; none → null', () => {
  expect(unstampedKey({ entries, turnSeq: 5, running: true })).toBe('5:a b')
  expect(unstampedKey({ entries, turnSeq: 5, running: false })).toBe(null)
  expect(unstampedKey({ entries: entries.filter((e) => e.id !== 'a' && e.id !== 'b'), turnSeq: 5, running: true })).toBe(null)
})

test('R1: two reads that stamp nothing for the same replies stop the reads until the key changes', () => {
  // Each row: the key when an event asks, whether a read goes out, and the key that read leaves.
  const rows: [key: string | null, reads: boolean, after: string | null][] = [
    ['5:a', true, '5:a'], // `ContextUsed` before the store: a miss
    ['5:a', true, '5:a'], // refused for good (text differs): the second miss
    ['5:a', false, null], // gave up
    ['5:a', false, null],
    ['5:a b', true, '5:a b'], // a new reply closed: reads again
    ['5:a b', true, '5:a b'],
    ['5:a b', false, null],
    ['9:x', true, null], // another turn: reads again; this one stamped
    ['9:x y', true, '9:y'], // stamped one of two: progress, not a miss
    ['9:y', true, '9:y'],
    ['9:y', true, '9:y'],
    ['9:y', false, null],
    [null, false, null], // nothing unstamped: never reads
  ]
  let tries: StampTries | undefined
  rows.forEach(([key, reads, after], i) => {
    expect([i, stampWanted(tries, key)]).toEqual([i, reads])
    if (reads && key !== null) tries = stampTried(tries, key, after)
  })
})
