import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { SectionRow } from './SectionRow'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, children, ...rest }: { to: string; children: unknown }) => createElement('a', { href: to, ...rest }, children as never),
}))

test('back is a link with the aria-label, the title an h2, children follow in order', () => {
  const html = renderToStaticMarkup(
    createElement(SectionRow, { back: { to: '/notes', label: 'Back to notes', className: 'md:hidden' }, title: 'Grocery list' }, createElement('button', { type: 'button' }, 'Act')),
  )
  const back = html.indexOf('href="/notes"')
  const title = html.indexOf('<h2')
  const action = html.indexOf('>Act</button>')
  expect(back).toBeGreaterThan(-1)
  expect(html).toContain('aria-label="Back to notes"')
  expect(html).toContain('md:hidden')
  expect(html).toContain('>Grocery list</h2>')
  expect(back < title && title < action).toBe(true)
})

test('without back or title only the children render', () => {
  const html = renderToStaticMarkup(createElement(SectionRow, null, createElement('span', null, 'x')))
  expect(html).not.toContain('<h2')
  expect(html).not.toContain('<a')
  expect(html).toContain('<span>x</span>')
})
