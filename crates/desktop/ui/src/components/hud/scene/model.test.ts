import { expect, test } from 'vitest'
import { sceneFixture, T } from './fixtures'
import { ARC, arc, arcPoint, C, contextFill, ctxTitle, dueFlows, END_X, FIELD_H, fieldHeight, flowPath, fmtUsd, gaugeEnd, gaugePath, geom, hairlines, HAIRLINES_MAX, hourOf, kTokens, LABEL_GAP, layoutEnds, localDay, needFlows, needsYou, outcomeMix, outcomes, punchCard, RING, ringByActor, ringByRoutine, ringBySession, rings, rng, running, scene, sessionPresence, sessionsToday, shortId, spendPanel, startOfDay, streamLink } from './model'

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
  expect(a.marks.filter((m) => m.filled).map((m) => m.name).sort()).toEqual(['morning-brief', 'nightly-digest', 'standup-notes'])
  expect(a.marks.filter((m) => !m.filled).map((m) => m.name)).toEqual(['knowledge-sync'])
  // NOW sits at 19:49 on the arc; its label is a little further along.
  expect(a.now.x1).toBeCloseTo(arcPoint(19 + 49 / 60 + 19 / 3600, ARC.R - 22)[0], 3)
})

test('hourOf clamps to the window: yesterday sits at 00:00, the future at the cap', () => {
  expect(hourOf(NOW - 48 * 3_600_000, NOW)).toBe(0)
  expect(hourOf(NOW + 10 * 3_600_000, NOW, 19.5)).toBe(19.5)
  expect(hourOf(startOfDay(NOW) + 6 * 3_600_000, NOW)).toBe(6)
})

test('rings: theuth first, working → waiting → idle, lapsed last; the routine group; the session group (Revision 4); 12 rings on the fixture at hv 812; geometry with two group gaps', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  expect(rs.map((r) => r.name)).toEqual(['theuth', 'ops-desk', 'research', 'builder', 'analyst', 'docs-desk', 'routine · nightly-digest', 'routine · standup-notes', 'routine · morning-brief', 'routine · knowledge-sync', 'HUD visual overhaul', '+3 sessions'])
  expect(rs.map((r) => r.style)).toEqual(['live', 'live', 'live', 'waiting', 'idle', 'lapsed', 'live', 'failed', 'idle', 'lapsed', 'live', 'lapsed'])
  expect(rs[0]!.sub).toBe('4 chats · 40s') // adv-1 and arch-1 are not chats
  expect(rs[1]!.sub).toBe('4 open · 1h')
  expect(rs[5]!.sub).toBe('paused')
  expect(rs.slice(6, 10).map((r) => r.sub)).toEqual(['running · 18m', 'failed 09:00', 'ran 06:00', 'next 23:30'])
  expect(rs.slice(6, 10).map((r) => r.id)).toEqual(['ring:routine:nightly-digest', 'ring:routine:standup-notes', 'ring:routine:morning-brief', 'ring:routine:knowledge-sync'])
  // R4 + Revisions 1/4: the column (11 steps + two 20 px group gaps) is centred on the arc's centre line (420 at hv 812).
  const y0 = 420 - (RING.dy * 11 + 2 * RING.gap) / 2
  expect(rs.map((r) => [r.x, r.y])).toEqual(rs.map((_, i) => [RING.x, y0 + RING.dy * i + (i >= 6 ? RING.gap : 0) + (i >= 10 ? RING.gap : 0)]))
  expect(rs[3]!.link).toEqual({ route: 'agent', name: 'builder' })
  expect(rs[9]!.link).toEqual({ route: 'routine', name: 'knowledge-sync' })
  expect(rs[10]!.link).toEqual({ route: 'session', id: 'chat-1' })
  expect(rs[11]).toMatchObject({ id: 'ring:sessions:more', link: null, ctx: null })
  expect(ringByRoutine(rs, 'morning-brief')?.id).toBe('ring:routine:morning-brief')
  expect(ringByRoutine(rs, 'nope')).toBeNull()
  // A routine that is both running and fired today is one ring (running); one with a fire today and a `next` later today is fired, not due.
  const twice = F.routines.map((s) => (s.name === 'morning-brief' ? { ...s, next: new Date(NOW + 3_600_000).toISOString() } : s))
  expect(rings(F.agents, F.sessions, twice, F.activityToday, F.attention, NOW).filter((r) => r.name.endsWith('morning-brief'))).toMatchObject([{ style: 'idle', sub: 'ran 06:00' }])
  // Without routines or sessions there is no gap: 6 rings evenly centred; with sessions only, one gap.
  const none = rings(F.agents, [], [], [], F.attention, NOW)
  expect(none).toHaveLength(6)
  expect(none.map((r) => r.y)).toEqual(none.map((_, i) => 420 - (RING.dy * 5) / 2 + RING.dy * i))
  const onlySessions = rings(F.agents, F.sessions, [], [], F.attention, NOW)
  expect(onlySessions).toHaveLength(10)
  expect(onlySessions[6]!.y - onlySessions[5]!.y).toBe(RING.dy + RING.gap)
  expect(onlySessions[7]!.y - onlySessions[6]!.y).toBe(RING.dy)
})

test('Revision 4 — sessionPresence: active roots only (no advisor, desk, archived, child or stale root), each with its live descendants at any depth; order running → running children → newest', () => {
  const p = sessionPresence(F.sessions, NOW)
  expect(p.map((r) => r.s.id)).toEqual(['chat-1', 'chat-2', 'chat-3', 'chat-4'])
  expect(p.map((r) => [r.children.length, r.runningChildren])).toEqual([
    [3, 2],
    [0, 0],
    [1, 1],
    [0, 0],
  ])
  expect(p[0]!.children.map((c) => [c.id, c.title, c.running])).toEqual([
    ['child-21', 'w21', true],
    ['child-22', 'w22', true],
    ['child-23', 'w23', false],
  ])
  // A grandchild counts; a stale root with only a live grandchild is active through it; a root whose only child is archived is not.
  const grand = [...F.sessions, { ...F.sessions[0]!, id: 'gc', parent: 'child-c1', updated_at: new Date(NOW - 60_000).toISOString(), running: false }]
  expect(sessionPresence(grand, NOW).find((r) => r.s.id === 'chat-3')?.children.map((c) => c.id)).toEqual(['child-c1', 'gc'])
  const stale = [{ ...F.sessions[2]!, id: 'r', updated_at: new Date(NOW - 3 * 3_600_000).toISOString() }, { ...F.sessions[0]!, id: 'k', parent: 'r', running: false, updated_at: new Date(NOW - 5 * 60_000).toISOString() }]
  expect(sessionPresence(stale, NOW).map((r) => r.s.id)).toEqual(['r'])
  expect(sessionPresence([stale[0]!, { ...stale[1]!, archived: true }], NOW)).toEqual([])
  expect(sessionPresence([stale[0]!], NOW)).toEqual([])
  // 31 minutes is not active; running overrides any age.
  const old = { ...F.sessions[3]!, updated_at: new Date(NOW - 31 * 60_000).toISOString() }
  expect(sessionPresence([old], NOW)).toEqual([])
  expect(sessionPresence([{ ...old, running: true }], NOW)).toHaveLength(1)
})

test('Revision 4 — session rings: title 22 / name / id-8 label, "k running · n children · age" sub, live when running, ctx from the map, children carried; capacity fold "+N sessions" without a link', () => {
  const tall = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW, geom(1400), F.contexts)
  const ss = tall.filter((r) => r.id.startsWith('ring:session:'))
  expect(ss.map((r) => r.id)).toEqual(['ring:session:chat-1', 'ring:session:chat-2', 'ring:session:chat-3', 'ring:session:chat-4'])
  expect(ss.map((r) => r.sub)).toEqual(['2 running · 3 children · 40s · ctx 71 %', '6m · ctx 19 %', '1 running · 1 child · 50m', '12m'])
  expect(ss.map((r) => r.style)).toEqual(['live', 'live', 'idle', 'idle'])
  expect(ss[0]).toMatchObject({ name: 'HUD visual overhaul', actor: null, link: { route: 'session', id: 'chat-1' }, ctx: { fill: 0.71 } })
  expect(ss[0]!.children?.map((c) => c.id)).toEqual(['child-21', 'child-22', 'child-23'])
  expect(ss[1]!.children).toEqual([])
  expect(ss[2]!.ctx).toBeNull()
  expect(ringBySession(tall, 'chat-3')?.id).toBe('ring:session:chat-3')
  expect(ringBySession(tall, null)).toBeNull()
  expect(ringBySession(tall, 'child-21')).toBeNull()
  // Labels: the title truncated to 22, else the name, else the first 8 chars of the id.
  const long = { ...F.sessions[1]!, title: 'a very long session title that keeps going' }
  const named = { ...F.sessions[1]!, id: 'abcdefgh-1234', title: '', name: 'nick' }
  const bare = { ...F.sessions[1]!, id: 'abcdefgh-1234', title: '' }
  expect(rings([], [long, named, bare], [], [], [], NOW).map((r) => r.name)).toEqual(['theuth', 'a very long session t…', 'nick', 'abcdefgh'])
  // At hv 812 the fixture has 2 slots left → one root and "+3 sessions"; agents and routines fold first as before.
  const at812 = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  expect(at812.slice(10).map((r) => r.name)).toEqual(['HUD visual overhaul', '+3 sessions'])
  expect(at812[11]).toMatchObject({ id: 'ring:sessions:more', style: 'lapsed', link: null })
  // No slot at all → the group is not drawn.
  const manyRoutines = Array.from({ length: 12 }, (_, i) => ({ ...F.routines[0]!, name: `r${i}`, running: true }))
  expect(rings(F.agents, F.sessions, manyRoutines, F.activityToday, F.attention, NOW, geom(600)).some((r) => r.id.startsWith('ring:session'))).toBe(false)
  // Liveness proxy (5): a running chat makes the theuth ring live even when nothing moved in 2 min; a running desk makes its agent live.
  const quiet = F.sessions.map((s) => ({ ...s, updated_at: new Date(NOW - 10 * 60_000).toISOString() }))
  expect(rings(F.agents.map((a) => ({ ...a, status: 'idle' as const })), quiet, [], [], [], NOW)[0]!.style).toBe('live')
  expect(rings(F.agents.map((a) => ({ ...a, status: 'idle' as const })), quiet.map((s) => ({ ...s, running: false })), [], [], [], NOW)[0]!.style).toBe('idle')
  const runningDesk = F.sessions.map((s) => (s.id === 'desk-analyst' ? { ...s, running: true } : s))
  expect(rings(F.agents, runningDesk, [], [], F.attention, NOW).find((r) => r.name === 'analyst')?.style).toBe('live')
})

test('rings: capacity max(9, ⌊(hv − 150) / 53⌋) — agents first with their own fold, routines beyond fold into one "+N routines" ring → /routines', () => {
  const many = Array.from({ length: 12 }, (_, i) => ({ ...F.routines[0]!, name: `r${i}`, running: true }))
  expect(RING.cap(600)).toBe(9)
  expect(RING.cap(812)).toBe(12)
  expect(RING.cap(1400)).toBe(23)
  const g600 = geom(600)
  const at600 = rings(F.agents, F.sessions, many, F.activityToday, F.attention, NOW, g600)
  expect(at600).toHaveLength(9)
  expect(at600.slice(6).map((r) => r.name)).toEqual(['routine · r0', 'routine · r1', '+13 routines']) // 12 running + 3 fired today (the fixture's rows), 2 shown
  expect(at600[8]).toMatchObject({ id: 'ring:routines:more', style: 'lapsed', link: { route: 'routines' } })
  expect(at600[0]!.y - RING.r).toBeGreaterThan(40)
  expect(at600[8]!.y + RING.r).toBeLessThan(g600.footer)
  expect((at600[0]!.y + at600[8]!.y) / 2).toBeCloseTo(g600.cy, 6)
  // At hv 1400 the same routines all fit (23 slots) and the four roots take the last two: one shown, "+3 sessions".
  const tall = rings(F.agents, F.sessions, many, F.activityToday, F.attention, NOW, geom(1400))
  expect(tall).toHaveLength(23)
  expect(tall.some((r) => r.id === 'ring:routines:more')).toBe(false)
  expect(tall[22]).toMatchObject({ name: '+3 sessions' })
  // The agent fold is unchanged and comes first: 12 agents + theuth → 9 agent rings, then the routines take the remaining slots.
  const agents = Array.from({ length: 12 }, (_, i) => ({ ...F.agents[3]!, file: { ...F.agents[3]!.file, name: `a${i}` } }))
  const both = rings(agents, [], many, [], [], NOW)
  expect(both).toHaveLength(12)
  expect(both[8]).toMatchObject({ name: '+5 more' })
  expect(both.slice(9).map((r) => r.name)).toEqual(['routine · r0', 'routine · r1', '+10 routines'])
  // A full agent column at hv 600 leaves no slot: the routine group is not drawn (the fold ring would cross the footer).
  expect(rings(agents, [], many, [], [], NOW, g600)).toHaveLength(9)
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
    const rs = rings(nine, [], [], [], [], NOW, g)
    expect((rs[0]!.y + rs[8]!.y) / 2).toBeCloseTo(g.cy, 6)
    expect(rs[0]!.y - RING.r).toBeGreaterThan(40)
    expect(rs[8]!.y + RING.r).toBeLessThan(g.footer)
    // Needs-you: twelve dots cascade from 0.3227·hv in steps ≤ 0.064·hv; the lowest stays above the footer.
    const { needs } = needsYou(twelve, g)
    expect(needs[0]!.y).toBeGreaterThanOrEqual(g.needY0 - 6)
    expect(needs[11]!.y).toBeLessThan(g.footer - 14)
    expect(needs[1]!.y - needs[0]!.y).toBeLessThanOrEqual(g.needDy + 12)
    // Label band: ends within [top, bottom], ≥ 22 apart per column.
    const all = [...outcomes(F.activityToday, F.routines, rs, NOW).flows, ...running(F.tasks, F.routines, F.sessions, rs, NOW)]
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
  const rs = rings(agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  expect(rs.find((r) => r.name === 'lonely')?.style).toBe('idle')
  expect(rs.find((r) => r.name === 'gone')).toMatchObject({ style: 'error', sub: 'unreachable' })
  expect(rings([{ ...F.agents[3]!, desk: null }], [], [], [], [], NOW)[1]!.sub).toBe('1 open')
  const many = Array.from({ length: 12 }, (_, i) => ({ ...F.agents[3]!, file: { ...F.agents[3]!.file, name: `a${i}` } }))
  const folded = rings(many, [], [], [], [], NOW)
  expect(folded).toHaveLength(9)
  expect(folded[8]).toMatchObject({ name: '+5 more', style: 'lapsed' })
})

test('rings: the roster agent named theuth folds into the sessions ring — one label, its open count on the sub-label, its actor on the ring', () => {
  const self = { ...F.agents[3]!, file: { ...F.agents[3]!.file, name: 'theuth' }, status: 'working' as const, open_tasks: 23 }
  const rs = rings([...F.agents, self], F.sessions, F.routines, F.activityToday, F.attention, NOW)
  expect(rs.filter((r) => r.name === 'theuth')).toHaveLength(1)
  expect(rs[0]).toMatchObject({ sub: '4 chats · 23 open · 40s', style: 'live', link: { route: 'new' } }) // R2: the aggregate ring opens a new chat
  expect(ringByActor(rs, 'agent:theuth')).toBe(rs[0])
  // Without chats today the ring still names theuth's work; a working theuth is live even when no chat moved.
  expect(rings([self], [], [], [], [], NOW)[0]).toMatchObject({ sub: 'no chats today · 23 open', style: 'live' })
})

test('Revision 2 — gaugePath: empty at 0, one arc with the large-arc flag flipping above 0.5, two half arcs at 1; the end point walks clockwise from 12 o’clock', () => {
  expect(gaugePath(100, 50, 17, 0)).toBe('')
  expect(gaugePath(100, 50, 17, -0.2)).toBe('')
  expect(gaugePath(100, 50, 17, 0.25)).toBe('M 100 33 A 17 17 0 0 1 117 50')
  expect(gaugePath(100, 50, 17, 0.5)).toBe('M 100 33 A 17 17 0 0 1 100 67')
  expect(gaugePath(100, 50, 17, 0.75)).toBe('M 100 33 A 17 17 0 1 1 83 50')
  expect(gaugePath(100, 50, 17, 1)).toBe('M 100 33 A 17 17 0 1 1 100 67 A 17 17 0 1 1 100 33')
  expect(gaugePath(100, 50, 17, 1.4)).toBe(gaugePath(100, 50, 17, 1))
  expect(gaugeEnd(100, 50, 17, 0)).toEqual([100, 33])
  expect(gaugeEnd(100, 50, 17, 0.25)[0]).toBeCloseTo(117, 9)
  expect(gaugeEnd(100, 50, 17, 1)[1]).toBeCloseTo(33, 9)
})

test('Revision 2 — contextFill: the provider’s last bill over the estimate, context_size over the catalog window, clamped; unknown window → null (no gauge, no fake value)', () => {
  const b = { head_images_tokens: 0, head_text_tokens: 0, tail_messages: 0, tail_tokens: 0, system_tokens: 0, total_estimate: 90_000, last_provider_context: 118_000, calibration: 1, live: true }
  const s = F.sessions.find((x) => x.id === 'desk-ops')!
  const models = [{ id: s.model, name: 'm', provider: 'p', context: 200_000, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null }]
  expect(contextFill(b, s, models)).toEqual({ fill: 0.59, used: 118_000, window: 200_000, live: true })
  expect(contextFill({ ...b, last_provider_context: null }, s, models)).toMatchObject({ fill: 0.45, used: 90_000 })
  expect(contextFill(b, { ...s, context_size: 128_000 }, models)).toMatchObject({ fill: 118 / 128, window: 128_000 })
  expect(contextFill(b, { ...s, context_size: 128_000 }, undefined)).toMatchObject({ window: 128_000 })
  expect(contextFill({ ...b, last_provider_context: 250_000 }, s, models)!.fill).toBe(1)
  expect(contextFill(b, s, undefined)).toBeNull()
  expect(contextFill(b, { ...s, model: 'unknown/model' }, models)).toBeNull()
  expect(contextFill(b, s, [{ ...models[0]!, context: 0 }])).toBeNull()
  expect(kTokens(8_240)).toBe('8.2k')
  expect(kTokens(118_400)).toBe('118k')
  expect(ctxTitle({ fill: 0.59, used: 118_000, window: 200_000, live: true })).toBe('context ≈ 118k of 200k (59 %)')
})

test('Revision 2 — ring.ctx: set from the desk session’s entry, null without a desk session or an entry, null on routine/fold rings; the sub-label appends "· ctx N %"', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW, geom(), F.contexts)
  const by = (name: string) => rs.find((r) => r.name === name)!
  expect(by('ops-desk')).toMatchObject({ ctx: { fill: 0.62, used: 124_000, window: 200_000, live: true }, sub: '4 open · 1h · ctx 62 %' })
  expect(by('builder').ctx?.fill).toBe(0.88)
  expect(by('analyst').ctx).toBeNull() // a desk, but no breakdown in the map
  expect(by('analyst').sub).toBe('1 open · 6m')
  expect(by('docs-desk').ctx).toBeNull() // no desk at all
  expect(by('theuth').ctx).toBeNull()
  for (const r of rs.filter((r) => r.id.startsWith('ring:routine'))) expect(r.ctx).toBeNull()
  // Without the map every ring is ctx-less and the labels are as before.
  const plain = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  expect(plain.every((r) => r.ctx === null)).toBe(true)
  expect(plain.find((r) => r.name === 'ops-desk')!.sub).toBe('4 open · 1h')
  // The desk is found by `SessionSummary.desk === name` when the roster row has no desk id.
  const noId = F.agents.map((a) => (a.file.name === 'ops-desk' ? { ...a, desk: null } : a))
  expect(rings(noId, F.sessions, F.routines, F.activityToday, F.attention, NOW, geom(), F.contexts).find((r) => r.name === 'ops-desk')!.ctx?.fill).toBe(0.62)
  // The fold ring carries no gauge.
  const many = Array.from({ length: 12 }, (_, i) => ({ ...F.agents[3]!, file: { ...F.agents[3]!.file, name: `a${i}` } }))
  expect(rings(many, [], [], [], [], NOW, geom(), F.contexts)[8]!.ctx).toBeNull()
  expect(scene(F, NOW).rings.filter((r) => r.ctx).map((r) => r.name)).toEqual(['ops-desk', 'research', 'builder', 'HUD visual overhaul']) // Revision 4: roots too
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

test('outcomes: done kinds + non-error routine fires, newest first, via the actor ring — a routine fire via ITS routine ring —, cost on the label, links', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  const o = outcomes(F.activityToday, F.routines, rs, NOW)
  expect(o.total).toBe(9) // standup-notes failed → not an outcome
  expect(o.flows[0]!).toMatchObject({ color: C.success, label: 'finished t-1212 · 3 files, … · $0.42', link: { route: 'task', id: T('1212') } })
  expect(o.flows[0]!.via?.name).toBe('ops-desk')
  expect(o.flows.find((f) => f.label.startsWith('nightly'))).toMatchObject({ link: { route: 'routine', name: 'nightly-digest' }, via: { id: 'ring:routine:nightly-digest' } })
  expect(o.flows.find((f) => f.label.startsWith('morning'))?.via?.id).toBe('ring:routine:morning-brief')
  expect(o.flows.find((f) => f.label === 'knowledge +2')?.via?.name).toBe('theuth')
  for (let i = 1; i < o.flows.length; i++) expect(o.flows[i - 1]!.time).toBeGreaterThanOrEqual(o.flows[i]!.time)
  const failed = F.routines.map((s) => (s.name === 'morning-brief' ? { ...s, last_result: 'error: boom' } : s))
  expect(outcomes(F.activityToday, failed, rs, NOW).total).toBe(8)
  // Revision 1: every routine_fired flow's path starts at its ring's right edge, not at the arc.
  const { flows } = layoutEnds(o.flows)
  for (const f of flows.filter((f) => f.link?.route === 'routine')) expect(f.d.startsWith(`M ${f.via!.x + 14} `)).toBe(true)
  // With the ring folded away the fire still draws, from the arc as before.
  const noRing = outcomes(F.activityToday, F.routines, rs.slice(0, 6), NOW).flows.find((f) => f.label.startsWith('morning'))!
  expect(noRing.via).toBeNull()
  expect(layoutEnds([noRing]).flows[0]!.d.startsWith(`M ${Math.round(arcPoint(noRing.hour)[0] * 10) / 10} `)).toBe(true)
})

test('running: doing tasks via the owner ring at the progress note, running routines via their own ring at `last`, newest first, label with elapsed', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW, geom(1400))
  const r = running(F.tasks, F.routines, F.sessions, rs, NOW)
  expect(r.map((f) => f.label)).toEqual(['HUD visual overhaul · running', 'release notes format · running', 'wire HUD scene into / · running · 12m', 'running · 18m', 'design 126 review · running · 40m', 'punch card arrivals · running · 55m'])
  expect(r[2]!.via?.name).toBe('ops-desk')
  expect(r[2]!.color).toBe(C.info)
  expect(r[3]).toMatchObject({ id: 'running:routine:nightly-digest', color: C.info, via: { id: 'ring:routine:nightly-digest' }, link: { route: 'routine', name: 'nightly-digest' }, title: 'nightly-digest · running · 18m' })
  expect(r[3]!.hour).toBeCloseTo(hourOf(NOW - 18 * 60_000, NOW), 9)
  // Revision 4: a running root draws its own in-flight line from its ring at `updated_at`; its running children draw none.
  expect(r[0]).toMatchObject({ id: 'running:session:chat-1', color: C.info, via: { id: 'ring:session:chat-1' }, link: { route: 'session', id: 'chat-1' }, time: NOW - 40_000, title: 'HUD visual overhaul · running · 40s' })
  expect(r[1]).toMatchObject({ id: 'running:session:chat-2', via: { id: 'ring:session:chat-2' } })
  expect(r.some((f) => f.id.includes('child'))).toBe(false)
  // Folded away at hv 812 → chat-2's line still draws, from the arc.
  expect(running(F.tasks, F.routines, F.sessions, rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW), NOW).find((f) => f.id === 'running:session:chat-2')?.via).toBeNull()
  // No `last` yet → timed at now.
  const fresh = running([], [{ ...F.routines[2]!, last: null }], [], rs, NOW)
  expect(fresh[0]).toMatchObject({ time: NOW, label: 'running · 0s' })
})

test('Revision 4 — needFlows: an item born in a root session leaves that ring; the agent lookup stays for desks; hairlines of a ringed session leave it', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  const { needs } = needsYou(F.attention)
  const flows = needFlows(needs, rs, NOW)
  expect(flows.find((f) => f.id === 'need:secret:gh-token')?.via?.id).toBe('ring:session:chat-1')
  expect(flows.find((f) => f.id === 'need:approval:ap-4821')?.via?.name).toBe('builder')
  // Session first, agent second — an item with both takes the session ring; a session without a ring falls back to the agent.
  const both = needsYou([{ ...F.attention[0]!, session: 'chat-1' }])
  expect(needFlows(both.needs, rs, NOW)[0]!.via?.id).toBe('ring:session:chat-1')
  const noRing = needsYou([{ ...F.attention[0]!, session: 'chat-4' }])
  expect(needFlows(noRing.needs, rs, NOW)[0]!.via?.name).toBe('builder')
  // Hairline: chat-1 (ringed) starts at its ring on a ring-sourced seed; chat-4 (folded) falls back to the theuth ring.
  const chat1 = F.sessions.find((s) => s.id === 'chat-1')!
  const ring = ringBySession(rs, 'chat-1')!
  const seeds = Array.from({ length: 40 }, (_, i) => ({ ...chat1, id: `chat-1-${i}` })).filter((s) => rng(`hair:s:${s.id}`)() < 0.62)
  expect(seeds.length).toBeGreaterThan(0)
  for (const s of seeds) {
    const [h] = hairlines([], [s], [...rs, { ...ring, id: `ring:session:${s.id}` }], new Set(), NOW)
    expect(Math.abs(Number(/^M [\d.]+ ([\d.]+)/.exec(h!.d)![1]) - ring.y)).toBeLessThanOrEqual(3)
  }
})

test('dueFlows: one faint connector per due-later routine, from its ring to exactly the hollow square, no label; nothing for running/fired/ringless routines', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  const d = dueFlows(F.routines, F.activityToday, rs, NOW)
  expect(d).toHaveLength(1)
  const f = d[0]!
  const ring = ringByRoutine(rs, 'knowledge-sync')!
  expect(f).toMatchObject({ id: 'due:knowledge-sync', kind: 'due', color: C.tertiary, label: '', via: { id: ring.id }, link: { route: 'routine', name: 'knowledge-sync' } })
  expect(f.d.startsWith(`M ${ring.x + 14} ${ring.y}`)).toBe(true)
  expect(f.d).not.toContain('Q')
  const square = arc(NOW, F.routines, F.activityToday).marks.find((k) => !k.filled)!
  expect(f.end).toEqual([square.x, square.y])
  expect(square.name).toBe('knowledge-sync')
  // The connector follows the field height like the square does.
  const g = geom(1100)
  const tall = dueFlows(F.routines, F.activityToday, rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW, g), NOW, g)[0]!
  const tallSquare = arc(NOW, F.routines, F.activityToday, g).marks.find((k) => !k.filled)!
  expect(tall.end).toEqual([tallSquare.x, tallSquare.y])
  // Folded away → no connector.
  expect(dueFlows(F.routines, F.activityToday, rs.slice(0, 6), NOW)).toEqual([])
})

test('flowPath: dot → ring edge → arc → end as the mockup draws it', () => {
  const via = { id: 'x', name: 'builder', sub: '', style: 'waiting' as const, x: 860, y: 424, actor: null, link: null, ctx: null }
  const d = flowPath([618, 259.6], via, 19.5, [1620, 82])
  expect(d).toMatch(/^M 618 259\.6 C 732 259\.6, 732 424, 846 424 M 874 424 C [\d.]+ 424, [\d.]+ [\d.]+, [\d.]+ [\d.]+ Q [\d.]+ [\d.]+ 1620 82$/)
  // Without a via the single cubic runs straight to the arc; without a source the line starts at the arc.
  expect(flowPath([618, 259.6], null, 19.5, [1620, 82]).split('M')).toHaveLength(2)
  expect(flowPath(null, null, 12, [1500, 300])).toMatch(/^M 1090 420 Q 1262\.2 384 1500 300$/)
})

test('layoutEnds: ascending by time, ≥ 22 px apart per column, x cycles, overflow drops the oldest', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  const all = [...outcomes(F.activityToday, F.routines, rs, NOW).flows, ...running(F.tasks, F.routines, F.sessions, rs, NOW)]
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
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
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

test('hairlines: a routine_fired row that is not an outcome leaves its routine ring when the seed says ring-sourced', () => {
  const rs = rings(F.agents, F.sessions, F.routines, F.activityToday, F.attention, NOW)
  const fail = F.activityToday.find((e) => e.kind === 'routine_fired' && e.subject === 'standup-notes')!
  const ring = ringByRoutine(rs, 'standup-notes')!
  // Find a seed that picks the ring branch (r() < 0.62) by trying ids; the first draw of the id's rng decides it.
  const ids = Array.from({ length: 40 }, (_, i) => fail.id + 1000 * i).filter((id) => rng(`hair:a:${id}`)() < 0.62)
  expect(ids.length).toBeGreaterThan(0)
  for (const id of ids) {
    const [h] = hairlines([{ ...fail, id }], [], rs, new Set(), NOW)
    const y = Number(/^M [\d.]+ ([\d.]+)/.exec(h!.d)![1])
    expect(h!.d.startsWith(`M ${RING.x + 14} `)).toBe(true)
    expect(Math.abs(y - ring.y)).toBeLessThanOrEqual(3)
  }
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

test('scene: footer numbers (IN FLIGHT counts the running routine) and the whole model from the fixture; empty data draws nothing but the chrome', () => {
  const s = scene(F, NOW)
  // SESSIONS LIVE: running first, 15-min recency as the fallback (chat-1/2/4, child-21/22/23, child-c1, desk-analyst, adv-1; arch-1 never).
  expect(s.footer).toEqual({ sessionsLive: 9, agentsWorking: 2, needYou: 6, month: 4102, outcomes: 9, inFlight: 3 + 1 + 2 }) // Revision 3: month from the ledger
  expect(s.rings).toHaveLength(12)
  expect(s.needs).toHaveLength(6)
  expect(s.flows).toHaveLength(6 + 9 + 6 + 1) // needs, outcomes, running (3 tasks + nightly-digest + 2 roots), the due connector
  expect(s.flows.filter((f) => f.kind === 'due').map((f) => f.id)).toEqual(['due:knowledge-sync'])
  expect(s.flows.find((f) => f.id === 'running:routine:nightly-digest')?.via?.id).toBe('ring:routine:nightly-digest')
  expect(s.hairlines.length).toBeGreaterThan(200)
  expect(scene({ ...F, routines: F.routines.filter((r) => !r.running) }, NOW).footer.inFlight).toBe(5)
  expect(scene({ ...F, sessions: F.sessions.map((x) => ({ ...x, running: false })) }, NOW).footer.inFlight).toBe(4)
  const empty = scene({}, NOW)
  expect(empty.rings).toHaveLength(1)
  expect(empty.flows).toEqual([])
  expect(empty.hairlines).toEqual([])
  expect(empty.footer.needYou).toBeNull() // the inbox has not loaded — not a false 0
})

test('spendPanel: seven bars with weekday letters and today flagged last, month/today numbers, owners capped at 4, caps only with a budget', () => {
  const p = spendPanel(F.spend, F.agents, NOW)
  expect(p.days).toHaveLength(7)
  // 2026-10-07 is a Wednesday: the week reads t f s s m t w, today last.
  expect(p.days.map((d) => d.label).join('')).toBe('tfssmtw')
  expect(p.days.map((d) => d.today)).toEqual([false, false, false, false, false, false, true])
  expect(p.days[3]!.usd).toBe(0) // a $0 day is a bar of nothing, still a day
  expect(p.today).toBe(1302.38)
  expect(p.month).toBe(4102)
  expect(p.owners.map((o) => [o.kind, o.name])).toEqual([
    ['user', 'you'],
    ['agent', 'builder'],
    ['agent', 'research'],
    ['routine', 'nightly-digest'],
  ])
  // analyst has no cap → no cap row; docs-desk is paused but capped → still a row.
  expect(p.caps.map((c) => c.name)).toEqual(['docs-desk', 'builder', 'research', 'ops-desk'])
  expect(p.caps.every((c) => c.cap !== null)).toBe(true)
  // A report longer than a week keeps the last seven; a missing report is a week of $0 ending today.
  const long = spendPanel({ ...F.spend, days: [{ day: '2026-09-30', usd: 9, calls: 1, unresolved: 0 }, ...F.spend.days] }, [], NOW)
  expect(long.days).toHaveLength(7)
  expect(long.days[6]!.today).toBe(true)
  const none = spendPanel(undefined, F.agents, NOW)
  expect(none.days.map((d) => d.usd)).toEqual([0, 0, 0, 0, 0, 0, 0])
  expect(none.days[6]).toEqual({ label: 'w', usd: 0, today: true })
  expect([none.month, none.today, none.owners]).toEqual([0, 0, []])
  expect(localDay(NOW)).toBe('2026-10-07')
  expect(fmtUsd(1302.38)).toBe('$1,302.38')
  expect(fmtUsd(0)).toBe('$0.00')
})
