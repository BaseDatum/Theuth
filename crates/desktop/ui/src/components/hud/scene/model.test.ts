import { expect, test } from 'vitest'
import { sceneFixture, T } from './fixtures'
import { ARC, arc, arcPoint, C, END_X, FIELD_H, fieldHeight, flowPath, geom, hairlines, HAIRLINES_MAX, hourOf, LABEL_GAP, layoutEnds, needsYou, outcomeMix, outcomes, punchCard, RING, ringByActor, rings, rng, running, scene, sessionsToday, shortId, startOfDay, streamLink } from './model'

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
  // R4: the column is centred on the arc's centre line (420 at hv 812): 7 rings → 261, 314, …
  expect(rs.map((r) => [r.x, r.y])).toEqual(rs.map((_, i) => [RING.x, 420 - (RING.dy * 6) / 2 + RING.dy * i]))
  expect(rs[3]!.link).toEqual({ route: 'agent', name: 'builder' })
})

test('R4 geometry: hv 812 is the mockup; at 600 / 1100 / 1400 the arc, rings, needs, label band and hairlines all scale and stay inside the field', () => {
  const g812 = geom(FIELD_H)
  expect(g812).toMatchObject({ cy: 420, top: 60, bottom: 775, srcTop: 250, srcBottom: 690, needY0: 262, needDy: 52, footer: 778 })
  expect((g812.thetaMax * 180) / Math.PI).toBeCloseTo(14, 9)
  expect(scene(F, NOW, FIELD_H)).toEqual(scene(F, NOW)) // the default is the mockup
  // The viewBox rule: 1920 · h / w, clamped to 600–1400; an unmeasured box is the mockup's 812.
  expect(fieldHeight(1600, 700)).toBe(840)
  expect(fieldHeight(1600, 100)).toBe(600)
  expect(fieldHeight(1000, 1000)).toBe(1400)
  expect(fieldHeight(0, 500)).toBe(FIELD_H)
  const twelve = Array.from({ length: 12 }, (_, i) => ({ ...F.attention[0]!, id: `n${i}`, at_ms: NOW - i * 60_000 }))
  const nine = Array.from({ length: 12 }, (_, i) => ({ ...F.agents[3]!, file: { ...F.agents[3]!.file, name: `a${i}` } }))
  for (const hv of [600, 812, 1100, 1400]) {
    const g = geom(hv)
    const k = hv / FIELD_H
    // Arc: centre scales, the band spans the same share of the height (≈ 82 %) and stays clear of the top bar and footer.
    expect(g.cy).toBeCloseTo(420 * k, 6)
    const [, topY] = arcPoint(0, ARC.R - ARC.band, g)
    const [, bottomY] = arcPoint(24, ARC.R + ARC.band, g)
    expect(topY).toBeGreaterThan(40)
    expect(bottomY).toBeLessThan(g.footer)
    expect((bottomY - topY) / hv).toBeGreaterThan(0.78)
    expect(arcPoint(12, ARC.R, g)[0]).toBeCloseTo(1090, 6) // x at noon is the ring-to-arc span the flows are drawn for
    // Rings: nine of them centred on cy, all inside the field.
    const rs = rings(nine, [], [], [], NOW, g)
    expect((rs[0]!.y + rs[8]!.y) / 2).toBeCloseTo(g.cy, 6)
    expect(rs[0]!.y - RING.r).toBeGreaterThan(40)
    expect(rs[8]!.y + RING.r).toBeLessThan(g.footer)
    // Needs-you: twelve dots cascade from 0.3227·hv in steps ≤ 0.064·hv; the lowest stays above the footer.
    const { needs } = needsYou(twelve, g)
    expect(needs[0]!.y).toBeGreaterThanOrEqual(g.needY0 - 6)
    expect(needs[11]!.y).toBeLessThan(g.footer - 14)
    expect(needs[1]!.y - needs[0]!.y).toBeLessThanOrEqual(g.needDy + 12)
    // Label band: ends within [top, bottom], ≥ 22 apart per column.
    const all = [...outcomes(F.activityToday, F.routines, rs, NOW).flows, ...running(F.tasks, rs, NOW)]
    const { flows } = layoutEnds(all, g)
    for (const f of flows) {
      expect(f.end[1]).toBeGreaterThanOrEqual(g.top - 1e-6)
      expect(f.end[1]).toBeLessThanOrEqual(g.bottom + 1e-6)
    }
    for (const x of END_X) {
      const ys = flows.filter((f) => f.end[0] === x).map((f) => f.end[1]).sort((a, b) => a - b)
      for (let i = 1; i < ys.length; i++) expect(ys[i]! - ys[i - 1]!).toBeGreaterThanOrEqual(LABEL_GAP)
    }
    // Hairlines end inside the same band.
    for (const h of hairlines(F.activityToday, sessionsToday(F.sessions, NOW), rs, new Set(), NOW, g)) {
      const ey = Number(/ ([\d.]+)$/.exec(h.d)![1])
      expect(ey).toBeGreaterThanOrEqual(g.top - 0.05)
      expect(ey).toBeLessThanOrEqual(g.bottom + 0.05)
    }
    expect(scene(F, NOW, hv).hv).toBe(hv)
  }
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
  expect(rs[0]).toMatchObject({ sub: '2 chats · 23 open · 40s', style: 'live', link: { route: 'new' } }) // R2: the aggregate ring opens a new chat
  expect(ringByActor(rs, 'agent:theuth')).toBe(rs[0])
  // Without chats today the ring still names theuth's work; a working theuth is live even when no chat moved.
  expect(rings([self], [], [], [], NOW)[0]).toMatchObject({ sub: 'no chats today · 23 open', style: 'live' })
})

test('streamLink: a row opens its subject — tasks for task/child/escalation kinds, agents for agent/budget kinds, routines for fires, nothing otherwise', () => {
  const e = (kind: Parameters<typeof streamLink>[0]['kind'], subject: string) => streamLink({ id: 1, ts_ms: NOW, kind, actor: 'system', subject, summary: '', cost: null })
  expect(e('task_done', T('1212'))).toEqual({ route: 'task', id: T('1212') })
  expect(e('child_spawned', T('1300'))).toEqual({ route: 'task', id: T('1300') })
  expect(e('escalation_raised', T('1301'))).toEqual({ route: 'task', id: T('1301') })
  expect(e('agent_hired', 'builder')).toEqual({ route: 'agent', name: 'builder' })
  expect(e('budget_threshold', 'ops-desk')).toEqual({ route: 'agent', name: 'ops-desk' })
  expect(e('routine_fired', 'morning-brief')).toEqual({ route: 'routine', name: 'morning-brief' })
})

test('needsYou: newest first, 12 drawn, jitter within ±6 and deterministic, short ids ≤ 8 chars, hex prefixes skipped, never a bare repeat of the kind', () => {
  const { needs, more } = needsYou(F.attention)
  // R3/R5: task-born → the task's short id; else the agent named; else the title's first word; hex-like candidates skipped; cut to 8.
  expect(needs.map((n) => n.label)).toEqual(['builder', '001198', '001188', 'ops-desk', '001203', 'GITHUB_T'])
  expect(more).toBe(0)
  for (const [i, n] of needs.entries()) {
    expect(n.x).toBe(610 - 60 * i)
    expect(Math.abs(n.y - (262 + 52 * i))).toBeLessThanOrEqual(6)
  }
  expect(needsYou(F.attention).needs.map((n) => n.y)).toEqual(needs.map((n) => n.y))
  const many = Array.from({ length: 15 }, (_, i) => ({ ...F.attention[0]!, id: `approval:${i}`, at_ms: NOW - i * 1000 }))
  expect(needsYou(many)).toMatchObject({ more: 3 })
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null, agent: null, title: 'macbook-pro hand-off' })).toBe('macbook-')
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null, agent: 'ops-desk' })).toBe('ops-desk')
  expect(shortId({ ...F.attention[0]!, task: T('1234') })).toBe('001234')
  // A show/ask-style item carries a session prefix as its agent and title word: both are hex-like, so the kind word stands.
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null, agent: '01a115f9', title: '01a115f9 showed a chart' })).toBe('handoff')
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null, agent: 'deadbeef', title: 'Which region?' })).toBe('Which')
  expect(shortId({ ...F.attention[0]!, kind: 'handoff', task: null, agent: 'abc12', title: 'x' })).toBe('abc12') // five hex chars is a name, not a prefix
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
