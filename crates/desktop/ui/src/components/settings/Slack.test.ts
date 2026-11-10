import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { Slack } from './Slack'

let manifestError: Error | null = null
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    slack_status: async () => ({ state: null, bot_user: null, team: null, owner: '', allowed: [], channels: [] }),
    slack_manifest: async () => {
      if (manifestError) throw manifestError
      return 'display_information:\n  name: theuth\n'
    },
  }),
}))

test('slack: "App manifest" is the first Slack row and renders before slack_status resolves (170 D2)', () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Slack) as ReactNode }))
  const slack = html.slice(html.indexOf('>Slack<'))
  expect(slack.indexOf('App manifest')).toBeGreaterThan(0)
  expect(slack).not.toContain('>Bot<')
  expect(slack).toMatch(/<button[^>]*disabled[^>]*>.*Copy manifest</)
})

test('slack: a failing slack_manifest is one error line; the row stays', async () => {
  manifestError = new Error('Method not found')
  try {
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false, retryOnMount: false } } })
    const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Slack) as ReactNode }))
    paint()
    for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey }).catch(() => undefined)
    const html = paint()
    expect(html).toContain('manifest unavailable: Method not found')
    expect(html).toContain('Open Slack apps')
  } finally {
    manifestError = null
  }
})
