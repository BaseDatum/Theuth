import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { LAWFUL } from '@/lib/detection'
import { StartForm, startOptions } from './StartForm'

vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => {} }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    meeting_processes: async () => [],
    meeting_permissions: async () => ({ microphone: 'Authorized', audio_capture: 'Authorized' }),
    list_models: async () => [],
    endless_settings_get: async () => ({ default_meeting: true, model: null }),
    meeting_templates: async () => [{ slug: 'standup', name: 'Standup', description: '', sections: [], builtin: true }],
  }),
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(StartForm) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('the ∞ switch follows agent.endless.default_meeting and sits under the model row with its one-line note', async () => {
  const html = await render()
  expect(html).toMatch(/Session model.*aria-checked="true"[^>]*aria-label="Endless memory"/s)
  expect(html).toContain('Keeps a compressed memory of the whole meeting behind the live transcript; zoom and recall bring any earlier moment back.')
})

test('158 R3 / A2: the form shows the one lawful sentence and no consent checkbox; Start is not gated on it', async () => {
  const html = await render()
  expect(html).toContain(LAWFUL)
  // The only checkbox left is the ∞ switch's hidden input; the consent label and its 53 §0.14 wording are gone.
  expect(html).not.toMatch(/<label[^>]*>\s*<input[^>]*type="checkbox"/)
  expect(html).not.toContain('I acknowledge I understand the law')
  const start = html.match(/<button[^>]*>[^<]*Start<\/button>/)?.[0]
  expect(start).toBeDefined()
  expect(start).not.toContain('disabled=""')
})

test('startOptions always sends consent: true and memory only with the switch on; template only when picked', () => {
  expect(startOptions({ pid: 0, title: ' Standup ', context: '', model: 'm', endless: true, template: 'standup' })).toEqual({
    process: null,
    title: 'Standup',
    context: null,
    consent: true,
    session_model: 'm',
    memory: 'endless',
    template: 'standup',
  })
  const auto = startOptions({ pid: 42, title: '', context: 'x', model: null, endless: false, template: null })
  expect(auto).toMatchObject({ process: 42, consent: true, memory: undefined })
  expect(auto.template).toBeUndefined()
})

test('158 §11.1: the Note template row sits under the session model, Automatic by default', async () => {
  const html = await render()
  expect(html).toMatch(/Session model.*Note template.*aria-label="Note template"/s)
  expect(html).toContain('else the default from Settings › Meetings')
})
