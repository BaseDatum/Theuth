import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { ErrorCard, FenceContext, FenceFallback } from './JsonRenderFence'

const render = (ctx: FenceContext) =>
  renderToStaticMarkup(createElement(FenceContext, { value: ctx }, createElement(FenceFallback, { error: 'bad arrow', source: 'graph TD', title: 'Diagram unavailable' })))

// fence-skel D1: no placeholder in any state — the fix streams in below while the turn runs.
test.each([
  ['the turn that wrote it still runs', { running: true, hasLaterAssistant: false }],
  ['a later reply of that turn carries the fix', { running: false, hasLaterAssistant: true }],
  ['both', { running: true, hasLaterAssistant: true }],
])('%s: nothing at all', (_, ctx) => {
  expect(render(ctx)).toBe('')
})

test('last reply of a turn that ended: one dim line with the source behind a disclosure and the message as tooltip', () => {
  const html = render({ running: false, hasLaterAssistant: false })
  expect(html).toContain('◌ diagram omitted ·')
  // Closed until the reader opens it (171F B13: the open state lives in the disclosure store), so no source yet.
  expect(html).toMatch(/<button[^>]*aria-expanded="false"[^>]*>show source<\/button>/)
  expect(html).not.toContain('graph TD')
  expect(html).toContain('title="Diagram unavailable · bad arrow"')
  expect(html).not.toContain('danger')
  expect(html).not.toContain('role="alert"')
  expect(html).not.toContain('animate-pulse')
  expect(html).not.toContain('aria-busy')
})

test('the default context (artifact viewer, tool cards) is the settled case; ErrorCard is the same component', () => {
  expect(ErrorCard).toBe(FenceFallback)
  expect(renderToStaticMarkup(createElement(ErrorCard, { error: 'e', source: 's' }))).toContain('diagram omitted')
})
