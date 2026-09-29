import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { SidebarMenuAction } from './sidebar'

const classes = (showOnHover: boolean) => /class="([^"]*)"/.exec(renderToStaticMarkup(createElement(SidebarMenuAction, { showOnHover })))![1]!.split(' ')

test('the ⋯ hides until hover only for a fine pointer at ≥ md; the finger hit area is dropped only there too', () => {
  const on = classes(true)
  expect(on).toContain('md:pointer-fine:opacity-0')
  expect(on).not.toContain('md:opacity-0')
  expect(on).toContain('group-hover/menu-item:opacity-100')
  expect(on).toContain('after:-inset-2')
  expect(on).toContain('md:pointer-fine:after:hidden')
  expect(on).not.toContain('md:after:hidden')
  expect(classes(false)).not.toContain('md:pointer-fine:opacity-0')
})
