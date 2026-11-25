import { createElement as h, type ReactNode } from 'react'
import { renderToString } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DocSummary } from '@/rpc/bindings/method.agent.output'
import { POPOVER_CAP } from './Rail'
import { WhiteboardsMenu } from './Whiteboards'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  Link: ({ to, params, children, onClick: _o, ...rest }: { to: string; params?: { id: string }; children: ReactNode; onClick?: unknown }) =>
    h('a', { href: params ? to.replace('$id', params.id) : to, ...rest }, children),
}))

const wb = (i: number) => ({ id: `w${i}`, title: `Board ${i}`, updated_ms: 1_700_000_000_000 - i * 1000 }) as unknown as DocSummary

test('the popover caps at POPOVER_CAP, offers New whiteboard and All whiteboards', () => {
  const rows = Array.from({ length: POPOVER_CAP + 3 }, (_, i) => wb(i))
  const html = renderToString(h(WhiteboardsMenu, { rows, active: 'w1', onNew: () => {}, onPick: () => {} }))
  expect(html.match(/data-whiteboard=/g)).toHaveLength(POPOVER_CAP)
  expect(html).toContain('href="/whiteboards/w0"')
  expect(html).not.toContain(`/whiteboards/w${POPOVER_CAP}"`)
  expect(html).toContain('New whiteboard')
  expect(html).toContain('href="/whiteboards"')
  expect(html).toContain(`All whiteboards (${POPOVER_CAP + 3})`)
})

test('an empty list still offers New whiteboard and All whiteboards', () => {
  const html = renderToString(h(WhiteboardsMenu, { rows: [], onNew: () => {}, onPick: () => {} }))
  expect(html).toContain('New whiteboard')
  expect(html).not.toContain('data-whiteboard=')
  expect(html).toContain('All whiteboards')
})
