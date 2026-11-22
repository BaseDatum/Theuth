import '../settings/test-dom'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type { Entry } from '@/store/sessions'
import { DesktopHandoffCard } from './DesktopHandoffCard'

const mocks = vi.hoisted(() => ({ mutateAsync: vi.fn(async (_value: unknown) => null) }))
vi.mock('@/components/settings/sandbox-queries', () => ({
  useHandoffs: () => ({ data: [{ id: 'handoff-row', session: 'session-1', resolved_ms: null }] }),
}))
vi.mock('@/components/hud/queries', () => ({
  useAttentionActions: () => ({ handoff: { mutateAsync: mocks.mutateAsync, isPending: false } }),
}))

beforeEach(() => mocks.mutateAsync.mockClear())

type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (extra: Partial<Tool> = {}): Tool => ({
  id: 'handoff',
  kind: 'tool',
  callId: 'call-handoff',
  name: 'desktop_handoff',
  args: { reason: 'Log in to GitHub, then mark the handoff Done.', url: 'https://github.com/login' },
  pending: true,
  child: false,
  ...extra,
})
const card = (value: Tool) => renderToStaticMarkup(createElement(DesktopHandoffCard, { tool: value, session: 'session-1' }))

test('a pending handoff presents the instruction, page and where to finish', () => {
  const html = card(tool())
  expect(html).toContain('Your help is needed at the desktop')
  expect(html).toContain('Log in to GitHub, then mark the handoff Done.')
  expect(html).toContain('Waiting for you')
  expect(html).toContain('>Done</button>')
  expect(html).toContain('>Cancel</button>')
  expect(html).toContain('href="https://github.com/login"')
  expect(html).toContain('Open page')
  expect(html).toContain('border-info-border')
  expect(html).toContain('bg-info-bg')
})

test('a finished handoff keeps its instruction visible and reports the outcome', () => {
  const done = card(tool({ pending: false, result: { output: 'done after 12 s', isError: false, files: [] } }))
  expect(done).toContain('Desktop handoff completed')
  expect(done).toContain('Done')
  expect(done).toContain('Log in to GitHub, then mark the handoff Done.')
  expect(done).not.toContain('>Done</button>')
  expect(done).not.toContain('>Cancel</button>')

  const timeout = card(tool({ pending: false, result: { output: 'timeout after 1800 s', isError: true, files: [] } }))
  expect(timeout).toContain('Desktop handoff timed out')
  expect(timeout).toContain('Timed out')
})

test('Done and Cancel resolve the pending handoff for this session', async () => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(createElement(DesktopHandoffCard, { tool: tool(), session: 'session-1' })))
  const button = (label: string) => [...host.querySelectorAll('button')].find((b) => b.textContent === label)!

  await act(async () => button('Done').click())
  expect(mocks.mutateAsync).toHaveBeenCalledWith({ id: 'handoff-row', outcome: 'done' })
  await act(async () => button('Cancel').click())
  expect(mocks.mutateAsync).toHaveBeenCalledWith({ id: 'handoff-row', outcome: 'cancelled' })

  await act(async () => root.unmount())
  host.remove()
})
