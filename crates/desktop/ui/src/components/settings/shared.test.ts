import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { Row } from './shared'

test('Settings rows stack in narrow body columns without changing rows elsewhere or desktop layout', () => {
  const plain = renderToStaticMarkup(createElement(Row, { label: 'Timeout', description: 'Seconds' }, createElement('input')))
  for (const rule of ['flex-col', 'items-stretch', 'gap-3', 'w-full', 'flex-wrap']) {
    expect(plain).toContain(`max-lg:@max-[480px]/settings:${rule}`)
  }
  expect(plain).toContain('flex items-center justify-between gap-6')
  expect(plain).not.toContain('max-md:flex-col')
  const stacked = renderToStaticMarkup(createElement(Row, { stack: true, label: 'Model' }, createElement('button', null, 'pick')))
  expect(stacked).toContain('max-md:flex-col max-md:items-stretch max-md:gap-3')
})

test('wide rows stay full-width on desktop and allow their actions to wrap in narrow Settings columns', () => {
  const wide = renderToStaticMarkup(createElement(Row, { wide: true, label: 'Manifest' }, createElement('button', null, 'Copy')))
  expect(wide).toContain('flex flex-col gap-3')
  expect(wide).toContain('flex w-full items-center gap-2')
  expect(wide).toContain('max-lg:@max-[480px]/settings:flex-wrap')
})
