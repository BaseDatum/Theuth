import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { MeetingSettings } from '@/rpc/bindings/MeetingSettings'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { DEFAULT_LIVE_TOOLS, Meetings, added } from './Meetings'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children?: ReactNode }) => createElement('a', null, children) }))
vi.mock('@/lib/tauri', () => ({ setHiddenFromCapture: vi.fn() }))
vi.mock('@/components/sidebar/ProjectStyle', () => ({ ProjectIcon: () => null }))

// Static markup drops handlers: the model comboboxes and switches register theirs by label so a test can "use" them.
const handlers: Record<string, ((v: never) => void) | undefined> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ value, none, onChange, 'aria-label': label }: { value: string | null | undefined; none?: { label: string }; onChange(id: string | null): void; 'aria-label': string }) => {
    handlers[label] = onChange
    return createElement('span', { 'aria-label': label }, value ?? none?.label)
  },
}))
vi.mock('@/components/ui/switch', () => ({
  Switch: ({ checked, onCheckedChange, 'aria-label': label }: { checked: boolean; onCheckedChange(on: boolean): void; 'aria-label'?: string }) => {
    if (label) handlers[label] = onCheckedChange
    return createElement('button', { role: 'switch', 'aria-checked': checked, 'aria-label': label })
  },
}))
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, onClick, disabled }: { children?: ReactNode; onClick?: () => void; disabled?: boolean }) => {
    const label = Array.isArray(children) ? children.join('') : String(children)
    handlers[label] = onClick
    return createElement('button', { disabled }, children)
  },
}))

const haiku = { id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5', efforts: [] as string[] } as ModelInfo
const block: MeetingSettings = {
  model: null,
  summary_model: null,
  template: 'meeting',
  language: 'en',
  live_diarization: true,
  live_tools: DEFAULT_LIVE_TOOLS,
  project: null,
  detect: { enabled: true, ignore: ['com.apple.FaceTime', 'obs64.exe'], poll_secs: 2 },
  calendar: { eventkit: false, prebrief: true, poll_secs: 900 },
}
const api = {
  meeting_settings_get: vi.fn(async () => block),
  meeting_settings_set: vi.fn(async (s: MeetingSettings) => s),
  meeting_templates: vi.fn(async () => []),
  list_models: vi.fn(async () => [haiku]),
  list_sessions: vi.fn(async () => [{ id: 's1', project: 'theuth' }]),
  list_project_styles: vi.fn(async () => [{ name: 'ops', icon: null, color: null }]),
  meeting_speakers: vi.fn(async () => []),
  meeting_speaker_delete: vi.fn(),
  meeting_live_diarization: vi.fn(async () => true),
  set_meeting_live_diarization: vi.fn(async (on: boolean) => on),
  decision_mode: vi.fn(async () => ({ effective: 'off' })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => vi.clearAllMocks())

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Meetings) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}
const write = (n: number) =>
  vi.waitFor(() => {
    expect(api.meeting_settings_set).toHaveBeenCalledTimes(n)
    return api.meeting_settings_set.mock.calls[n - 1]![0]
  })

test('§6.3: Defaults, Detection and Live tools lead, above Screen capture; the block is read once and shown', async () => {
  const html = await render()
  const at = (s: string) => html.indexOf(s)
  expect(at('Defaults')).toBeGreaterThan(-1)
  expect(at('Defaults')).toBeLessThan(at('Detection'))
  expect(at('Detection')).toBeLessThan(at('Live tools'))
  expect(at('Live tools')).toBeLessThan(at('Screen capture'))
  expect(api.meeting_settings_get).toHaveBeenCalledTimes(1)
  expect(html).toMatch(/aria-label="Meeting model">Same as new sessions</)
  expect(html).toMatch(/aria-label="Summary model">Same as meeting model</)
  expect(html).toContain('English only until the multilingual tier')
  expect(html).toMatch(/aria-label="Detect meetings"/)
  expect(html).toContain('com.apple.FaceTime')
  expect(html).toContain('obs64.exe')
  expect(html).toContain('aria-label="Remove com.apple.FaceTime"')
  expect(html).toContain('meeting_topic')
  // Reset is idle while the list is the engine's default.
  expect(html).toMatch(/<button disabled="">Reset to default<\/button>/)
})

test('round-trip: each control writes the whole block with one field changed (§6.2), never a partial', async () => {
  await render()
  handlers['Meeting model']?.(haiku.id as never)
  expect(await write(1)).toEqual({ ...block, model: haiku.id })
  handlers['Summary model']?.(haiku.id as never)
  expect(await write(2)).toEqual({ ...block, summary_model: haiku.id })
  handlers['Detect meetings']?.(false as never)
  expect(await write(3)).toEqual({ ...block, detect: { ...block.detect, enabled: false } })
})

test('the ignore list and live tools: Reset writes the default list; added() trims, drops blanks and repeats', async () => {
  api.meeting_settings_get.mockResolvedValueOnce({ ...block, live_tools: ['meeting_topic'] })
  const html = await render()
  expect(html).toMatch(/<button>Reset to default<\/button>/)
  handlers['Reset to default']?.(undefined as never)
  expect((await write(1)).live_tools).toEqual(DEFAULT_LIVE_TOOLS)
  expect(added(['a'], ' b ')).toEqual(['a', 'b'])
  expect(added(['a'], 'a')).toEqual(['a'])
  expect(added(['a'], '  ')).toEqual(['a'])
})
