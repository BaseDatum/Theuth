// 208 §10.2: the delta reducer — contract §9.4 row by row.
import { expect, test } from 'vitest'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_DELTAS as D } from './fixture'
import { applyDelta, cursorOf, fromSnapshot, type GraphState, mergeSnapshot, settleSnapshot } from './reducer'

const d = (i: number): AgentGraphChanged => D[i] as AgentGraphChanged
const step = (s: GraphState, i: number): GraphState => {
  const r = applyDelta(s, d(i))
  expect(r.resnapshot).toBe(false)
  return r.state as GraphState
}

test('§9.4: deltas 0–4 applied in order', () => {
  const s0 = fromSnapshot(GRAPH_FIXTURE)
  expect(s0.seq).toBe(7)
  expect(cursorOf(s0)).toEqual({ epoch_ms: GRAPH_FIXTURE.epoch_ms, seq: 7 })

  const s1 = step(s0, 0)
  expect(s1.seq).toBe(8)
  expect(s1.nodes.size).toBe(41)
  expect(s1.nodes.get('r1.builder.fmt')?.parent).toBe('r1.builder')
  for (const [id, n] of s0.nodes) {
    if (id === 'r1.builder') expect(s1.nodes.get(id)).not.toBe(n)
    else expect(s1.nodes.get(id), id).toBe(n)
  }
  expect(s1.nodes).not.toBe(s0.nodes)
  for (const [id, e] of s0.edges) expect(s1.edges.get(id)).toBe(e)

  const s2 = step(s1, 1)
  expect([s2.seq, s2.edges.size, s2.pulses.length]).toEqual([9, 19, 1])

  const s3 = step(s2, 2)
  expect(s3.seq).toBe(10)
  expect(s3.nodes.get('r1.builder')?.activity.state).toEqual({ state: 'running' })

  const s4 = step(s3, 3)
  expect(s4.seq).toBe(11)
  expect(s4.nodes.get('r1.builder.fmt')?.outcome).toBe('finished')
  expect(s4.edges.get('r1.builder|r1.builder.fmt')).toMatchObject({ results: 1, open: 0 })
  expect(s4.pulses).toHaveLength(2)
  expect(s4.pulses[1]?.kind).toBe('result')

  const s5 = step(s4, 4)
  expect(s5.seq).toBe(12)
  expect(s5.nodes.size).toBe(40)
  expect(s5.nodes.has('r8')).toBe(false)
  expect(s5.edges.size).toBe(18)
  expect(s5.edges.has('r1|r8')).toBe(false)

  // 5: stale — the same state object back, no re-snapshot.
  const stale = applyDelta(s5, d(5))
  expect(stale.resnapshot).toBe(false)
  expect(stale.state).toBe(s5)
  // 6: a gap.
  const gap = applyDelta(s5, d(6))
  expect(gap.resnapshot).toBe(true)
  expect(gap.state).toBe(s5)
  // 7: a new activation.
  expect(applyDelta(s5, d(7)).resnapshot).toBe(true)
  // 8: resync, applied to the state after 4.
  const resync = applyDelta(s5, d(8))
  expect(resync.resnapshot).toBe(true)
  expect(resync.state).toBe(s5)
})

test('no state → re-snapshot; an unchanged renewal keeps the state itself; a full one replaces it', () => {
  expect(applyDelta(null, d(0))).toEqual({ state: null, resnapshot: true })
  const s = fromSnapshot(GRAPH_FIXTURE)
  expect(mergeSnapshot(s, { ...GRAPH_FIXTURE, unchanged: true, nodes: [], edges: [] })).toBe(s)
  const full = mergeSnapshot(s, GRAPH_FIXTURE)
  expect(full).not.toBe(s)
  expect(full.nodes.size).toBe(40)
  expect(mergeSnapshot(null, GRAPH_FIXTURE).seq).toBe(7)
  expect(cursorOf(null)).toBeNull()
})

test('review U3/U7: a reply older than the state is dropped; an unchanged one carries its window', () => {
  const s0 = fromSnapshot(GRAPH_FIXTURE)
  const s2 = step(step(s0, 0), 1)
  // A renewal answered at seq 7 while deltas 8 and 9 were applied: no rollback.
  expect(mergeSnapshot(s2, GRAPH_FIXTURE)).toBe(s2)
  // An earlier epoch is dropped too; a later one replaces.
  expect(mergeSnapshot(s2, { ...GRAPH_FIXTURE, epoch_ms: GRAPH_FIXTURE.epoch_ms - 1, seq: 99 })).toBe(s2)
  expect(mergeSnapshot(s2, { ...GRAPH_FIXTURE, epoch_ms: GRAPH_FIXTURE.epoch_ms + 1, seq: 0 }).seq).toBe(0)
  const wider = mergeSnapshot(s2, { ...GRAPH_FIXTURE, unchanged: true, nodes: [], edges: [], window_ms: s2.window + 1 })
  expect([wider.window, wider.nodes]).toEqual([s2.window + 1, s2.nodes])
})

test('review U3: deltas buffered while a snapshot was in flight apply after it, older ones skipped', () => {
  const snap = fromSnapshot(GRAPH_FIXTURE)
  const s1 = step(snap, 0)
  // Delta 0 is already in the snapshot (seq 8 reply); 1 and 2 arrived during the call.
  const atEight = mergeSnapshot(null, { ...GRAPH_FIXTURE, seq: 8, nodes: [...s1.nodes.values()], edges: [...s1.edges.values()] })
  const out = settleSnapshot(atEight, [d(0), d(1), d(2)])
  expect([out.state.seq, out.resnapshot]).toEqual([10, false])
  // A gap among them still asks for a re-snapshot; a later epoch does too.
  expect(settleSnapshot(atEight, [d(2)]).resnapshot).toBe(true)
  expect(settleSnapshot(atEight, [{ ...d(1), epoch_ms: GRAPH_FIXTURE.epoch_ms + 1 }]).resnapshot).toBe(true)
  // An older epoch is ignored.
  expect(settleSnapshot(atEight, [{ ...d(1), epoch_ms: GRAPH_FIXTURE.epoch_ms - 1 }])).toEqual({ state: atEight, resnapshot: false })
})

test('pulses are capped at PULSE_KEEP, newest kept', () => {
  let s = fromSnapshot(GRAPH_FIXTURE)
  const pulse = (d(1).pulses[0] as AgentGraphChanged['pulses'][number])
  for (let i = 0; i < 30; i++) {
    const ev: AgentGraphChanged = { ...d(1), seq: s.seq + 1, pulses: Array.from({ length: 10 }, (_, k) => ({ ...pulse, id: `p${i}-${k}` })) }
    s = applyDelta(s, ev).state as GraphState
  }
  expect(s.pulses).toHaveLength(200)
  expect(s.pulses[199]?.id).toBe('p29-9')
})
