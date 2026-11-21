import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { useToolRuns } from '@/lib/tool-runs'
import { DisclosureScope } from './disclosure'
import { MessageInfo } from './MessageInfo'

const html = (label?: string) =>
  renderToStaticMarkup(createElement(DisclosureScope, { value: 'n' }, createElement(MessageInfo, { text: 'L1 added to Goals: ship it', label })))

test('201 R3-2: a labelled harness note shows only its label until opened; a plain info line shows its text', () => {
  useToolRuns.setState({ disclosed: {} })
  const closed = html('Ledger updated')
  expect(closed).toContain('Ledger updated')
  expect(closed).not.toContain('ship it')
  useToolRuns.setState({ disclosed: { 'n|note': true } })
  expect(html('Ledger updated')).toContain('L1 added to Goals: ship it')
  expect(html()).toContain('L1 added to Goals: ship it')
})
