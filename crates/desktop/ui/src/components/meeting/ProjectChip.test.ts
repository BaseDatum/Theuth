import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { ProjectChip } from './ProjectChip'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({ list_sessions: async () => [], list_project_styles: async () => [] }) }))

function render(value: string | null, empty: string): string {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(ProjectChip, { value, empty, onChange: () => {} }) as ReactNode }))
}

test('the trigger is a combobox that reads the project, or the caller\u2019s empty label for null (the start form\u2019s Default, the header\u2019s No project)', () => {
  expect(render('theuth', 'Default')).toMatch(/role="combobox"[^>]*aria-label="Project"[^>]*>.*theuth/s)
  expect(render(null, 'Default')).toContain('>Default<')
  expect(render(null, 'No project')).toContain('>No project<')
})
