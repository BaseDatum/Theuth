import { expect, test } from 'vitest'
import { sceneFixture, T } from './fixtures'
import { ARC, arc, arcPoint, C, END_X, flowPath, hairlines, HAIRLINES_MAX, hourOf, LABEL_GAP, layoutEnds, needsYou, outcomeMix, outcomes, punchCard, RING, ringByActor, rings, rng, running, scene, sessionsToday, shortId, startOfDay } from './model'

// A fixed local evening, as the mockup's clock reads.
const NOW = new Date(2026, 9, 7, 19, 49, 19).getTime()
const F = sceneFixture(NOW)

test('arc geometry matches the mockup: 00:00 tick, 12:00 label upright, band path', () => {
  const [x, y] = arcPoint(0, ARC.R - 18)
  expect([x, y].map((v) => Math.round(v * 10) / 10)).toEqual([1031.2, 88.1])
  const a = arc(NOW, F.routines, F.activityToday)
  expect(a.ticks).toHaveLength(25)
  expect(a.labels.map((l) => l.text)).toEqual(['00:00', '02:00', '04:00', '06:00', '08:00', '10:00', '12:00', '14:00', '16:00', '18:00', '20:00', '22:00'])
  expect(a.labels[6]!.rot).toBeCloseTo(-14 + (28 * 12.55) / 24 + 90, 5)
  expect(a.band).toMatch(/^M 1066\.2 79\.4 A 1408 1408 0 0 1 1066\.2 760\.6 L 1031\.2 751\.9 A 1372 1372 0 0 0 1031\.2 88\.1 Z$/)
  // Fired today → filled; knowledge-sync is due later today → hollow; morning-brief's next is tomorrow → not marked twice.
  expect(a.marks.filter((m) => m.filled).map((m) => m.name).sort()).toEqual(['lunch-digest', 'morning-brief', 'nightly-digest', 'standup-notes'])
  expect(a.marks.filter((m) => !m.filled).map((m) => m.name)).toEqual(['knowledge-sync'])
  // NOW sits at 19:49 on the arc; its label is a little further along.
  expect(a.now.x1).toBeCloseTo(arcPoint(19 + 49 / 60 + 19 / 3600, ARC.R - 22)[0], 3)
})

test('hourOf clamps to the window: yesterday sits at 00:00, the future at the cap', () => {
  expect(hourOf(NOW - 48 * 3_600_000, NOW)).toBe(0)
  expect(hourOf(NOW + 10 * 3_600_000, NOW, 19.5)).toBe(19.5)
  expect(hourOf(startOfDay(NOW) + 6 * 3_600_000, NOW)).toBe(6)
})

test('rings: theuth first, working → waiting → idle, routine, lapsed last; 7 rings on the fixture; geometry', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.attention, NOW)
  expect(rs.map((r) => r.name)).toEqual(['theuth', 'ops-desk', 'research', 'builder', 'analyst', 'docs-desk', 'routine · nightly-digest'])
  expect(rs.map((r) => r.style)).toEqual(['live', 'live', 'live', 'waiting', 'idle', 'lapsed', 'idle'])
  expect(rs[0]!.sub).toBe('2 chats · 40s')
  expect(rs[1]!.sub).toBe('4 open · 1h')
  expect(rs[5]!.sub).toBe('paused')
  expect(rs[6]!.sub).toBe('running · 18m')
  expect(rs.map((r) => [r.x, r.y])).toEqual(rs.map((_, i) => [RING.x, RING.y0 + RING.dy * i]))
  expect(rs[3]!.link).toEqual({ route: 'agent', name: 'builder' })
})

test('rings: blocked without an attention item is idle; unreachable is error; a desk-less agent is just "N open"; beyond 9 fold into "+N more"', () => {
  const agents = [...F.agents, { ...F.agents[3]!, file: { ...F.agents[3]!.file, name: 'lonely' }, status: 'blocked' as const }, { ...F.agents[3]!, file: { ...F.agents[3]!.file, name: 'gone' }, status: 'unreachable' as const }]
  const rs = rings(agents, F.sessions, F.routines, F.attention, NOW)
  expect(rs.find((r) => r.name === 'lonely')?.style).toBe('idle')
  expect(rs.find((r) => r.name === 'gone')).toMatchObject({ style: 'error', sub: 'unreachable' })
  expect(rings([{ ...F.agents[3]!, desk: null }], [], [], [], NOW)[1]!.sub).toBe('1 open')
  const many = Array.from({ length: 12 }, (_, i) => ({ ...F.agents[3]!, file: { ...F.agents[3]!.file, name: `a${i}` } }))
  const folded = rings(many, [], [], [], NOW)
  expect(folded).toHaveLength(9)
  expect(folded[8]).toMatchObject({ name: '+5 more', style: 'lapsed' })
})

test('rings: the roster agent named theuth folds into the sessions ring — one label, its open count on the sub-label, its actor on the ring', () => {
  const self = { ...F.agents[3]!, file: { ...F.agents[3]!.file, name: 'theuth' }, status: 'working' as const, open_tasks: 23 }
  const rs = rings([...F.agents, self], F.sessions, F.routines, F.attention, NOW)
  expect(rs.filter((r) => r.name === 'theuth')).toHaveLength(1)
  expect(rs[0]).toMatchObject({ sub: '2 chats · 23 open · 40s', style: 'live', link: { route: 'agent', name: 'theuth' } })
  expect(ringByActor(rs, 'agent:theuth')).toBe(rs[0])
  // Without chats today the ring still names theuth's work; a working theuth is live even when no chat moved.
  expect(rings([self], [], [], [], NOW)[0]).toMatchObject({ sub: 'no chats today · 23 open', style: 'live' })
})

test('needsYou: newest first, 12 drawn, jitter within ±6 and deterministic, short ids ≤ 10 chars', () => {
  const { needs, more } = needsYou(F.attention)
  expect(needs.map((n) => n.label)).toEqual(['approval', '001198', '001188', 'hire', '001203', 'secret'])
  expect(more).toBe(0)
  for (const [i, n] of needs.entries()) {
    expect(n.x).toBe(610 - 60 * i)
    expect(Math.abs(n.y - (262 + 52 * i))).toBeLessThanOrEqual(6)
  }
  expect(needsYou(F.attention).needs.map((n) => n.y)).toEqual(needs.map((n) => n.y))
  const many = Array.from({ length: 15 }, (_, i) => ({ ...F.attention[0]!, id: `approval:${i}`, at_ms: NOW - i * 1000 }))
  expect(needsYou(many)).toMatchObject({ more: 3 })
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null })).toBe('handoff')
  expect(shortId({ ...F.attention[0]!, task: T('1234') })).toBe('001234')
})

test('outcomes: done kinds + non-error routine fires, newest first, via the actor ring, cost on the label, links', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.attention, NOW)
  const o = outcomes(F.activityToday, F.routines, rs, NOW)
  expect(o.total).toBe(11)
  expect(o.flows[0]!).toMatchObject({ color: C.success, label: 'finished t-1212 · 3 files, … · $0.42', link: { route: 'task', id: T('1212') } })
  expect(o.flows[0]!.via?.name).toBe('ops-desk')
  expect(o.flows.find((f) => f.label.startsWith('nightly'))).toMatchObject({ link: { route: 'routine', name: 'nightly-digest' } })
  expect(o.flows.find((f) => f.label.startsWith('nightly'))?.via).toBeNull()
  expect(o.flows.find((f) => f.label === 'knowledge +2')?.via?.name).toBe('theuth')
  for (let i = 1; i < o.flows.length; i++) expect(o.flows[i - 1]!.time).toBeGreaterThanOrEqual(o.flows[i]!.time)
  const failed = F.routines.map((s) => (s.name === 'morning-brief' ? { ...s, last_result: 'error: boom' } : s))
  expect(outcomes(F.activityToday, failed, rs, NOW).total).toBe(10)
})

test('running: doing tasks via the owner ring at the progress note, label with elapsed', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.attention, NOW)
  const r = running(F.tasks, rs, NOW)
  expect(r.map((f) => f.label)).toEqual(['wire HUD scene into / · running · 12m', 'design 126 review · running · 40m', 'punch card arrivals · running · 55m'])
  expect(r[0]!.via?.name).toBe('ops-desk')
  expect(r[0]!.color).toBe(C.info)
})

test('flowPath: dot → ring edge → arc → end as the mockup draws it', () => {
  const via = { id: 'x', name: 'builder', sub: '', style: 'waiting' as const, x: 860, y: 424, actor: null, link: null }
  const d = flowPath([618, 259.6], via, 19.5, [1620, 82])
  expect(d).toMatch(/^M 618 259\.6 C 732 259\.6, 732 424, 846 424 M 874 424 C [\d.]+ 424, [\d.]+ [\d.]+, [\d.]+ [\d.]+ Q [\d.]+ [\d.]+ 1620 82$/)
  // Without a via the single cubic runs straight to the arc; without a source the line starts at the arc.
  expect(flowPath([618, 259.6], null, 19.5, [1620, 82]).split('M')).toHaveLength(2)
  expect(flowPath(null, null, 12, [1500, 300])).toMatch(/^M 1090 420 Q 1262\.2 384 1500 300$/)
})

test('layoutEnds: ascending by time, ≥ 22 px apart per column, x cycles, overflow drops the oldest', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.attention, NOW)
  const all = [...outcomes(F.activityToday, F.routines, rs, NOW).flows, ...running(F.tasks, rs, NOW)]
  const { flows, more } = layoutEnds(all)
  expect(more).toBe(0)
  expect(flows).toHaveLength(all.length)
  for (let i = 1; i < flows.length; i++) expect(flows[i - 1]!.time).toBeLessThanOrEqual(flows[i]!.time)
  for (const x of END_X) {
    const ys = flows.filter((f) => f.end[0] === x).map((f) => f.end[1]).sort((a, b) => a - b)
    for (let i = 1; i < ys.length; i++) expect(ys[i]! - ys[i - 1]!).toBeGreaterThanOrEqual(LABEL_GAP)
  }
  expect(flows.map((f) => f.end[0]).slice(0, 5)).toEqual([1500, 1580, 1660, 1740, 1500])
  const many = Array.from({ length: 40 }, (_, i) => ({ ...all[0]!, id: `f${i}`, time: NOW - i * 1000 }))
  const over = layoutEnds(many)
  expect(over.more).toBe(7)
  expect(over.flows[0]!.time).toBe(NOW - 32 * 1000)
})

test('hairlines: deterministic, skips drawn outcomes, one per session today, capped at 600', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.attention, NOW)
  const drawn = new Set(outcomes(F.activityToday, F.routines, rs, NOW).flows.map((f) => f.id))
  const st = sessionsToday(F.sessions, NOW)
  const a = hairlines(F.activityToday, st, rs, drawn, NOW)
  const b = hairlines(F.activityToday, st, rs, drawn, NOW)
  expect(a).toEqual(b)
  expect(a).toHaveLength(F.activityToday.length - drawn.size + st.length)
  expect(a.length).toBeGreaterThan(200)
  for (const h of a) {
    expect(h.opacity).toBeGreaterThanOrEqual(0.07)
    expect(h.opacity).toBeLessThanOrEqual(0.26)
  }
  const flood = Array.from({ length: 700 }, (_, i) => ({ ...F.activityToday[20]!, id: 100_000 + i }))
  expect(hairlines(flood, st, rs, new Set(), NOW)).toHaveLength(HAIRLINES_MAX)
  expect(rng('same')()).toBe(rng('same')())
  expect(rng('a')()).not.toBe(rng('b')())
})

test('punchCard: 7×24 by local weekday (Mon = 0), now cell marked', () => {
  const p = punchCard(F.activityWeek, NOW)
  expect(p.cells).toHaveLength(7)
  expect(p.cells.every((r) => r.length === 24)).toBe(true)
  expect(p.cells.flat().reduce((s, n) => s + n, 0)).toBe(F.activityWeek.length)
  expect(p.now).toEqual([2, 19]) // 2026-10-07 is a Wednesday
  expect(p.max).toBe(Math.max(...p.cells.flat()))
})

test('outcomeMix counts today’s moves; running counts every doing task', () => {
  expect(outcomeMix(F.tasks, NOW)).toEqual({ done: 7, running: 3, review: 1, blocked: 0, cancelled: 1 })
})

test('scene: footer numbers and the whole model from the fixture; empty data draws nothing but the chrome', () => {
  const s = scene(F, NOW)
  expect(s.footer).toEqual({ sessionsLive: 3, agentsWorking: 2, needYou: 6, month: 31.2, outcomes: 11, inFlight: 4 })
  expect(s.rings).toHaveLength(7)
  expect(s.needs).toHaveLength(6)
  expect(s.flows).toHaveLength(6 + 11 + 3)
  expect(s.hairlines.length).toBeGreaterThan(200)
  const empty = scene({}, NOW)
  expect(empty.rings).toHaveLength(1)
  expect(empty.flows).toEqual([])
  expect(empty.hairlines).toEqual([])
  expect(empty.footer.needYou).toBeNull() // the inbox has not loaded — not a false 0
})
