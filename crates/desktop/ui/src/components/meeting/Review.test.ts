import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { Enrolled } from '@/rpc/bindings/Enrolled'
import type { MeetingTranscript } from '@/rpc/bindings/MeetingTranscript'
import type { Segment } from '@/rpc/bindings/Segment'
import type { SpeakerSuggestion } from '@/rpc/bindings/SpeakerSuggestion'
import { Review } from './Review'

const toast = vi.hoisted(() => Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), message: vi.fn() }))
vi.mock('sonner', () => ({ toast }))
vi.mock('@/hooks/use-audio-context', () => ({ getSharedAudioContext: () => null }))
vi.mock('@/lib/segment-audio', () => ({ SegmentPlayer: class {}, playError: String }))
vi.mock('./EnrollDialog', () => ({ EnrollDialog: () => null }))
vi.mock('./SegmentMenu', () => ({ SegmentMenu: () => null }))

// Static markup drops handlers, so the buttons register theirs by aria-label or text; a test "clicks" by name.
const handlers: Record<string, (() => void) | undefined> = {}
const text = (c: ReactNode): string => (Array.isArray(c) ? c.map(text).join('') : typeof c === 'string' || typeof c === 'number' ? String(c) : '')
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, onClick, disabled, 'aria-label': label }: { children?: ReactNode; onClick?: () => void; disabled?: boolean; 'aria-label'?: string }) => {
    handlers[label ?? text(children).trim()] = onClick
    return createElement('button', { disabled, 'aria-label': label }, children)
  },
}))
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuTrigger: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuItem: ({ children }: { children?: ReactNode }) => children,
}))
vi.mock('@/components/ui/select', () => ({
  Select: ({ children }: { children?: ReactNode }) => children,
  SelectTrigger: ({ children }: { children?: ReactNode }) => children,
  SelectValue: () => null,
  SelectContent: ({ children }: { children?: ReactNode }) => children,
  SelectItem: ({ children }: { children?: ReactNode }) => children,
}))

const ID = 'm1'
const seg = (id: number, speaker: string, t0: number): Segment => ({
  id,
  track: speaker === 'me' ? 'Mic' : 'System',
  speaker,
  t0_ms: BigInt(t0),
  t1_ms: BigInt(t0 + 4000),
  text: 'hello there',
  words: [
    { text: 'hello', t0_ms: BigInt(t0), t1_ms: BigInt(t0 + 2000) },
    { text: 'there', t0_ms: BigInt(t0 + 2000), t1_ms: BigInt(t0 + 4000) },
  ],
})
const transcript: MeetingTranscript = {
  meeting: ID,
  speakers: [
    { id: 'me', name: 'Me', profile: null, proposed: null },
    { id: 's1', name: 'Speaker 1', profile: null, proposed: null },
    { id: 's2', name: 'Speaker 2', profile: null, proposed: null },
  ],
  segments: [seg(1, 'me', 0), seg(2, 's1', 5000), seg(3, 's2', 10_000), seg(4, 's1', 15_000)],
  reviewed_at: null,
  summarized_at: null,
  diarized: true,
  note: null,
}
const priya: SpeakerSuggestion = { label: 's1', name: 'Priya Shah', email: 'priya@example.com', profile: null, confidence: 0.84, source: 'Vocative', evidence: 'attendee · "thanks, Priya" at 04:12' }
const tom: SpeakerSuggestion = { label: 's2', name: 'Tom Okafor', email: null, profile: 'p-tom', confidence: 0.66, source: 'Voiceprint', evidence: 'voiceprint 0.71 (next best 0.52)' }
const renamed = (name: string): MeetingTranscript => ({ ...transcript, speakers: transcript.speakers.map((s) => (s.id === 's1' ? { ...s, name } : s)) })
const enrolled: Enrolled = {
  profile: { id: 'p-priya', name: 'Priya Shah', entity: null, samples: 1, created_at: '', updated_at: '' },
  speech_ms: 8000,
  available_ms: 8000,
  transcript: { ...renamed('Priya Shah'), speakers: renamed('Priya Shah').speakers.map((s) => (s.id === 's1' ? { ...s, profile: 'p-priya' } : s)) },
}
const api = {
  meeting_transcript: vi.fn(async () => transcript),
  meeting_suggest_speakers: vi.fn(async (): Promise<SpeakerSuggestion[]> => [priya, tom]),
  meeting_edit: vi.fn(async () => renamed('Priya Shah')),
  meeting_speaker_enroll: vi.fn(async () => enrolled),
  meeting_speakers: vi.fn(async () => []),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => {
  vi.clearAllMocks()
  for (const k of Object.keys(handlers)) delete handlers[k]
})

async function render(suggestions?: SpeakerSuggestion[], readOnly = false): Promise<{ html: string; qc: QueryClient }> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  if (suggestions) qc.setQueryData(queryKeys.meetingSuggestions(ID), suggestions)
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Review, { id: ID, readOnly }) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll({ queryKey: queryKeys.meetingTranscript(ID) })) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return { html: paint(), qc }
}
const press = (label: string) => {
  const h = handlers[label]
  if (!h) throw new Error(`no control "${label}" (have ${Object.keys(handlers).join(', ')})`)
  h()
}

test('Suggest names sits with the speaker chips in Review only; nothing is fetched until it is pressed', async () => {
  const { html, qc } = await render()
  expect(html).toContain('Suggest names')
  expect(api.meeting_suggest_speakers).not.toHaveBeenCalled()
  expect(html).not.toContain('Nothing changes until you accept')
  press('Suggest names')
  await vi.waitFor(() => expect(qc.getQueryData(queryKeys.meetingSuggestions(ID))).toEqual([priya, tom]))
  expect(api.meeting_suggest_speakers).toHaveBeenCalledWith(ID)
  expect((await render(undefined, true)).html).not.toContain('Suggest names')
})

test('one row per suggestion: speaker → name, the confidence bar, the evidence line, Accept / Not them; me and already-applied names are skipped', async () => {
  const { html } = await render([priya, tom, { ...priya, label: 'me', name: 'Dana' }, { ...tom, name: 'Speaker 2' }])
  expect(html).toContain('Speaker 1</span><span class="text-fg-tertiary">→</span><b class="font-medium">Priya Shah</b>')
  expect(html).toContain('0.84')
  expect(html).toContain('aria-valuenow="84"')
  expect(html).toContain('style="width:84%"')
  expect(html).toContain('attendee · &quot;thanks, Priya&quot; at 04:12')
  expect(html).toContain('Tom Okafor')
  expect(html).not.toContain('Dana')
  expect(html.match(/aria-label="Accept /g)).toHaveLength(2)
  expect(html).toContain('accepting can enroll the voice when the sample passes the quality gates')
})

test('Accept renames through meeting_edit, then enrolls the label\u2019s segments as New; both answers replace the cached transcript', async () => {
  const { qc } = await render([priya])
  press('Accept Priya Shah for Speaker 1')
  await vi.waitFor(() => expect(api.meeting_speaker_enroll).toHaveBeenCalledTimes(1))
  expect(api.meeting_edit).toHaveBeenCalledWith(ID, [{ op: 'Rename', speaker: 's1', name: 'Priya Shah' }])
  expect(api.meeting_speaker_enroll).toHaveBeenCalledWith(ID, [2, 4], { kind: 'New', name: 'Priya Shah', entity: null, gated: true })
  await vi.waitFor(() => expect(qc.getQueryData(queryKeys.meetingTranscript(ID))).toEqual(enrolled.transcript))
  expect(qc.getQueryData(queryKeys.meetingSuggestions(ID))).toEqual([])
  expect(toast.success).toHaveBeenCalledTimes(1)
})

test('a voiceprint suggestion (profile set) is accepted by the rename alone; a refused enrollment keeps the name and says why', async () => {
  const { qc } = await render([tom])
  press('Accept Tom Okafor for Speaker 2')
  await vi.waitFor(() => expect(api.meeting_edit).toHaveBeenCalledWith(ID, [{ op: 'Rename', speaker: 's2', name: 'Tom Okafor' }]))
  expect(api.meeting_speaker_enroll).not.toHaveBeenCalled()
  expect(qc.getQueryData(queryKeys.meetingSuggestions(ID))).toEqual([])

  api.meeting_speaker_enroll.mockRejectedValueOnce(new Error('span has 4 s of speech; 5 s needed'))
  await render([priya])
  press('Accept Priya Shah for Speaker 1')
  await vi.waitFor(() => expect(toast.message).toHaveBeenCalledWith('Priya Shah named; voice not enrolled', { description: 'span has 4 s of speech; 5 s needed' }))
  expect(toast.error).not.toHaveBeenCalled()
})

test('Not them drops the row from the cached list without any RPC', async () => {
  const { qc } = await render([priya, tom])
  press('Not Priya Shah')
  expect(qc.getQueryData(queryKeys.meetingSuggestions(ID))).toEqual([tom])
  expect(api.meeting_edit).not.toHaveBeenCalled()
  expect(api.meeting_speaker_enroll).not.toHaveBeenCalled()
})
