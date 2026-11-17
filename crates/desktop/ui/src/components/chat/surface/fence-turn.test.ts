import { expect, test } from 'vitest'
import type { Entry } from '@/store/sessions'
import { hasLaterAssistant, ownTurnRunning } from './fence-turn'

const user = (id: string, turn?: number): Entry => ({ id, kind: 'user', text: id, turn })
const reply = (id: string, turn?: number): Entry => ({ id, kind: 'assistant', text: id, open: false, turn })

// Turn 1 settled with a broken fence in its last reply `a1`; turn 5 runs: reply `a5` (broken), reply `b5` (the fix), a
// mid-turn steer (no turn: not an opener), then `tail`, a reply whose turn is unknown. `old` is an unknown-turn reply
// before turn 5's opener.
const ENTRIES: Entry[] = [user('u1', 1), reply('old'), reply('a1', 1), user('u5', 5), reply('a5', 5), reply('b5', 5), user('steer'), reply('tail')]

test.each([
  // [case, entry, session running, turnSeq, the entry's turn runs, a later reply exists]
  ['own turn running, a later reply exists', 'a5', true, 5, true, true],
  ['own turn running, no later reply', 'b5', true, 5, true, false],
  ['an earlier turn while a later one runs', 'a1', true, 5, false, false],
  ['unknown turn in the live tail', 'tail', true, 5, true, false],
  ['unknown turn before the last opener', 'old', true, 5, false, true],
  ['running turn unknown: the live tail runs', 'b5', true, undefined, true, false],
  ['running turn unknown: an earlier turn does not', 'a1', true, undefined, false, false],
  ['settled', 'a5', false, 5, false, true],
  ['settled, the last reply', 'b5', false, 5, false, false],
  ['entry not in the transcript', 'gone', true, 5, false, false],
] as const)('%s', (_, id, running, turnSeq, own, later) => {
  expect(ownTurnRunning(ENTRIES, id, running, turnSeq)).toBe(own)
  expect(hasLaterAssistant(ENTRIES, id)).toBe(later)
})
