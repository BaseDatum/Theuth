import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type { Task } from '@/rpc/bindings/Task'
import { sceneFixture, T } from './fixtures'
import { scene } from './model'
import { Scene } from './Scene'
import { SceneCompact, SceneSwitch } from './SceneCompact'

// A `Link` needs a router; a plain anchor with the params substituted and the search appended does not, and lets the hrefs be asserted.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ to, params, search, children, className, style, 'aria-label': label }: { to: string; params?: Record<string, string>; search?: Record<string, string>; children: ReactNode; className?: string; style?: object; 'aria-label'?: string }) => {
    const q = search && Object.keys(search).length > 0 ? `?${new URLSearchParams(search).toString()}` : ''
    return createElement('a', { href: to.replace(/\$(\w+)/g, (_, k: string) => params?.[k] ?? '') + q, className, style, 'aria-label': label }, children)
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
  // the theuth ring and SESSIONS LIVE → /new, the footer's split right slot, the BOARD legend/projects/board, SPEND and AGENTS big numbers.
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
  expect(html).toMatch(/<a href="\/tasks"><text x="340" y="236"/) // the BOARD big number → the board
  expect(count(html, /<a href="\/agents"><text x="340" y="236"/g)).toBe(1) // the AGENTS big number
  expect(count(html, /<a href="\/costs"><text x="340" y="236"/g)).toBe(1) // 160 D1: the SPEND big number → /costs
  expect(html).toContain('href="/agents/ops-desk"')
  // STREAM: an HTML box holding every feed row, each linked to its subject by kind.
  expect(html).toMatch(/<section aria-label="Stream"/)
  expect(count(html, /<li title="/g)).toBe(F.activity.length)
  const done = F.activity.find((e) => e.kind === 'task_done')!
  expect(html).toContain(`href="/tasks/${done.subject}"`)
  expect(html).toContain(`>${F.activity.length}/50<`)
  // Text: the top bar, the clock, the NEEDS YOU table, footer numbers, panel titles.
  expect(html).toContain('T H E U T H')
  expect(html).toContain('<tspan fill="var(--fg-accent)">●</tspan> 19:49:19') // R5: the dot is gold while something needs you; no tick behind the digits
  expect(html).not.toContain('<rect x="1852"')
  expect(html).toContain('>06<')
  expect(html).toContain('06 NEED YOU')
  expect(html).toContain('>9 OUTCOMES TODAY<')
  expect(html).toContain('>6 IN FLIGHT<')
  expect(html).toMatch(/<a href="\/costs"><text[^>]*>\$4,102\.00 THIS MONTH</) // Revision 3: the ledger's month, not the roster's budget sum; 160 D1: → /costs
  for (const t of ['AGENTS', 'ARRIVALS', 'BOARD', 'SPEND', 'STREAM']) expect(html).toContain(`>${t}<`)
  expect(html).toContain('>19:49<')
  // Accessibility: the field is one image with a title and a summary; the band is HTML (four SVG panels + the stream box).
  expect(html).toMatch(/<svg[^>]*viewBox="0 0 1920 812"[^>]*role="img"[^>]*aria-labelledby="hud-scene-title hud-scene-desc"/)
  expect(count(html, /viewBox="0 0 352 248"/g)).toBe(4)
  expect(html).toContain('6 need you · 2 agents working')
})

test('Revision 6 — BOARD: every task in every project — a 60-cell waffle in board order, seven legend rows with all-time counts, the project line with links, total and project count', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW }))
  const panel = /<text x="24" y="19"[^>]*>BOARD<\/text>(.*?)<\/g><\/svg>/.exec(html)![1]!
  expect(panel).toContain('>3 PROJECTS<')
  expect(panel).toMatch(/<a href="\/tasks"><text x="340" y="236"[^>]*>20<\/text>/)
  // The waffle: 20 filled cells (one per task), 40 hollow; backlog first at 0.55, then todo, doing, review, blocked, done, cancelled.
  const cells = [...panel.matchAll(/<circle cx="(\d+)" cy="(\d+)" r="3.2" fill="([^"]+)"( fill-opacity="([\d.]+)")?/g)]
  expect(cells).toHaveLength(60)
  const filled = cells.filter((c) => c[3] !== 'none')
  expect(filled).toHaveLength(20)
  expect(cells.filter((c) => c[3] === 'none')).toHaveLength(40)
  expect(panel).toMatch(/<circle cx="146" cy="120" r="3.2" fill="none" stroke="var\(--border-strong\)"/)
  const run = (fill: string, opacity?: string) => filled.filter((c) => c[3] === fill && c[5] === opacity).length
  expect(run('var(--fg-tertiary)', '0.55')).toBe(4) // backlog
  expect(run('var(--fg-secondary)')).toBe(2) // todo
  expect(run('var(--status-info-fg)')).toBe(3) // doing
  expect(run('var(--fg-primary)')).toBe(2) // review
  expect(run('var(--fg-accent)')).toBe(1) // blocked
  expect(run('var(--status-success-fg)')).toBe(7) // done
  expect(run('var(--fg-tertiary)')).toBe(1) // cancelled, tertiary in full
  expect(filled.slice(0, 4).every((c) => c[5] === '0.55')).toBe(true)
  expect(filled[19]![3]).toBe('var(--fg-tertiary)')
  expect(filled[19]![5]).toBeUndefined()
  // Seven legend rows, 14 px apart from y 52, each the all-time count and a link to its column.
  const rows = [...panel.matchAll(/<a href="\/tasks\?state=(\w+)&amp;project="><circle cx="186" cy="(\d+)" r="2.5"[^>]*><\/circle><text x="196" y="(\d+)"[^>]*>\w+<\/text><text x="338" y="\d+"[^>]*>(\d+)<\/text>/g)].map((r) => [r[1], Number(r[2]), Number(r[3]), Number(r[4])])
  expect(rows).toEqual([
    ['backlog', 49, 52, 4],
    ['todo', 63, 66, 2],
    ['doing', 77, 80, 3],
    ['review', 91, 94, 2],
    ['blocked', 105, 108, 1],
    ['done', 119, 122, 7],
    ['cancelled', 133, 136, 1],
  ])
  expect(panel).toContain('<title>4 backlog · open the board at backlog</title>')
  // Today's movement, secondary, under the waffle; the project line at the bottom, each project a link, unfiled → the bare board.
  expect(panel).toMatch(/<a href="\/tasks"><text x="20" y="136"[^>]*>12 moved today<\/text>/)
  const line = /<text x="20" y="148"[^>]*data-projects="true">(.*?)<\/text>/.exec(panel)![1]!
  expect(line.replace(/<[^>]+>/g, '')).toBe('9 tasks · open the board at hudhud 9 · 6 tasks in no project · open the boardunfiled 6 · 5 tasks · open the board at relayrelay 5')
  expect(line).toContain('<a href="/tasks?project=hud"><title>9 tasks · open the board at hud</title><tspan>hud 9</tspan></a>')
  expect(line).toContain('<a href="/tasks"><title>6 tasks in no project · open the board</title><tspan>unfiled 6</tspan></a>')
  expect(line).toContain('<a href="/tasks?project=relay">')
  expect(line).not.toContain('+')
  // Sky's board (80 tasks, nothing moved today) is never zeros: 60 cells over five colours, 80 and 4 PROJECTS, 0 moved today.
  const t0 = F.tasks[0]!
  const old = new Date(2026, 9, 6, 18, 34).getTime()
  const mk = (state: Task['state'], n: number, project: string | null) => Array.from({ length: n }, (_, i) => ({ ...t0, id: `${state}-${project}-${i}`, state, project, checkout: null, updated_ms: old }))
  const sky = [...mk('review', 41, 'DR failover'), ...mk('backlog', 9, 'DR failover'), ...mk('backlog', 15, null), ...mk('blocked', 10, null), ...mk('cancelled', 2, null), ...mk('done', 1, 'sre-2379'), ...mk('cancelled', 1, 'sre-2379'), ...mk('done', 1, 'theuth')]
  const h2 = renderToStaticMarkup(createElement(Scene, { data: { ...F, tasks: sky }, now: NOW }))
  const p2 = /<text x="24" y="19"[^>]*>BOARD<\/text>(.*?)<\/g><\/svg>/.exec(h2)![1]!
  expect(p2).toContain('>4 PROJECTS<')
  expect(p2).toMatch(/<text x="340" y="236"[^>]*>80<\/text>/)
  const c2 = [...p2.matchAll(/<circle cx="\d+" cy="\d+" r="3.2" fill="([^"]+)"/g)].map((c) => c[1])
  expect(c2.filter((f) => f !== 'none')).toHaveLength(60)
  expect(new Set(c2)).toEqual(new Set(['var(--fg-tertiary)', 'var(--fg-primary)', 'var(--fg-accent)', 'var(--status-success-fg)']))
  expect(p2).toContain('>0 moved today<')
  const l2 = /<text x="20" y="148"[^>]*data-projects="true">(.*?)<\/text>/.exec(p2)![1]!
  expect(l2.replace(/<title>[^<]*<\/title>/g, '').replace(/<[^>]+>/g, '')).toBe('DR failover 50 · unfiled 27 · sre-2379 2 · theuth 1')
  expect(l2).toContain('href="/tasks?project=DR+failover"')
  // Many projects fold into +K and the line stays within the character budget.
  const many = Array.from({ length: 12 }, (_, i) => mk('todo', 12 - i, `project-${String.fromCharCode(97 + i)}`)).flat()
  const l3 = /<text x="20" y="148"[^>]*data-projects="true">(.*?)<\/text>/.exec(renderToStaticMarkup(createElement(Scene, { data: { ...F, tasks: many }, now: NOW })))![1]!
  const text3 = l3.replace(/<title>[^<]*<\/title>/g, '').replace(/<[^>]+>/g, '')
  expect(text3).toMatch(/ \+\d+$/)
  expect(text3.length).toBeLessThanOrEqual(70)
})

test('Revision 3 — SPEND: month big number → /costs with a chart-icon action after the title, TODAY meta, seven bars (today faience, $0 day a hairline), owner rows with glyphs and links, cap rows only for budgeted agents, no gold/carnelian', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW }))
  const panel = /<text x="24" y="19"[^>]*>SPEND<\/text>(.*?)<\/g><\/svg>/.exec(html)![1]!
  expect(panel).toContain('>TODAY $1,302.38<')
  expect(panel).toMatch(/<a href="\/costs"><text x="340" y="236"[^>]*>\$4,102\.00<\/text>/)
  // 160 D1: the title action — a focusable, labelled `<a>` to /costs with a tooltip, a 14-unit square hit area after the 5-glyph title, the lucide chart-line icon; tokens only.
  const action = /<g><title>Open costs<\/title><a href="\/costs" aria-label="Open costs">(.*?)<\/a><\/g>/.exec(panel)![1]!
  expect(action).toMatch(/^<rect x="64" y="9" width="14" height="14" rx="3" fill="var\(--bg-elevated\)" stroke="var\(--border-strong\)"><\/rect>/)
  expect(action).toMatch(/<path transform="translate\(65 10\) scale\(0.5\)" d="M3 3v18h18M19 9l-5 5-4-4-3 3" fill="none" stroke="var\(--fg-secondary\)"/)
  expect(panel).not.toMatch(/(fill|stroke)="#/)
  expect(count(panel, /href="\/costs"/g)).toBe(2) // the action and the big number, nothing else
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
  expect(quiet).toContain('<tspan fill="var(--fg-secondary)">●</tspan> 19:49:19') // R5: nothing needs you → the dot is the clock's own ink
  expect(quiet).not.toContain('<rect x="1852"')
})

test('R5 — hover: every flow (due too) starts with an 8 px transparent hit stroke and an inert visible stroke; nothing is hot by default', () => {
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW }))
  const m = scene(F, NOW)
  const groups = [...html.matchAll(/<g data-flow="([^"]+)"[^>]*>(.*?)<\/g>/g)]
  expect(groups).toHaveLength(m.flows.length)
  for (const [, , body] of groups) {
    expect(body).toMatch(/^<title>[^<]*<\/title>(<a href="[^"]*">)?<path d="[^"]+" stroke="transparent" stroke-width="8" fill="none" pointer-events="stroke"><\/path><path d="[^"]+" stroke="var\(--[^"]+\)" stroke-opacity="0\.(95|5)" stroke-width="1.1"[^>]*pointer-events="none">/)
  }
  expect(html).not.toContain('data-hot=')
  expect(html).not.toContain('opacity:0.35')
  expect(html).not.toContain('stroke-width="1.8"')
})

test('R5 — hover: the hot flow is drawn again after every flow — stroke 1.8 at opacity 1, end dot 2.6, label primary 500, its need dot ring 2 and id brightened —, its via ring is emphasised, every other flow dims to 0.35; a hot outcome emphasises its ring, not a dot', () => {
  const m = scene(F, NOW)
  const need = m.flows.find((f) => f.id === 'need:approval:ap-4821')!
  const html = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW, hot: need.id }))
  // All flows still in model order, each keyed once; the copy is the last flow-shaped group and takes no pointer.
  expect([...html.matchAll(/<g data-flow="([^"]+)"/g)].map((x) => x[1])).toEqual(m.flows.map((f) => f.id))
  const hotAt = html.indexOf(`<g data-hot="${need.id}" pointer-events="none">`)
  expect(hotAt).toBeGreaterThan(html.lastIndexOf('<g data-flow="'))
  const hot = /<g data-hot="[^"]+" pointer-events="none">(.*?)<\/g>/.exec(html)![1]!
  expect(hot).toMatch(/^<path d="[^"]+" stroke="var\(--fg-accent\)" stroke-opacity="1" stroke-width="1.8" fill="none" pointer-events="none"><\/path>/)
  expect(hot).toContain(`<circle cx="${need.end[0]}" cy="${need.end[1]}" r="2.6" fill="var(--fg-accent)">`)
  expect(hot).toMatch(new RegExp(`<text x="${need.end[0] + 9}"[^>]*fill="var\\(--fg-primary\\)" font-weight="500">${need.label}</text>`))
  expect(hot).toMatch(/<circle[^>]*r="7" fill="var\(--bg-page\)" stroke="var\(--fg-accent\)" stroke-width="2">/)
  expect(hot).toMatch(/<text[^>]*fill="var\(--fg-primary\)" font-weight="500" text-anchor="end">builder<\/text>/)
  expect(hot).not.toContain('stroke="transparent"')
  // The original stays untouched under the copy (its hit path keeps the pointer); every other flow is dimmed inline.
  const own = new RegExp(`<g data-flow="${need.id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"([^>]*)>`).exec(html)![1]!
  expect(own).not.toContain('opacity')
  expect(count(html, /<g data-flow="[^"]+" class="hud-flow-in" style="opacity:0.35">/g)).toBe(m.flows.length - 1)
  // The left end: builder's ring (the item's `via`) gets stroke 2 and 500-weight labels; no other ring does.
  expect(need.via?.id).toBe('ring:agent:builder')
  const ring = (id: string) => new RegExp(`<g data-ring="${id}">(.*?)</a></g>`).exec(html)![1]!
  expect(ring('ring:agent:builder')).toMatch(/<circle[^>]*r="13" fill="var\(--bg-page\)" stroke="var\(--fg-accent\)" stroke-width="2">/)
  expect(ring('ring:agent:builder')).toMatch(/font-weight="500" text-anchor="end">builder<\/text>/)
  expect(ring('ring:agent:ops-desk')).not.toContain('font-weight')
  expect(count(html, /font-weight="500"/g)).toBe(4) // hot label, hot need id, builder's name and sub
  // A hot outcome: ring emphasis, no need dot in the copy; a hot due connector: dashed, 1.8 at opacity 1.
  const out = m.flows.find((f) => f.kind === 'outcome' && f.via !== null)!
  const h2 = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW, hot: out.id }))
  const hot2 = /<g data-hot="[^"]+" pointer-events="none">(.*?)<\/g>/.exec(h2)![1]!
  expect(hot2).not.toContain('r="7"')
  expect(hot2).toContain('stroke="var(--status-success-fg)" stroke-opacity="1" stroke-width="1.8"')
  expect(new RegExp(`<g data-ring="${out.via!.id}">(.*?)</a></g>`).exec(h2)![1]).toContain('stroke-width="2"')
  const due = m.flows.find((f) => f.kind === 'due')!
  const h3 = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW, hot: due.id }))
  expect(/<g data-hot="[^"]+" pointer-events="none">(.*?)<\/g>/.exec(h3)![1]).toMatch(/^<path d="[^"]+" stroke="var\(--fg-tertiary\)" stroke-opacity="1" stroke-width="1.8" stroke-dasharray="3 3" fill="none" pointer-events="none"><\/path>$/)
  // Unknown id: nothing is hot, nothing dims.
  const h4 = renderToStaticMarkup(createElement(Scene, { data: F, now: NOW, hot: 'need:gone' }))
  expect(h4).not.toContain('data-hot=')
  expect(h4).not.toContain('opacity:0.35')
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
