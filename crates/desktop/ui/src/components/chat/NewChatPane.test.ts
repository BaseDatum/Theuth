import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { createOptions } from './Composer'
import { CreateOptions } from './NewChatPane'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ list_sessions: async () => [] }),
}))

const render = (props: Partial<Parameters<typeof CreateOptions>[0]>) =>
  renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(
        TooltipProvider,
        null,
        createElement(CreateOptions, {
          priv: false,
          onPriv() {},
          project: null,
          onProject() {},
          endless: false,
          onEndless() {},
          settings: undefined,
          ...props,
        }) as ReactNode,
      ),
    }),
  )

test('the ∞ switch sits in the options row after Private; off, no notice', () => {
  const html = render({})
  expect(html).toMatch(/aria-label="Private session".*aria-checked="false"[^>]*aria-label="Endless memory".*>Project</s)
  expect(html).not.toContain('Every turn starts from a compressed view')
})

test('on: the neutral notice with the three sentences and the compactor line; the model name when one is set', () => {
  const html = render({ endless: true })
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Endless memory"/)
  expect(html).toMatch(/border-border-subtle bg-muted\/40[^"]*"><p class="font-medium">Endless memory<\/p>/)
  expect(html).not.toMatch(/bg-warning-bg.*Endless memory/s)
  expect(html).toContain('Every turn starts from a compressed view of the whole chat; nothing is deleted, the distant past just gets coarser.')
  expect(html).toContain('A background summariser keeps the view fresh; the first answer after a long turn may wait a few seconds for it.')
  expect(html).toContain('zoom and recall bring any past message back word for word.')
  expect(html).toContain('Compactor: same as the session')
  const named = render({
    endless: true,
    settings: { default_chat: true, default_desk: false, view_bytes: 64000, node_bytes: 512, slack: 0.125, settle_secs: 45, jobs: 8, tries: 5, retry_secs: 10, cap_chars: 30000, model: 'anthropic/claude-haiku-4-5' },
  })
  expect(named).toContain('Compactor: anthropic/claude-haiku-4-5')
})

test('createOptions: memory is endless only when the switch is on; private and project ride along unchanged', () => {
  expect(createOptions({ private: false, project: null, endless: true })).toMatchObject({ retention: 'durable', project: null, view: null, memory: 'endless' })
  expect(createOptions({ private: true, project: 'lab', endless: false })).toMatchObject({ retention: 'private', project: 'lab', memory: null })
  expect(createOptions({ private: true, project: null, endless: false }).view).not.toBeNull()
})
