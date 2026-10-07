import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import { sceneFixture, T } from './fixtures'
import { scene } from './model'
import { Scene } from './Scene'
import { SceneCompact, SceneSwitch } from './SceneCompact'

// A `Link` needs a router; a plain anchor with the params substituted does not, and lets the hrefs be asserted.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ to, params, children }: { to: string; params?: Record<string, string>; children: ReactNode }) =>
    createElement('a', { href: to.replace(/\$(\w+)/g, (_, k: string) => params?.[k] ?? '') }, children),
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
  // 7 rings (r=13), 6 needs-you dots (r=7), one flow per laid-out flow, each hover-keyed.
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(7)
  expect(count(html, /<circle[^>]*r="7"/g)).toBe(6)
  expect(count(html, /data-flow="/g)).toBe(m.flows.length)
  // Only live rings pulse; motion is a class (styles.css turns it off under prefers-reduced-motion), never inline.
  expect(count(html, /class="hud-pulse"/g)).toBe(m.rings.filter((r) => r.style === 'live').length)
  expect(html).not.toContain('animation')
  // Links: an attention row, a task flow, an agent ring, a routine mark, the footer segments.
  expect(html).toContain('href="/attention/approval:ap-4821"')
  expect(html).toContain(`href="/tasks/${T('1212')}"`)
  expect(html).toContain('href="/agents/builder"')
  expect(html).toContain('href="/routines/morning-brief"')
  expect(html).toContain('href="/attention"')
  expect(html).toContain('href="/tasks"')
  // Text: the top bar, the clock, the NEEDS YOU table, footer numbers, panel titles.
  expect(html).toContain('T H E U T H')
  expect(html).toContain('● 19:49:19')
  expect(html).toContain('>06<')
  expect(html).toContain('06 NEED YOU')
  expect(html).toContain('2 AGENTS WORKING')
  expect(html).toContain('$31.20 THIS MONTH')
  expect(html).toContain('11 OUTCOMES TODAY · 4 IN FLIGHT')
  for (const t of ['AGENTS', 'ARRIVALS', 'OUTCOMES', 'SPEND', 'STREAM']) expect(html).toContain(`>${t}<`)
  expect(html).toContain('>19:49<')
  // The spend row over 85 % is carnelian; the others malachite.
  expect(html).toContain('stroke="var(--status-danger-fg)" stroke-width="2"')
  // Accessibility: one image with a title and a summary.
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="hud-scene-title hud-scene-desc"/)
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

test('SceneCompact: pills, needs rows, ring list, five panels; SceneSwitch picks it under md and Scene above', () => {
  const html = renderToStaticMarkup(createElement(SceneCompact, { data: F, now: NOW }))
  expect(html).toContain('06 NEED YOU')
  expect(count(html, /href="\/attention\//g)).toBe(6)
  expect(count(html, /<circle[^>]*r="13"/g)).toBe(7)
  expect(count(html, /viewBox="0 0 352 248"/g)).toBe(5)
  expect(html).not.toMatch(/(fill|stroke)="#/)
  // Under md: the compact stack — no flow field, no arc band, no 1920×1080 scene.
  mobile.value = true
  const below = renderToStaticMarkup(createElement(SceneSwitch, { data: F, now: NOW }))
  expect(below).toContain('Needs you')
  expect(count(below, /viewBox="0 0 352 248"/g)).toBe(5)
  expect(count(below, /data-flow="/g)).toBe(0)
  expect(below).not.toContain('viewBox="0 0 1920 1080"')
  expect(below).not.toContain('A 1408 1408')
  // From md up: the full scene.
  mobile.value = false
  const above = renderToStaticMarkup(createElement(SceneSwitch, { data: F, now: NOW }))
  expect(above).toContain('viewBox="0 0 1920 1080"')
  expect(count(above, /data-flow="/g)).toBe(scene(F, NOW).flows.length)
})
