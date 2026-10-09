import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { Row } from './shared'

test('148 R3: a `stack` row drops its control under the label below md; a plain row keeps the one-line layout', () => {
  const stacked = renderToStaticMarkup(createElement(Row, { stack: true, label: 'Model', description: 'Wide control' }, createElement('button', null, 'pick')))
  expect(stacked).toContain('max-md:flex-col max-md:items-stretch max-md:gap-3')
  const plain = renderToStaticMarkup(createElement(Row, { label: 'Switch', description: 'Narrow control' }, createElement('button', null, 'on')))
  expect(plain).not.toContain('max-md:')
  // Desktop classes are byte-identical either way.
  expect(plain).toContain('class="flex items-center justify-between gap-6 border-b border-border-subtle py-4 last:border-0"')
  expect(stacked).toContain('class="flex items-center justify-between gap-6 border-b border-border-subtle py-4 last:border-0 max-md:')
})
