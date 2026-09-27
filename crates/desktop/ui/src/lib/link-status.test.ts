import { expect, test } from 'vitest'
import { INITIAL, type LinkEvent, type LinkState, PARKED_AFTER_MS, diagnostics, failure, reduce } from './link-status'

const engine = { id: 'AbCdEfGh12345678', name: 'Studio Mac', relayHost: 'relay.example.com', secure: true }
const run = (events: LinkEvent[], from: LinkState = INITIAL) => events.reduce(reduce, from)
const T = 1_000_000

test('parked: nothing before 10 s, engine-offline with the elapsed seconds after, cleared by PRESENCE 1', () => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: false, at: T + 100 },
  ])
  expect(failure(s, T + 100 + PARKED_AFTER_MS - 1, engine)).toBeNull()
  const f = failure(s, T + 100 + PARKED_AFTER_MS + 2_400, engine)
  expect(f).toMatchObject({ kind: 'engine-offline', action: 'repair' })
  expect(f?.title).toBe('Studio Mac is not connected to the relay')
  expect(f?.detail).toContain('Waiting… (13s)')
  expect(failure(run([{ type: 'presence', online: true, at: T + 20_000 }], s), T + 40_000, engine)).toBeNull()
})

test('CLOSE 3 after PRESENCE 0 on this socket is a stale pairing; it survives the redial and a long park', () => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: false, at: T },
    { type: 'close', reason: 3, at: T + 50, handshake: false },
    { type: 'lost', at: T + 60, opened: true, reachable: false },
    { type: 'connecting', at: T + 600 },
    { type: 'presence', online: false, at: T + 700 },
  ])
  const f = failure(s, T + 700 + 60_000, engine)
  expect(f).toMatchObject({ kind: 'stale-pairing', action: 'repair' })
  expect(f?.title).toBe('This pairing no longer matches an engine on the relay')
  expect(failure(run([{ type: 'ready', at: T + 61_000 }], s), T + 61_000, engine)).toBeNull()
})

test('CLOSE 3 with no PRESENCE 0 first is the conn cap; CLOSE 3 answering our handshake is a revocation', () => {
  const full = run([
    { type: 'connecting', at: T },
    { type: 'close', reason: 3, at: T + 10, handshake: false },
    { type: 'lost', at: T + 11, opened: true, reachable: false },
  ])
  expect(failure(full, T + 11, engine)).toMatchObject({ kind: 'engine-full', title: 'Studio Mac is serving its maximum number of devices' })
  expect(failure(full, T + 11, engine)?.action).toBeUndefined()

  const revoked = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: true, at: T + 5 },
    { type: 'close', reason: 3, at: T + 10, handshake: true },
  ])
  expect(failure(revoked, T + 10, engine)).toMatchObject({ kind: 'revoked', action: 'repair' })
})

test('a parked socket after CLOSE 3 from a live engine reads as engine-offline once 10 s pass (only stale/revoked win over parked)', () => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'close', reason: 3, at: T + 10, handshake: false },
    { type: 'lost', at: T + 11, opened: true, reachable: false },
    { type: 'connecting', at: T + 600 },
    { type: 'presence', online: false, at: T + 650 },
  ])
  expect(failure(s, T + 700, engine)?.kind).toBe('engine-full')
  expect(failure(s, T + 650 + PARKED_AFTER_MS, engine)?.kind).toBe('engine-offline')
})

test.each([
  [4, 'engine-gone', 'Studio Mac went away'],
  [5, 'hub-moved', 'The mesh hub moved'],
  [1, 'queue-full', 'Disconnected: the relay queue for this device overflowed'],
  [2, 'idle', 'Disconnected after inactivity'],
])('CLOSE %i → %s, shown with the redial countdown', (reason, kind, title) => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: true, at: T + 5 },
    { type: 'close', reason, at: T + 10, handshake: false },
    { type: 'lost', at: T + 11, opened: true, reachable: false },
  ])
  const f = failure(s, T + 11, engine)
  expect(f).toMatchObject({ kind, title })
  expect(f?.detail).toMatch(/Retrying in 1s\.$/) // first redial: 500 ms, rounded up
  expect(f?.action).toBeUndefined()
})

test('CLOSE 0 names nothing; a socket that opened and dropped with no CLOSE is a plain reconnect', () => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: true, at: T + 5 },
    { type: 'close', reason: 0, at: T + 10, handshake: false },
    { type: 'lost', at: T + 11, opened: true, reachable: false },
  ])
  expect(failure(s, T + 11, engine)).toBeNull()
  expect(s.lastClose).toEqual({ reason: 0, at: T + 10 })
})

test('never-opened socket: unreachable cross-origin, rate-limited when the relay serves this page; backoff grows', () => {
  const dead = run([
    { type: 'connecting', at: T },
    { type: 'lost', at: T + 30, opened: false, reachable: false },
  ])
  expect(failure(dead, T + 30, engine)).toMatchObject({ kind: 'unreachable', title: 'Cannot reach relay.example.com from this network' })
  expect(failure(dead, T + 30, engine)?.detail).toContain('proxy')

  const limited = run(
    [
      { type: 'connecting', at: T + 500 },
      { type: 'lost', at: T + 530, opened: false, reachable: true },
      { type: 'connecting', at: T + 1_530 },
      { type: 'lost', at: T + 1_560, opened: false, reachable: true },
    ],
    dead,
  )
  const f = failure(limited, T + 1_560, engine)
  expect(f).toMatchObject({ kind: 'rate-limited', title: 'The relay is rate-limiting this network' })
  expect(f?.detail).toBe('Retrying in 2s.') // third socket: 500 · 2² = 2 s
  expect(limited.attempts).toBe(3)
})

test('diagnostics: one line each for relay, engine, state, last close, parked, attempts', () => {
  const s = run([
    { type: 'connecting', at: T },
    { type: 'presence', online: false, at: T + 100 },
    { type: 'close', reason: 3, at: T + 200, handshake: false },
  ])
  const d = diagnostics(s, T + 3_100, engine)
  expect(d).toContain('relay: wss://relay.example.com/h/AbCdEfGh…')
  expect(d).toContain('engine: Studio Mac (AbCdEfGh)')
  expect(d).toContain('state: parked · stale-pairing')
  expect(d).toContain(`last close: 3 refused at ${new Date(T + 200).toISOString()}`)
  expect(d).toContain('parked: 3s')
  expect(d).toContain('attempts: 1')
})
