import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ComponentType, createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { MeetingSettings } from "@/rpc/bindings/method.meeting.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { DEFAULT_LIVE_TOOLS, MeetingDefaults, MeetingDetection, MeetingLive, added, echoSummary } from './Meetings'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/lib/tauri', () => ({ setHiddenFromCapture: vi.fn() }))
vi.mock('@/components/sidebar/ProjectStyle', () => ({ ProjectIcon: () => null }))

// Static markup drops handlers: the model comboboxes and switches register theirs by label so a test can "use" them.
const handlers: Record<string, ((v: never) => void) | undefined> = {}
type Effort = { levels: string[]; current?: string | null; onSelect(level: string | null): void }
const chips: Record<string, Effort | undefined> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ value, none, onChange, effort, 'aria-label': label }: { value: string | null | undefined; none?: { label: string }; onChange(id: string | null): void; effort?: Effort; 'aria-label': string }) => {
    handlers[label] = onChange
    chips[label] = effort
    handlers[`${label} effort`] = effort?.onSelect
    return createElement('span', { 'aria-label': label, 'data-levels': effort?.levels.join(','), 'data-effort': effort?.current ?? undefined }, value ?? none?.label)
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
const block: MeetingSettings = receiveComponent("method.meeting.output.MeetingSettings", {
  model: null,
  effort: null,
  summary_model: null,
  template: 'meeting',
  language: 'en',
  live_diarization: true,
  live_tools: DEFAULT_LIVE_TOOLS,
  project: null,
  detect: { enabled: true, ignore: ['com.apple.FaceTime', 'obs64.exe'], poll_secs: 2 },
  calendar: { eventkit: false, prebrief: true, poll_secs: 900 },
})
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
  meeting_list: vi.fn(async () => [{ id: 'm1' }, { id: 'm2' }, { id: 'm3' }]),
  meeting_get: vi.fn(async (id: string) => ({ id, stats: { m1: { echo_dupes: 3, mic_segments: 40, speakers: true }, m2: { echo_dupes: 1, mic_segments: 10, speakers: null }, m3: null }[id] })),
  projects_list: vi.fn(async () => []),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => vi.clearAllMocks())

async function render(tab: ComponentType): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(tab) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}
const write = (n: number) =>
  vi.waitFor(() => {
    expect(api.meeting_settings_set).toHaveBeenCalledTimes(n)
    return api.meeting_settings_set.mock.calls[n - 1]![0]
  })

test('§6.3: Defaults, Detection and Live each read the block once and show it', async () => {
  const defaults = await render(MeetingDefaults)
  expect(api.meeting_settings_get).toHaveBeenCalledTimes(1)
  expect(defaults).toMatch(/aria-label="Meeting model">Same as new sessions</)
  expect(defaults).toMatch(/aria-label="Summary model">Same as meeting model</)
  expect(defaults).toContain('English only until the multilingual tier')

  const detection = await render(MeetingDetection)
  expect(api.meeting_settings_get).toHaveBeenCalledTimes(2)
  expect(detection).toMatch(/aria-label="Detect meetings"/)
  expect(detection).toContain('com.apple.FaceTime')
  expect(detection).toContain('obs64.exe')
  expect(detection).toContain('aria-label="Remove com.apple.FaceTime"')

  const live = await render(MeetingLive)
  expect(api.meeting_settings_get).toHaveBeenCalledTimes(3)
  expect(live).toContain('meeting_topic')
  // Reset is idle while the list is the engine's default.
  expect(live).toMatch(/<button disabled="">Reset to default<\/button>/)
})

test('round-trip: each control writes the whole block with one field changed (§6.2), never a partial', async () => {
  await render(MeetingDefaults)
  handlers['Meeting model']?.(haiku.id as never)
  expect(await write(1)).toEqual({ ...block, model: haiku.id })
  handlers['Summary model']?.(haiku.id as never)
  expect(await write(2)).toEqual({ ...block, summary_model: haiku.id })
  await render(MeetingDetection)
  handlers['Detect meetings']?.(false as never)
  expect(await write(3)).toEqual({ ...block, detect: { ...block.detect, enabled: false } })
})

test('194: the summary model carries its effort chips at meeting.summary_effort and a chip writes it into the whole block; none while it follows the meeting model', async () => {
  await render(MeetingDefaults)
  expect(chips['Summary model']).toBeUndefined()
  const think = { id: 'anthropic/claude-haiku-5-5', name: 'Claude Haiku 5.5', efforts: ['low', 'high'] } as ModelInfo
  const picked = { ...block, summary_model: think.id, summary_effort: 'low' }
  api.meeting_settings_get.mockResolvedValueOnce(picked)
  api.list_models.mockResolvedValueOnce([haiku, think])
  await render(MeetingDefaults)
  expect(chips['Summary model']?.levels).toEqual(['low', 'high'])
  expect(chips['Summary model']?.current).toBe('low')
  chips['Summary model']?.onSelect('high')
  expect(await write(1)).toEqual({ ...picked, summary_effort: 'high' })
})

test('194 row 16: the meeting model carries `meeting.effort` as its chips; a chip writes the whole block with the level, Default with null', async () => {
  const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', efforts: ['low', 'high'] } as ModelInfo
  api.list_models.mockResolvedValue([haiku, sonnet])
  api.meeting_settings_get.mockResolvedValue({ ...block, model: sonnet.id, effort: 'high' })
  const html = await render(MeetingDefaults)
  expect(html).toContain(`aria-label="Meeting model" data-levels="low,high" data-effort="high">${sonnet.id}<`)
  handlers['Meeting model effort']?.('low' as never)
  expect(await write(1)).toEqual({ ...block, model: sonnet.id, effort: 'low' })
  handlers['Meeting model effort']?.(null as never)
  expect(await write(2)).toEqual({ ...block, model: sonnet.id, effort: null })
  // A model without levels has no chips (the engine drops a stored effort it does not take).
  api.meeting_settings_get.mockResolvedValue({ ...block, model: haiku.id })
  expect(await render(MeetingDefaults)).toContain(`aria-label="Meeting model">${haiku.id}<`)
  api.list_models.mockResolvedValue([haiku])
  api.meeting_settings_get.mockResolvedValue(block)
})

test('the ignore list and live tools: Reset writes the default list; added() trims, drops blanks and repeats', async () => {
  api.meeting_settings_get.mockResolvedValueOnce({ ...block, live_tools: ['meeting_topic'] })
  const html = await render(MeetingLive)
  expect(html).toMatch(/<button>Reset to default<\/button>/)
  handlers['Reset to default']?.(undefined as never)
  expect((await write(1)).live_tools).toEqual(DEFAULT_LIVE_TOOLS)
  expect(added(['a'], ' b ')).toEqual(['a', 'b'])
  expect(added(['a'], 'a')).toEqual(['a'])
  expect(added(['a'], '  ')).toEqual(['a'])
})

test('158 §13: the Echo row (not macOS here) sums stats over the last 10 manifests; unmeasured meetings are skipped', async () => {
  const html = await render(MeetingLive)
  expect(api.meeting_list).toHaveBeenCalledWith(10)
  expect(html).toContain('Echo in your microphone')
  expect(html).toContain('8 % of your microphone segments repeated the other side (4 of 50) over the last 2 meetings, 1 on speakers.')
  expect(echoSummary([])).toMatch(/^No meeting measured yet/)
})
