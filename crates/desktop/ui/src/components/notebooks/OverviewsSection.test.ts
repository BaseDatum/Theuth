import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { askKeys } from '@/lib/ask-rpc'
import { queryKeys } from '@/lib/rpc'
import type { Overview } from "@/rpc/bindings/method.agent.output"
import { OverviewsSection, versionsOf } from './OverviewsSection'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, params }: { children?: unknown; to: string; params?: Record<string, string> }) =>
    createElement('a', { href: Object.entries(params ?? {}).reduce((p, [k, v]) => p.replace(`$${k}`, v), to) }, children as never),
}))
vi.mock('@/store/connection', () => ({ useConnection: (pick: (s: unknown) => unknown) => pick({ client: { call: vi.fn() }, discovery: { phase: 'legacy' } }) }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/components/chat/Attachments', () => ({ useFileUrl: (path: string) => `blob:${path}` }))

const NB = 'nb1'
const row = (p: Partial<Overview> & Pick<Overview, 'kind' | 'version' | 'status'>): Overview => ({
  artifact: `call-${p.kind}-${p.version}`,
  name: `${p.kind}.md`,
  created_ms: Date.now() - 3_600_000 * p.version,
  session: 's',
  note: null,
  error: null,
  ...p,
})

const render = (rows: Overview[]) => {
  const qc = new QueryClient()
  qc.setQueryData(askKeys.overviews(NB), rows)
  qc.setQueryData(queryKeys.artifact('call-audio-1'), { path: '/data/artifacts/abc.mp3' })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(OverviewsSection, { notebook: NB }) }))
}

test('versionsOf: newest version first per kind; the audio script rows are not versions of their own', () => {
  const rows = [row({ kind: 'briefing', version: 1, status: 'ready' }), row({ kind: 'briefing', version: 2, status: 'running' }), row({ kind: 'audio', version: 1, status: 'ready', name: 'overview-script.md' }), row({ kind: 'audio', version: 1, status: 'ready', name: 'overview.mp3' })]
  expect(versionsOf(rows, 'briefing').map((o) => o.version)).toEqual([2, 1])
  expect(versionsOf(rows, 'audio').map((o) => o.name)).toEqual(['overview.mp3'])
  expect(versionsOf(rows, 'faq')).toEqual([])
})

test('rows: the newest version with its status, Open and Regenerate; older ones behind ▾ versions; failed carries its error', () => {
  const html = render([
    row({ kind: 'briefing', version: 1, status: 'ready' }),
    row({ kind: 'briefing', version: 2, status: 'running', artifact: null }),
    row({ kind: 'faq', version: 1, status: 'failed', error: 'model timed out', artifact: null }),
    row({ kind: 'timeline', version: 1, status: 'ready' }),
  ])
  expect(html).toContain('briefing.md · v2 ·')
  expect(html).toContain('running…')
  expect(html).toContain('versions · 1')
  // Older versions sit behind the closed disclosure; a running row has nothing to open yet.
  expect(html).not.toContain('href="/artifacts/call-briefing-1"')
  expect(html).toContain('href="/artifacts/call-timeline-1"')
  expect(html).toContain('failed · model timed out')
  expect(html).toContain('aria-label="Regenerate faq"')
  expect(html).not.toContain('No overviews yet')
})

test('the audio row plays overview.mp3 inline over the artifact bytes, shows the one-voice note and lists the script beneath', () => {
  const html = render([
    row({ kind: 'audio', version: 1, status: 'ready', name: 'overview.mp3', note: 'one voice available — both hosts read by peter_yearsley' }),
    row({ kind: 'audio', version: 1, status: 'ready', name: 'overview-script.md', artifact: 'call-script-1' }),
  ])
  expect(html).toContain('<audio controls')
  expect(html).toContain('src="blob:/data/artifacts/abc.mp3"')
  expect(html).toContain('one voice available — both hosts read by peter_yearsley')
  expect(html).toContain('overview-script.md')
  expect(html).toContain('href="/artifacts/call-script-1"')
  expect(html).not.toContain('overview-script.md · v1')
})

test('empty: the pointer to the header menu', () => {
  expect(render([])).toContain('No overviews yet — Overviews ▾ in the header generates a briefing, FAQ, timeline, mind map or an audio overview.')
})
