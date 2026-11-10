import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ComponentType, createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { HistoryRow } from '@/rpc/bindings/HistoryRow'
import { Dictation, Microphone, SYSTEM_DEFAULT, VoiceHistory, inputDevicesFor, phrases, timingsLine } from './Voice'

const row: HistoryRow = {
  id: 'd1',
  started_ms: BigInt(Date.now() - 3 * 60_000),
  finished_ms: BigInt(Date.now() - 3 * 60_000 + 4_000),
  surface: 'composer',
  destination: 'agent_prompt',
  app: null,
  raw: 'git hub comma twenty three',
  shaped: 'GitHub, 23',
  polished: null,
  delivered: 'GitHub, 23',
  press_ms: 42,
  first_partial_ms: 310,
  first_final_ms: 890,
  final_ms: 1_300,
  reason: 'done',
  audio_bytes: BigInt(128_000),
  has_audio: true,
}
const polishedRow: HistoryRow = { ...row, id: 'd2', surface: 'phone', raw: 'hello there', shaped: 'hello there', polished: 'Hello there.', has_audio: false }

const calls: Record<string, unknown[][]> = {}
const record =
  <T,>(name: string, reply: T) =>
  async (...args: unknown[]) => {
    ;(calls[name] ??= []).push(args)
    return reply
  }

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_input_devices: async () => [
      { name: 'MacBook Pro Microphone', default: true },
      { name: 'Shure MV7', default: false },
    ],
    voice_status: async () => ({ model: { state: 'ready' }, accelerator: '', voice: 'af', voices: [], speaking: null, queued: [], input_devices: ['Shure MV7'] }),
    voice_settings: record('voice_settings', {
      shape: { composer: 'plain', tui: 'full', companion: 'full', phone: 'off', any_app: 'full' },
      end_commands: { send: ['ship it', 'send it'], scratch: ['scratch that'], literal: 'verbatim' },
      input_devices: ['Shure MV7'],
      standby_secs: 8,
      duck_media: 'off',
      polish_model: null,
      polish_profiles: {},
      history: { enabled: true, rows: 200, audio_mib: 64, learn: true },
    }),
    list_models: async () => [{ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5' }],
    polish_model: async () => null,
    dictation_history: record('dictation_history', [row, polishedRow]),
    dictation_status: async () => ({
      model: { state: 'ready' },
      active: null,
      input_device: 'Shure MV7',
      last: { surface: 'composer', press_to_first_pcm_ms: 42, press_to_first_partial_ms: 310, press_to_first_final_ms: 890, stop_to_final_ms: 420, polish_ms: null, device: 'Shure MV7' },
    }),
    set_input_devices: record('set_input_devices', null),
  }),
}))

async function render(tab: ComponentType): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(tab) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('Microphone: the device list lands (the select is enabled); the turn chime sits beside it', async () => {
  const html = await render(Microphone)
  // Radix renders no value text server-side (the old whole-page match on "Shure MV7" hit the debug row instead).
  expect(html).toMatch(/<button(?![^>]*\sdisabled(?:=""|\s|>))[^>]*aria-label="Input device"/)
  expect(html).toContain('Turn chime')
})

test('Dictation: shaping + end phrases come from voice_settings (nothing remembered locally), the polish row names the chain', async () => {
  const html = await render(Dictation)
  expect(calls.voice_settings).toHaveLength(1)
  // Radix renders no value text server-side; the enabled trigger proves `voice_settings` landed, the phrase inputs its content.
  expect(html).toMatch(/<button(?![^>]*\sdisabled(?:=""|\s|>))[^>]*aria-label="Composer shaping"/)
  expect(html).toContain('value="ship it, send it"')
  expect(html).toContain('value="verbatim"')
  expect(html).toContain('Chain (title model, then session)')
  expect(html).toContain('never polished')
})

test('History: rows show raw → shaped (+ polished), the debug row the four numbers', async () => {
  const html = await render(VoiceHistory)
  expect(calls.dictation_history?.[0]).toEqual([50])
  expect(html).toContain('3 min ago · Composer')
  expect(html).toMatch(/line-through">git hub comma twenty three <\/span>→ GitHub, 23/)
  expect(html).toContain('polished: Hello there.')
  expect(html).toMatch(/aria-label="Play"/)
  expect(html).toContain('Composer · press → first PCM 42 ms · → first partial 310 ms · → first final 890 ms · stop → final 420 ms · Shure MV7')
})

test('timingsLine: missing numbers print as —, polish only when it ran, no dictation yet', () => {
  expect(timingsLine(null)).toBe('No dictation yet.')
  expect(timingsLine({ surface: 'phone', press_to_first_pcm_ms: 10, press_to_first_partial_ms: null, press_to_first_final_ms: null, stop_to_final_ms: 5, polish_ms: 700, device: null })).toBe(
    'Phone · press → first PCM 10 ms · → first partial — · → first final — · stop → final 5 ms · polish 700 ms',
  )
})

test('phrases: comma-separated, trimmed, lower-cased, empties dropped', () => {
  expect(phrases(' Send It, ship it ,, ')).toEqual(['send it', 'ship it'])
  expect(phrases('')).toEqual([])
})

test('set_input_devices call shape (§8.1): the picked name alone, "System default" = the empty list', () => {
  expect(inputDevicesFor('Shure MV7')).toEqual(['Shure MV7'])
  expect(inputDevicesFor(SYSTEM_DEFAULT)).toEqual([])
})
