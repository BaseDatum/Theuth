import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import { sceneFixture, T } from './fixtures'
import { scene } from './model'
import { Scene } from './Scene'
import { SceneCompact, SceneSwitch } from './SceneCompact'

// A `Link` needs a router; a plain anchor with the params substituted and the search appended does not, and lets the hrefs be asserted.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ to, params, search, children, className, style }: { to: string; params?: Record<string, string>; search?: Record<string, string>; children: ReactNode; className?: string; style?: object }) => {
    const q = search && Object.keys(search).length > 0 ? `?${new URLSearchParams(search).toString()}` : ''
    return createElement('a', { href: to.replace(/\$(\w+)/g, (_, k: string) => params?.[k] ?? '') + q, className, style }, children)
  },
}))

// The app's one breakpoint; flipped per test to prove both branches of `SceneSwitch`.
const mobile = vi.hoisted(() => ({ value: true }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => mobile.value }))
const NOW = new Date(2026, 9, 7, 19, 49, 19).getTime()
const F = sceneFixture(NOW)
const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

test('Scene draws the fixture: rings, needs, flows, panels, links, tokens only', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW }))
  const m = scene(F, NOW)
  // No literal colour anywhere — every fill/stroke is a token.
  expect(html).not.toMatch(/(fill|stroke)="#/)
  expect(html).toContain('stroke="var(--fg-accent)"')
  expect(html).toContain('stroke="var(--status-success-fg)"')
  // 12 rings (r=13), 6 needs-you dots (r=7), one flow per laid-out flow, each hover-keyed.
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(12) // 6 agents + 4 routines + 1 root session + the "+3 sessions" fold (Revision 4)
  expect(count(html, /<circle[^>]*r="7"/g)).toBe(6)
  expect(count(html, /data-flow="/g)).toBe(m.flows.length)
  // Only live rings pulse; motion is a class (styles.css turns it off under prefers-reduced-motion), never inline.
  expect(count(html, /class="hud-pulse"/g)).toBe(m.rings.filter((r) => r.style === 'live').length)
  expect(html).not.toContain('animation')
  // Revision 2: a desk ring with a breakdown gets a track (r=17, border token) + a sweep (width 2, round cap, faience when live, secondary otherwise)
  // + an end dot; rings without ctx (analyst, docs-desk, theuth, every routine) get nothing. Never gold, never carnelian.
  expect(count(html, /data-gauge="/g)).toBe(4) // three desks + the shown root (Revision 4)
  expect(count(html, /<circle[^>]*r="17"/g)).toBe(4)
  const gauge = (id: string) => new RegExp(`<g data-gauge="${id}">(.*?)</g>`).exec(html)?.[1]
  const ops = gauge('ring:agent:ops-desk')!
  expect(ops).toMatch(/<circle[^>]*r="17" fill="none" stroke="var\(--border-default\)" stroke-width="1">/)
  expect(ops).toMatch(/<path d="M 860 [\d.]+ A 17 17 0 1 1 [\d.]+ [\d.]+" fill="none" stroke="var\(--status-info-fg\)" stroke-width="2" stroke-linecap="round">/) // 62 % → large arc, live → faience
  expect(ops).toMatch(/<circle[^>]*r="1.6" fill="var\(--status-info-fg\)">/)
  const builder = gauge('ring:agent:builder')!
  expect(builder).toContain('stroke="var(--fg-secondary)"') // waiting (gold ring) → the gauge stays secondary
  expect(builder).not.toContain('var(--fg-accent)')
  expect(gauge('ring:agent:analyst')).toBeUndefined()
  expect(gauge('ring:routine:nightly-digest')).toBeUndefined()
  expect(gauge('ring:theuth')).toBeUndefined()
  // Revision 4: satellites — chat-1's 3 live children on r 23 from 3 o'clock (2 filled faience running, 1 hollow secondary idle), titled; its labels step left to x−30.
  const sats = /<g data-satellites="ring:session:chat-1">(.*?)<\/g>/.exec(html)![1]!
  expect(count(sats, /<circle[^>]*r="2.2"/g)).toBe(3)
  expect(count(sats, /fill="var\(--status-info-fg\)"/g)).toBe(2)
  expect(count(sats, /fill="none" stroke="var\(--fg-secondary\)"/g)).toBe(1)
  expect(sats).toContain('<title>w21 · running</title>')
  expect(sats).toContain('<title>w23 · idle</title>')
  expect(sats).not.toContain('+')
  const root = m.rings.find((r) => r.id === 'ring:session:chat-1')!
  expect(sats).toContain(`<circle cx="${root.x + 23}" cy="${root.y}"`) // the first sits at 3 o'clock
  expect(html).toContain(`<text x="${root.x - 30}" y="${root.y - 1}"`)
  expect(count(html, /data-satellites="/g)).toBe(1) // the fold ring and every other ring draw none
  expect(html).toContain('href="/c/chat-1"')
  expect(html).toContain(`>${scene(F, NOW - (NOW % 60_000)).rings.find((r) => r.id === root.id)!.sub}<`) // the Scene clock is minute-floored: 21s here, 40s at NOW
  expect(root.sub).toBe('2 running · 3 children · 40s · ctx 71 %')
  expect(html).toContain('>4 open · 1h · ctx 62 %<')
  expect(html).toContain('<title>ops-desk · 4 open · 1h · ctx 62 % · context ≈ 124k of 200k (62 %)</title>')
  // Links (R2: every destination the old sections offered): attention rows and header, task flows, agent rings, routine marks,
  // the theuth ring and SESSIONS LIVE → /new, the footer's split right slot, the OUTCOMES legend/backlog/board, SPEND and AGENTS big numbers.
  expect(html).toContain('href="/attention/approval:ap-4821"')
  expect(html).toContain(`href="/tasks/${T('1212')}"`)
  expect(html).toContain('href="/agents/builder"')
  expect(html).toContain('href="/routines/morning-brief"')
  expect(html).toContain('href="/routines/standup-notes"') // the failed routine's ring: hollow carnelian, not dashed, sub "failed 09:00"
  expect(html).toMatch(/<circle[^>]*r="13"[^>]*stroke="var\(--status-danger-fg\)" stroke-width="1.2"><\/circle>/)
  expect(html).not.toMatch(/<circle[^>]*stroke="var\(--status-danger-fg\)"[^>]*stroke-dasharray/)
  expect(html).toContain('>failed 09:00<')
  expect(html).toContain('>ran 06:00<')
  expect(html).toContain('>next 23:30<')
  expect(html).toContain('>running · 18m<')
  // Revision 1: every routine_fired flow leaves its routine ring (`M <ring.x+14>`), the running routine has an in-flight line, the due routine
  // a dashed faint connector with no label and no end dot.
  for (const fl of m.flows) {
    if (fl.link?.route !== 'routine') continue
    expect(fl.via?.id).toBe(`ring:routine:${fl.link.name}`)
    expect(fl.d.startsWith(`M ${fl.via!.x + 14} `)).toBe(true)
  }
  const due = m.flows.find((f) => f.kind === 'due')!
  const dueG = new RegExp(`<g data-flow="${due.id}"[^>]*>(.*?)</g>`).exec(html)![1]!
  expect(dueG).toMatch(/<path[^>]*stroke="var\(--fg-tertiary\)"[^>]*stroke-opacity="0.5"[^>]*stroke-dasharray="3 3"/)
  expect(dueG).not.toContain('<text')
  expect(dueG).not.toContain('<circle')
  expect(html).toMatch(/<g data-flow="running:routine:nightly-digest"[^>]*>.*?<text[^>]*>running · 18m<\/text>/)
  expect(html).toMatch(/<a href="\/attention"><text x="28" y="60"/)
  expect(count(html, /href="\/new"/g)).toBe(3) // the theuth ring, its AGENTS row, SESSIONS LIVE
  expect(html).toMatch(/href="\/tasks\?state=done&amp;project="><tspan>9 OUTCOMES TODAY<\/tspan>/)
  expect(html).toMatch(/href="\/tasks\?state=doing&amp;project="><tspan>6 IN FLIGHT<\/tspan>/) // 3 doing tasks + the running routine + 2 running roots
  for (const s of ['done', 'doing', 'review', 'blocked', 'cancelled', 'backlog', 'todo']) expect(html).toContain(`href="/tasks?state=${s}&amp;project="`)
  expect(html).toMatch(/<a href="\/tasks"><text x="340" y="236"/) // the OUTCOMES big number → the board
  expect(count(html, /<a href="\/agents"><text x="340" y="236"/g)).toBe(2) // AGENTS and SPEND big numbers
  expect(html).toContain('href="/agents/ops-desk"')
  // STREAM: an HTML box holding every feed row, each linked to its subject by kind.
  expect(html).toMatch(/<section aria-label="Stream"/)
  expect(count(html, /<li title="/g)).toBe(F.activity.length)
  const done = F.activity.find((e) => e.kind === 'task_done')!
  expect(html).toContain(`href="/tasks/${done.subject}"`)
  expect(html).toContain(`>${F.activity.length}/50<`)
  // Text: the top bar, the clock, the NEEDS YOU table, footer numbers, panel titles.
  expect(html).toContain('T H E U T H')
  expect(html).toContain('● 19:49:19')
  expect(html).toContain('>06<')
  expect(html).toContain('06 NEED YOU')
  expect(html).toContain('>9 OUTCOMES TODAY<')
  expect(html).toContain('>6 IN FLIGHT<')
  expect(html).toContain('$4,102.00 THIS MONTH') // Revision 3: the ledger's month, not the roster's budget sum
  for (const t of ['AGENTS', 'ARRIVALS', 'OUTCOMES', 'SPEND', 'STREAM']) expect(html).toContain(`>${t}<`)
  expect(html).toContain('>19:49<')
  // Accessibility: the field is one image with a title and a summary; the band is HTML (four SVG panels + the stream box).
  expect(html).toMatch(/<svg[^>]*viewBox="0 0 1920 812"[^>]*role="img"[^>]*aria-labelledby="hud-scene-title hud-scene-desc"/)
  expect(count(html, /viewBox="0 0 352 248"/g)).toBe(4)
  expect(html).toContain('6 need you · 2 agents working')
})

test('Revision 3 — SPEND: month big number, TODAY meta, seven bars (today faience, $0 day a hairline), owner rows with glyphs and links, cap rows only for budgeted agents, no gold/carnelian', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW }))
  const panel = /<text x="24" y="19"[^>]*>SPEND<\/text>(.*?)<\/g><\/svg>/.exec(html)![1]!
  expect(panel).toContain('>TODAY $1,302.38<')
  expect(panel).toMatch(/<text x="340" y="236"[^>]*>\$4,102\.00<\/text>/)
  // Seven bars 28 wide; six secondary, today faience; the $0 Saturday is a 1-unit hairline, the max day fills 52.
  const bars = [...panel.matchAll(/<rect x="[\d.]+" y="([\d.]+)" width="28" height="([\d.]+)" fill="([^"]+)">/g)].map((m) => [Number(m[1]), Number(m[2]), m[3]] as const)
  expect(bars).toHaveLength(7)
  expect(bars.filter(([, , fill]) => fill === 'var(--status-info-fg)')).toHaveLength(1)
  expect(bars[6]).toEqual([40, 52, 'var(--status-info-fg)'])
  expect(bars[3]).toEqual([91, 1, 'var(--fg-secondary)'])
  expect(panel).toMatch(/>t<.*>f<.*>s<.*>s<.*>m<.*>t<.*>w</s)
  // Owners: you (plain, unlinked), builder + research (ring dot → /agents/<name>), nightly-digest (square → /routines/<name>).
  expect(panel).toContain('>you<')
  expect(panel).not.toMatch(/<a href="[^"]*"><text x="28" y="124"/)
  expect(panel).toMatch(/<a href="\/agents\/builder"><circle cx="18"/)
  expect(panel).toMatch(/<a href="\/routines\/nightly-digest"><rect x="15.5"/)
  expect(panel).toContain('>$2,210.50<')
  expect(panel).not.toContain('ops-desk</text>') // the fifth owner is cut at four
  // Cap rows: only budgeted agents, in share order; with four owner rows one fits (docs-desk, the fullest), analyst (no cap) never.
  expect(panel).toContain('>docs-desk cap<')
  expect(panel).not.toContain('analyst')
  expect(panel).not.toContain('var(--fg-accent)')
  expect(panel).not.toContain('var(--status-danger-fg)')
  // Compact: the text line carries month, today and the top two owners.
  const compact = renderToStaticMarkup(createElement(SceneCompact, { data: F, now: NOW }))
  const line = /<div aria-label="Spend"[^>]*>(.*?)<svg viewBox="0 0 352 248"/.exec(compact)![1]!
  expect(line).toContain('$4,102.00 month')
  expect(line).toContain('$1,302.38 today')
  expect(line).toContain('you $2,210.50')
  expect(line).toMatch(/<a href="\/agents\/builder"[^>]*><span[^>]*>builder \$1,104\.20/)
  expect(line).not.toContain('research')
  // No report: seven hairlines, $0.00 everywhere, no owner rows; the chrome stays.
  const empty = /<text x="24" y="19"[^>]*>SPEND<\/text>(.*?)<\/g><\/svg>/.exec(renderToStaticMarkup(createElement(Scene, { data: {}, now: NOW })))![1]!
  expect(count(empty, /height="1" fill=/g)).toBe(7)
  expect(empty).toContain('>TODAY $0.00<')
  expect(empty).not.toMatch(/<text x="28"/)
})

test('Revision 4 — satellites: none for a root without live children; capped at 8 with a "+N" after the last; the fold ring draws none', () => {
  // Two roster agents → 3 agent rings, 4 routines, 4 roots fit at hv 812 (12 slots).
  const data = { ...F, agents: F.agents.slice(0, 2) }
  const html = renderToStaticMarkup(createElement(Scene, { data, now: NOW }))
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(3 + 4 + 4)
  expect(html).toContain('data-satellites="ring:session:chat-1"')
  expect(html).toContain('data-satellites="ring:session:chat-3"')
  expect(html).not.toContain('data-satellites="ring:session:chat-2"')
  expect(html).not.toContain('data-satellites="ring:session:chat-4"')
  expect(count(html, /data-satellites="/g)).toBe(2)
  expect(html).toContain('href="/c/chat-2"')
  // Ten live children → 8 dots and "+2" at the ninth position.
  const ten = Array.from({ length: 10 }, (_, i) => ({ ...F.sessions.find((s) => s.id === 'child-21')!, id: `k${i}`, parent: 'chat-2' }))
  const crowded = renderToStaticMarkup(createElement(Scene, { data: { ...data, sessions: [...F.sessions, ...ten] }, now: NOW }))
  const sats = /<g data-satellites="ring:session:chat-2">(.*?)<\/g>/.exec(crowded)![1]!
  expect(count(sats, /<circle[^>]*r="2.2"/g)).toBe(8)
  expect(sats).toMatch(/<text[^>]*font-size="7"[^>]*>\+2<\/text>/)
})

test('Scene while loading shows — not 00; a loaded empty inbox is quiet and tertiary; errors name the query', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: {}, now: NOW, errors: ['tasks'] }))
  expect(count(html, /data-flow="/g)).toBe(0)
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(1) // theuth's ring is the chrome
  expect(html).toContain('— NEED YOU')
  expect(html).not.toContain('00 NEED YOU')
  expect(html).not.toContain('Nothing needs you') // attention still loading
  expect(html).toContain('tasks failed to load')
  const quiet = renderToStaticMarkup(createElement(Scene, { data: { attention: [] }, now: NOW }))
  expect(quiet).toMatch(/fill="var\(--fg-tertiary\)"[^>]*>Nothing needs you</)
  expect(quiet).toMatch(/fill="var\(--fg-tertiary\)"[^>]*>00 NEED YOU</)
  expect(quiet).toMatch(/<text x="430"[^>]*fill="var\(--fg-tertiary\)"[^>]*>00</)
  expect(quiet).not.toContain('<rect x="1852"')
})

test('SceneCompact: pills, needs rows, ring list, four SVG panels + the stream box; SceneSwitch picks it under md and Scene above', () => {
  const html = renderToStaticMarkup(createElement(SceneCompact, { data: F, now: NOW }))
  expect(html).toContain('06 NEED YOU')
  expect(count(html, /href="\/attention\//g)).toBe(6)
  expect(html).toMatch(/<a href="\/attention"[^>]*><h3/) // the NEEDS YOU header opens the inbox
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(12) // the routine and session rings are in the compact list too
  expect(html).toContain('>2 running · 3 children · 40s · ctx 71 %<')
  expect(html).toContain('href="/c/chat-1"')
  expect(count(html, /viewBox="0 0 352 248"/g)).toBe(4)
  expect(html).toMatch(/<section aria-label="Stream" class="[^"]*max-h-\[40vh\]/)
  expect(count(html, /<li title="/g)).toBe(F.activity.length)
  expect(html).not.toMatch(/(fill|stroke)="#/)
  // Every desktop destination is a tap here too: pills, ring rows, panel links.
  expect(count(html, /href="\/new"/g)).toBe(3) // the theuth ring row, its AGENTS row, the SESSIONS LIVE pill
  expect(html).toContain('href="/agents/builder"')
  expect(html).toContain('href="/routines/nightly-digest"')
  expect(html).toContain('href="/routines/knowledge-sync"')
  expect(html).toContain('>next 23:30<')
  expect(html).toContain('>4 open · 1h · ctx 62 %<') // Revision 2: the ctx share as text; no gauge track in the compact list
  expect(count(html, /<circle[^>]*r="17"/g)).toBe(0)
  expect(html).toContain('href="/tasks?state=doing&amp;project="')
  expect(html).toContain('href="/tasks?state=backlog&amp;project="')
  // Under md: the compact stack — no flow field, no arc band, no 1920-wide scene.
  mobile.value = true
  const below = renderToStaticMarkup(createElement(SceneSwitch, { data: F, now: NOW }))
  expect(below).toContain('Needs you')
  expect(count(below, /viewBox="0 0 352 248"/g)).toBe(4)
  expect(count(below, /data-flow="/g)).toBe(0)
  expect(below).not.toContain('viewBox="0 0 1920')
  expect(below).not.toContain('A 1408 1408')
  // From md up: the full scene.
  mobile.value = false
  const above = renderToStaticMarkup(createElement(SceneSwitch, { data: F, now: NOW }))
  expect(above).toContain('viewBox="0 0 1920 812"')
  expect(count(above, /data-flow="/g)).toBe(scene(F, NOW).flows.length)
})
