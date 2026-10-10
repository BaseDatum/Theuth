import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { NEW_TEMPLATE, templateBody } from '@/lib/meeting-templates'
import type { MeetingTemplate } from '@/rpc/bindings/MeetingTemplate'
import { type Draft, MeetingTemplates } from './MeetingTemplates'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
// CodeMirror needs a document; the editor is a marker carrying its value.
vi.mock('@/components/agents/CharterEditor', () => ({ CharterEditor: ({ value }: { value: string }) => createElement('textarea', { 'data-editor': '', defaultValue: value }) }))

// Static markup drops handlers, so the buttons register theirs by aria-label or text; a test "clicks" by name.
const handlers: Record<string, (() => void) | undefined> = {}
const text = (c: ReactNode): string => (Array.isArray(c) ? c.map(text).join('') : typeof c === 'string' || typeof c === 'number' ? String(c) : '')
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, onClick, disabled, title, 'aria-label': label }: { children?: ReactNode; onClick?: () => void; disabled?: boolean; title?: string; 'aria-label'?: string }) => {
    handlers[label ?? text(children).trim()] = onClick
    return createElement('button', { disabled, title, 'aria-label': label }, children)
  },
}))

const meeting: MeetingTemplate = {
  slug: 'meeting',
  name: 'General',
  description: 'Any meeting',
  sections: [
    { title: 'Summary', instruction: null, required: false },
    { title: 'Decisions', instruction: null, required: false },
  ],
  builtin: true,
  body: '---\nname: General\ndescription: Any meeting\nsections:\n  - title: Summary\n  - title: Decisions\n---\n# my notes\n',
  edited: true,
}
const retro: MeetingTemplate = {
  slug: 'retro',
  name: 'Retro',
  description: '',
  sections: [{ title: 'Went well', instruction: 'bullets', required: true }],
  builtin: false,
  body: '---\nname: Retro\nsections:\n  - title: Went well\n    instruction: bullets\n    required: true\n---\n',
  edited: false,
}
const api = {
  meeting_templates: vi.fn(async () => [meeting, retro]),
  meeting_template_put: vi.fn(async (slug: string) => ({ ...retro, slug })),
  meeting_template_delete: vi.fn(async () => null),
  meeting_template_reset: vi.fn(async () => ({ ...meeting, edited: false })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => {
  vi.clearAllMocks()
  for (const k of Object.keys(handlers)) delete handlers[k]
})

async function render(initial?: Draft): Promise<{ html: string; qc: QueryClient }> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(MeetingTemplates, { initial }) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return { html: paint(), qc }
}
const press = (label: string) => {
  const h = handlers[label]
  if (!h) throw new Error(`no control "${label}" (have ${Object.keys(handlers).join(', ')})`)
  h()
}

test('lists every template with its sections; built-ins are badged and their Delete is disabled with the reason', async () => {
  const { html } = await render()
  expect(api.meeting_templates).toHaveBeenCalledTimes(1)
  expect(html).toContain('General')
  expect(html).toContain('built-in')
  expect(html).toContain('Any meeting · Summary, Decisions')
  expect(html).toContain('retro.md · Went well')
  expect(html).toMatch(/<button disabled="" title="Built-in templates cannot be deleted" aria-label="Delete General">/)
  expect(html).toMatch(/<button aria-label="Delete Retro">/)
  expect(html).not.toContain('data-editor')
})

test('Delete calls meeting_template_delete with the slug and re-reads the list', async () => {
  const { qc } = await render()
  press('Delete Retro')
  await vi.waitFor(() => expect(api.meeting_template_delete).toHaveBeenCalledWith('retro'))
  await vi.waitFor(() => expect(qc.getQueryState(['meeting-templates'])?.isInvalidated).toBe(true))
})

test('New template opens the editor on templateBody(NEW_TEMPLATE); Save puts the slugified file name and the body', async () => {
  const body = templateBody(NEW_TEMPLATE)
  const { html } = await render({ slug: 'Stand Up!', isNew: true, body })
  expect(html).toContain('data-editor')
  expect(html).toContain('  - title: &quot;Summary&quot;')
  expect(html).toContain('stand-up.md')
  press('Save')
  await vi.waitFor(() => expect(api.meeting_template_put).toHaveBeenCalledWith('stand-up', body))
})

test('Edit opens the file itself (body, verbatim) and Save writes it back unchanged; a new slug that already exists cannot be saved', async () => {
  const { html } = await render({ slug: 'retro', isNew: false, body: retro.body })
  expect(html).toContain('retro.md')
  expect(html).toContain('    instruction: bullets')
  expect(html).toContain('    required: true')
  press('Save')
  await vi.waitFor(() => expect(api.meeting_template_put).toHaveBeenCalledWith('retro', retro.body))

  const dup = await render({ slug: 'Meeting', isNew: true, body: templateBody(NEW_TEMPLATE) })
  expect(dup.html).toContain('already exists')
  expect(dup.html).toMatch(/<button disabled="">Save<\/button>/)
})

test('Reset to built-in shows only on an edited built-in and calls meeting_template_reset', async () => {
  const { html, qc } = await render()
  expect(html).toMatch(/<button aria-label="Reset General to built-in">/)
  expect(html).not.toContain('Reset Retro')
  press('Reset General to built-in')
  await vi.waitFor(() => expect(api.meeting_template_reset).toHaveBeenCalledWith('meeting'))
  await vi.waitFor(() => expect(qc.getQueryState(['meeting-templates'])?.isInvalidated).toBe(true))
  api.meeting_templates.mockResolvedValueOnce([{ ...meeting, edited: false }, retro])
  const pristine = await render()
  expect(pristine.html).not.toContain('Reset General')
})
