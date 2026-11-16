import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
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

test.each([
  ['switch', createElement(Switch, { 'aria-label': 'Enabled' })],
  ['checkbox', createElement('input', { type: 'checkbox', 'aria-label': 'Enabled' })],
  ['radio', createElement('input', { type: 'radio', 'aria-label': 'Enabled' })],
  ['icon button', createElement(Button, { size: 'icon-sm', 'aria-label': 'Edit' })],
  ['custom checkbox', createElement('button', { role: 'checkbox', 'aria-checked': false })],
])('a lone %s stays centred beside its wrapping label instead of stacking', (_name, child) => {
  const html = renderToStaticMarkup(createElement(Row, { label: 'Enabled', description: 'A longer explanation', stack: true }, child))
  expect(html).toContain('data-settings-small="true"')
  expect(html).toContain('flex items-center justify-between')
  expect(html).not.toContain('flex-col')
  expect(html).not.toContain('settings:w-full')
})

test('native checkbox/radio controls have a label for the invisible touch hit area', () => {
  for (const type of ['checkbox', 'radio']) {
    const html = renderToStaticMarkup(createElement(Row, { label: 'Enabled' }, createElement('input', { type })))
    expect(html).toMatch(/<label data-settings-control=""[^>]*><input/)
  }
})

test('a group of small controls still stacks as a wide control', () => {
  const html = renderToStaticMarkup(createElement(Row, { label: 'Options' }, createElement(Switch, { key: 'a' }), createElement(Switch, { key: 'b' })))
  expect(html).not.toContain('data-settings-small="true"')
  expect(html).toContain('max-lg:@max-[480px]/settings:flex-col')
  expect(html).toContain('max-lg:@max-[480px]/settings:w-full')
})
