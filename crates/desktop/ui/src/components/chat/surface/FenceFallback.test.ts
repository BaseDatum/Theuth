import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { ErrorCard, FenceContext, FenceFallback } from './JsonRenderFence'

const render = (ctx: FenceContext) =>
  renderToStaticMarkup(createElement(FenceContext, { value: ctx }, createElement(FenceFallback, { error: 'bad arrow', source: 'graph TD', title: 'Diagram unavailable' })))

test('turn running: a skeleton, no red card, no alert', () => {
  const html = render({ running: true, hasLaterAssistant: false })
  expect(html).toContain('animate-pulse')
  expect(html).toContain('aria-busy')
  expect(html).not.toContain('role="alert"')
  expect(html).not.toContain('bad arrow')
})

test('a later reply carries the fix: nothing', () => {
  expect(render({ running: false, hasLaterAssistant: true })).toBe('')
  expect(render({ running: true, hasLaterAssistant: true })).toBe('')
})

test('last reply of the turn: one dim line with the source behind a disclosure and the message as tooltip', () => {
  const html = render({ running: false, hasLaterAssistant: false })
  expect(html).toContain('◌ diagram omitted ·')
  expect(html).toContain('<summary class="inline cursor-pointer">show source</summary>')
  expect(html).toContain('<pre')
  expect(html).toContain('graph TD')
  expect(html).toContain('title="Diagram unavailable · bad arrow"')
  expect(html).not.toContain('danger')
  expect(html).not.toContain('role="alert"')
})

test('the default context (artifact viewer, tool cards) is the settled case; ErrorCard is the same component', () => {
  expect(ErrorCard).toBe(FenceFallback)
  expect(renderToStaticMarkup(createElement(ErrorCard, { error: 'e', source: 's' }))).toContain('diagram omitted')
})
