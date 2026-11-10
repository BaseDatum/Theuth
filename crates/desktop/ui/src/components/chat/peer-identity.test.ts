import { expect, test } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'
import { type Directory, initials, parseReceipts, resolveSender, sameText, senderColor, trackOf } from './peer-identity'

const KID = '01a1c28b-8e10-7757-afd9-db253e4f5e73'
const SIB = '01a1c28b-0000-7757-afd9-db253e4f5e74'
const PARENT = '01a1c28b-1111-7757-afd9-db253e4f5e75'
const BINDING = '9b0e2b1c-5d7a-4c55-8f0a-2a1e7a3b9c10'
const row = (id: string, extra: Partial<SessionSummary> = {}) => ({ id, title: `title of ${id.slice(0, 4)}`, name: null, parent: null, owner: '', here: true, ...extra }) as SessionSummary
const dir: Directory = {
  self: 'me',
  children: { [KID]: { id: KID, name: 'z4-183', agent: 'omp' } as AgentInfo },
  sessions: [row('me', { parent: PARENT }), row(KID, { title: 'kid title' }), row(SIB, { title: 'Notes on mesh', owner: 'e333', here: false }), row(PARENT, { title: 'The plan' })],
  peers: [{ name: 'sky', binding: BINDING }],
  contacts: [{ name: 'sky', machine: '333' }],
  mesh: { self_id: 'e1', self_name: 'laptop', peers: [{ id: 'e333', name: '333' }] as never },
  exported: 'hi-there',
}

test('names: subagent name, then session title, then contact name, then alias, then the short id', () => {
  // A subagent's own name beats its session title.
  expect(resolveSender(`session:${KID}`, dir)).toMatchObject({ name: 'z4-183', subtitle: 'Your subagent · omp', session: KID })
  expect(resolveSender(`session:${SIB}`, dir)).toMatchObject({ name: 'Notes on mesh', subtitle: 'Chat on 333', session: SIB })
  expect(resolveSender(`session:${PARENT}`, dir)).toMatchObject({ name: 'The plan', subtitle: 'Parent' })
  expect(resolveSender('parent', dir)).toMatchObject({ name: 'The plan', subtitle: 'Parent' })
  // `peer:<binding uuid>` → the binding's alias → the contact.
  expect(resolveSender(`peer:${BINDING}`, dir)).toMatchObject({ name: 'sky', subtitle: 'Contact · on 333 · via hi-there' })
  expect(resolveSender('peer:sky/notes', dir)).toMatchObject({ name: 'sky', subtitle: 'Contact · on 333 · via notes' })
  expect(resolveSender('contact:e333:sky', dir).name).toBe('sky')
  // No contact: the alias, then the short id.
  expect(resolveSender('peer:lab', dir)).toMatchObject({ name: 'lab', subtitle: 'A2A peer · via hi-there' })
  expect(resolveSender('peer:0d1e2f3a-5d7a-4c55-8f0a-2a1e7a3b9c10', dir).name).toBe('0d1e2f3a')
  expect(resolveSender('session:ffffffff-8e10-7757-afd9-db253e4f5e73', dir)).toEqual({ name: 'ffffffff', subtitle: undefined, key: 'session:ffffffff-8e10-7757-afd9-db253e4f5e73', session: undefined })
  // A label stays a label unless it names a subagent.
  expect(resolveSender('job j4', dir)).toEqual({ name: 'job j4', key: 'job j4' })
  expect(resolveSender('z4-183', dir)).toMatchObject({ subtitle: 'Your subagent · omp', session: KID })
})

test('initials follow the mock: a short first word as written, else its first letter', () => {
  expect(['z4-183', 'r183-sec', 'Notes on mesh', 'sky', ''].map(initials)).toEqual(['Z4', 'R', 'N', 'S', '?'])
})

test('colour: one stable project-palette class per sender, never ochre or carnelian', () => {
  const keys = Array.from({ length: 400 }, (_, i) => `session:${i.toString(16).padStart(8, '0')}-8e10-7757-afd9-db253e4f5e73`)
  const colours = keys.map(senderColor)
  expect(keys.map(senderColor)).toEqual(colours)
  for (const c of colours) expect(c).toMatch(/^bg-project-(lapis|faience|malachite|plum|umber|sand|slate|nile)$/)
  // The hash spreads: every allowed mineral is used.
  expect(new Set(colours).size).toBe(8)
})

test('track: accepted Sent, consumed Read, replied Answered; held warns, failed and suppressed are danger, unknown is muted', () => {
  expect(trackOf('accepted')).toMatchObject({ step: 1, word: 'sent' })
  expect(trackOf('consumed')).toMatchObject({ step: 2, word: 'read' })
  expect(trackOf('replied')).toMatchObject({ step: 3, tone: 'ok', word: 'answered' })
  expect(trackOf('held', "sky hasn't allowed messages yet")).toMatchObject({ step: 1, tone: 'warning', note: "sky hasn't allowed messages yet" })
  expect(trackOf('failed', 'offline')).toMatchObject({ tone: 'danger', note: 'offline' })
  expect(trackOf('suppressed', 'blocked')).toMatchObject({ tone: 'danger', note: 'blocked' })
  expect(trackOf('unknown')).toMatchObject({ tone: 'muted', word: 'outcome unknown' })
})

test('receipts: one per recipient line, the reply lines ride on the receipt above them', () => {
  const a = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af5'
  const b = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af6'
  expect(parseReceipts(`session:x replied id=${a} task=t1\nVERDICT: SAFE.\nsecond line\npeer:bob failed id=${b} error=bob's machine is offline`)).toEqual([
    { to: 'session:x', state: 'replied', id: a, reply: 'VERDICT: SAFE.\nsecond line' },
    { to: 'peer:bob', state: 'failed', id: b, error: "bob's machine is offline" },
  ])
  expect(parseReceipts('communication is off')).toEqual([])
})

test('repeats: the same words after trimming and collapsing whitespace', () => {
  expect(sameText('  DONE  Z3\n\nok ', 'DONE Z3 ok')).toBe(true)
  expect(sameText('DONE Z3', 'DONE Z4')).toBe(false)
})
