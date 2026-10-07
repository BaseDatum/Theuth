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
  // 10 rings (r=13: 6 agents + 4 routines), 6 needs-you dots (r=7), one flow per laid-out flow, each hover-keyed.
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(10)
  expect(count(html, /<circle[^>]*r="7"/g)).toBe(6)
  expect(count(html, /data-flow="/g)).toBe(m.flows.length)
  // Only live rings pulse; motion is a class (styles.css turns it off under prefers-reduced-motion), never inline.
  expect(count(html, /class="hud-pulse"/g)).toBe(m.rings.filter((r) => r.style === 'live').length)
  expect(html).not.toContain('animation')
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
  expect(html).toMatch(/href="\/tasks\?state=doing&amp;project="><tspan>4 IN FLIGHT<\/tspan>/) // 3 doing tasks + the running routine
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
  expect(html).toContain('>4 IN FLIGHT<')
  expect(html).toContain('$31.20 THIS MONTH')
  for (const t of ['AGENTS', 'ARRIVALS', 'OUTCOMES', 'SPEND', 'STREAM']) expect(html).toContain(`>${t}<`)
  expect(html).toContain('>19:49<')
  // Accessibility: the field is one image with a title and a summary; the band is HTML (four SVG panels + the stream box).
  expect(html).toMatch(/<svg[^>]*viewBox="0 0 1920 812"[^>]*role="img"[^>]*aria-labelledby="hud-scene-title hud-scene-desc"/)
  expect(count(html, /viewBox="0 0 352 248"/g)).toBe(4)
  expect(html).toContain('6 need you · 2 agents working')
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
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(10) // the routine rings are in the compact list too
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
