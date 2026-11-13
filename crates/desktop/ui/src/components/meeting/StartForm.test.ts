import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { LAWFUL } from '@/lib/detection'
import { useMeetingSettings } from '@/lib/meeting-settings'
import type * as MeetingSettings from '@/lib/meeting-settings'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'
import { StartForm, startOptions } from './StartForm'

vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => {} }))
// No DOM in the test runner: the remembered pick writes through the smallest `Storage`.
vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => {} })
// Static markup reads a zustand store's initial state: the hook answers the live one instead.
vi.mock('@/lib/meeting-settings', async (original) => {
  const m = await original<typeof MeetingSettings>()
  const store = m.useMeetingSettings
  const live = Object.assign((pick: (s: object) => unknown) => pick(store.getState()), store) as typeof store
  return { ...m, useMeetingSettings: live }
})
// Static markup drops handlers: the model picker registers its own so a test can "use" it.
const handlers: Record<string, ((v: never) => void) | undefined> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ value, none, onChange, effort, 'aria-label': label }: { value: string | null | undefined; none?: { label: string }; onChange(id: string | null): void; effort?: { levels: string[]; current: string | null | undefined; onSelect(l: string | null): void }; 'aria-label': string }) => {
    handlers[label] = onChange
    handlers[`${label} effort`] = effort?.onSelect
    return createElement('span', { 'aria-label': label, 'data-levels': effort?.levels.join(','), 'data-effort': effort?.current ?? undefined }, value ?? none?.label)
  },
}))
const thinker = { id: 't/think', name: 'Thinker', efforts: ['low', 'high'] } as ModelInfo
const agentModel = { id: 'agent/new', name: 'New sessions', efforts: ['medium'] } as ModelInfo
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    meeting_processes: async () => [],
    meeting_permissions: async () => ({ microphone: 'Authorized', audio_capture: 'Authorized' }),
    list_models: async () => [thinker, agentModel],
    endless_settings_get: async () => ({ default_meeting: true, model: null }),
    meeting_templates: async () => [{ slug: 'standup', name: 'Standup', description: '', sections: [], builtin: true }],
    list_sessions: async () => [],
    list_project_styles: async () => [],
    meeting_list: async () => [],
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

test('158 §9.2: the Project row sits between Context and the model row and reads Default until a project is picked', async () => {
  const html = await render()
  expect(html).toMatch(/Context.*role="combobox"[^>]*aria-label="Project"[^>]*>.*Default.*Session model/s)
})

test('startOptions always sends consent: true, memory only with the switch on, project and template only when picked; the rest is unchanged', () => {
  expect(startOptions({ pid: 0, title: ' Standup ', context: '', model: 'm', endless: true, project: 'theuth', template: 'standup' })).toEqual({
    process: null,
    title: 'Standup',
    context: null,
    consent: true,
    session_model: 'm',
    memory: 'endless',
    project: 'theuth',
    template: 'standup',
  })
  const dflt = startOptions({ pid: 42, title: '', context: 'x', model: null, endless: false, project: null, template: null })
  expect(dflt).toMatchObject({ process: 42, consent: true, memory: undefined })
  // Absent, not `null`: `StartMeeting.project` / `.template` are `Option<String>` with `None` = the engine's remembered / default choice.
  expect(JSON.stringify(dflt)).not.toContain('project')
  expect(dflt.template).toBeUndefined()
})

test('194 row 17: `session_effort` rides with a picked `session_model` only', () => {
  const base = { pid: 0, title: '', context: '', endless: false, project: null }
  expect(startOptions({ ...base, model: 'a/b', effort: 'high' })).toMatchObject({ session_model: 'a/b', session_effort: 'high' })
  expect(startOptions({ ...base, model: 'a/b', effort: null }).session_effort).toBeUndefined()
  expect(startOptions({ ...base, model: null, effort: 'high' }).session_effort).toBeUndefined()
})

test('158 §11.1: the Note template row sits under the session model, Automatic by default', async () => {
  const html = await render()
  expect(html).toMatch(/Session model.*Note template.*aria-label="Note template"/s)
  expect(html).toContain('else the default from Settings › Meetings')
})

test('194 row 17 (R1): the pick is the start override — Meeting default, not the New sessions model, until one is picked; its chips set its effort; Meeting default clears both', async () => {
  useConnection.setState({ info: { default_model: agentModel.id } as never })
  useMeetingSettings.getState().setModel(null)
  let html = await render()
  expect(html).toMatch(/<span aria-label="Session model">Meeting default<\/span>/)
  expect(handlers['Session model effort']).toBeUndefined()
  handlers['Session model']?.(thinker.id as never)
  expect(useMeetingSettings.getState()).toMatchObject({ model: thinker.id, effort: null })
  html = await render()
  expect(html).toContain('data-levels="low,high"')
  handlers['Session model effort']?.('high' as never)
  expect(useMeetingSettings.getState()).toMatchObject({ model: thinker.id, effort: 'high' })
  html = await render()
  expect(html).toMatch(/data-effort="high"[^>]*>t\/think</)
  handlers['Session model']?.(null as never)
  expect(useMeetingSettings.getState()).toMatchObject({ model: null, effort: null })
  html = await render()
  expect(html).toContain('>Meeting default<')
  expect(html).not.toContain('data-levels')
})
