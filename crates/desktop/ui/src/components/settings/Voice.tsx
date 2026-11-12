// Settings › Voice (153 §8.1, §4.5, §7.2, §9.2, §6.7, §8.7): the microphone, the shaping preset per surface, the
// spoken end phrases, the polish model, the dictation history and the latency debug row. Everything reads and writes
// the engine except the talk view's turn chime (client-only, `lib/talk-settings.ts`). 188: one component per tab —
// Microphone (with the chime, from Chat › Talk), Dictation, History; the query keys below are what they share.

import { useQuery } from '@tanstack/react-query'
import { Play } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { pcmSamples, playPcm } from '@/components/ui/DictionarySuggestion'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { ago } from '@/lib/codemap/map'
import { queryKeys, useApi } from '@/lib/rpc'
import { useTalkSettings } from '@/lib/talk-settings'
import type { DictationTimings } from '@/rpc/bindings/DictationTimings'
import type { EndPhrases } from '@/rpc/bindings/EndPhrases'
import type { HistoryRow } from '@/rpc/bindings/HistoryRow'
import type { Preset } from '@/rpc/bindings/Preset'
import type { Surface } from '@/rpc/bindings/Surface'

const DEFAULT_DEVICE = '__default__'
const PRESETS: [Preset, string][] = [
  ['full', 'Full'],
  ['plain', 'Plain'],
  ['off', 'Off'],
]
const SURFACES: [Surface, string, string][] = [
  ['composer', 'Composer', 'The chat box and every text field of this app.'],
  ['phone', 'Phone', 'The webview on a phone.'],
]
/** §7.2 defaults. */
const END_DEFAULTS: EndPhrases = { send: ['send it'], scratch: ['scratch that'], literal: 'literal' }
const HISTORY = 50
const keys = {
  devices: ['voice', 'input-devices'] as const,
  status: ['voice', 'status'] as const,
  settings: ['voice', 'settings'] as const,
  polishModel: ['polish-model'] as const,
  /** `polish_effort()` (194); refetched with `polishModel` (a model change may drop it). */
  polishEffort: ['polish-effort'] as const,
  history: ['voice', 'history'] as const,
  dictation: ['voice', 'dictation-status'] as const,
}

/** `ms` as the debug row prints it. */
const ms = (v: number | null): string => (v === null ? '—' : `${v} ms`)

/** §8.1: the `set_input_devices` list for a pick — the chosen name first (the engine falls back to the default), "System default" = `[]`. */
export function inputDevicesFor(choice: string): string[] {
  return choice === DEFAULT_DEVICE ? [] : [choice]
}
/** The Select value for "System default". */
export const SYSTEM_DEFAULT = DEFAULT_DEVICE

/** The §8.7 debug line: "Composer · mic 42 ms · first words 310 ms · final 890 ms · stop → final 420 ms · Shure MV7". */
export function timingsLine(t: DictationTimings | null | undefined): string {
  if (!t) return 'No dictation yet.'
  const parts = [
    SURFACES.find(([s]) => s === t.surface)?.[1] ?? t.surface,
    `press → first PCM ${ms(t.press_to_first_pcm_ms)}`,
    `→ first partial ${ms(t.press_to_first_partial_ms)}`,
    `→ first final ${ms(t.press_to_first_final_ms)}`,
    `stop → final ${ms(t.stop_to_final_ms)}`,
  ]
  if (t.polish_ms !== null) parts.push(`polish ${ms(t.polish_ms)}`)
  if (t.device) parts.push(t.device)
  return parts.join(' · ')
}

/** Comma-separated phrases → trimmed, non-empty, lower-cased list. */
export function phrases(raw: string): string[] {
  return raw
    .split(',')
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean)
}

/** Voice › Microphone: the input device and the talk view's turn chime. */
export function Microphone() {
  const api = useApi()
  const devices = useQuery({ queryKey: keys.devices, queryFn: () => api.list_input_devices() })
  const status = useQuery({ queryKey: keys.status, queryFn: () => api.voice_status() })
  const setDevice = useAction((name: string) => api.set_input_devices(inputDevicesFor(name)), { keys: [keys.status, keys.settings] })
  const cue = useTalkSettings((s) => s.cue)

  const preferred = status.data?.input_devices[0]
  // A preferred device the host no longer enumerates stays visible, never silently "System default".
  const deviceNames = devices.data?.map((d) => d.name) ?? []
  const device = preferred ?? DEFAULT_DEVICE

  return (
    <>
      <Section title="Microphone">
        <Row
          label="Input device"
          description={devices.isError ? `Could not list microphones: ${devices.error.message}` : 'Dictation opens this microphone first and falls back to the system default. The companion reads the same choice.'}
        >
          <Select value={device} disabled={!devices.isSuccess || setDevice.isPending} onValueChange={(v) => setDevice.mutate(v)}>
            <SelectTrigger className="w-56" aria-label="Input device">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT_DEVICE}>System default</SelectItem>
              {preferred && !deviceNames.includes(preferred) && <SelectItem value={preferred}>{preferred} (not connected)</SelectItem>}
              {devices.data?.map((d) => (
                <SelectItem key={d.name} value={d.name}>
                  {d.name}
                  {d.default ? ' (default)' : ''}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
      </Section>
      <Section title="Talk">
        <Row label="Turn chime" description="A soft two-tone chime when you stop talking and the model takes the turn.">
          <Switch checked={cue} onCheckedChange={useTalkSettings.getState().setCue} />
        </Row>
      </Section>
    </>
  )
}

/** Voice › Dictation: how dictated words are shaped, the spoken end phrases and the polish model. */
export function Dictation() {
  const api = useApi()
  const settings = useQuery({ queryKey: keys.settings, queryFn: () => api.voice_settings() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const polishModel = useQuery({ queryKey: keys.polishModel, queryFn: () => api.polish_model() })
  const polishEffort = useQuery({ queryKey: keys.polishEffort, queryFn: () => api.polish_effort() })

  const setShape = useAction(({ surface, preset }: { surface: Surface; preset: Preset }) => api.set_shape(surface, preset), { keys: [keys.settings] })
  const setEnd = useAction((p: EndPhrases) => api.set_end_phrases(p), { keys: [keys.settings] })
  const setPolishModel = useAction((id: string | null) => api.set_polish_model(id), { keys: [keys.polishModel, keys.polishEffort, keys.settings] })
  const setPolishEffort = useAction((level: string | null) => api.set_polish_effort(level), { keys: [keys.polishEffort] })
  const polish = models.data?.find((m) => m.id === polishModel.data?.id)

  const shape = settings.data?.shape
  const end = settings.data?.end_commands ?? END_DEFAULTS

  return (
    <>
      <Section title="Shaping">
        {SURFACES.map(([surface, label, description]) => (
          <Row key={surface} label={label} description={`${description} Full writes punctuation, numbers, code words, dictionary terms and takes "send it" / "scratch that"; Plain skips the dictionary and the end phrases; Off keeps the model's words.`}>
            <Select
              value={shape?.[surface] ?? 'full'}
              disabled={!settings.isSuccess || setShape.isPending}
              onValueChange={(v) => setShape.mutate({ surface, preset: v as Preset })}
            >
              <SelectTrigger className="w-32" aria-label={`${label} shaping`}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PRESETS.map(([id, name]) => (
                  <SelectItem key={id} value={id}>
                    {name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Row>
        ))}
      </Section>
      <Section title="End phrases">
        <PhraseRow
          key={`send:${end.send.join(',')}`}
          label="Send"
          description="Said at the end of a dictation, sends the message after 1.5 s of silence. Comma-separated."
          value={end.send.join(', ')}
          commit={(raw) => setEnd.mutate({ ...end, send: phrases(raw) })}
        />
        <PhraseRow
          key={`scratch:${end.scratch.join(',')}`}
          label="Scratch"
          description="Drops the last closed utterance. Comma-separated."
          value={end.scratch.join(', ')}
          commit={(raw) => setEnd.mutate({ ...end, scratch: phrases(raw) })}
        />
        <PhraseRow
          key={`literal:${end.literal}`}
          label="Literal"
          description="Say this before a phrase or symbol word to write it as said."
          value={end.literal}
          commit={(raw) => setEnd.mutate({ ...end, literal: raw.trim().toLowerCase() || END_DEFAULTS.literal })}
        />
      </Section>
      <Section title="Polish">
        <Row
          stack
          label="Polish model"
          description="Corrects dictation meant for people — documents, notes and message replies — for a human reader. What you dictate to the agent is never polished; it reads the shaped words as spoken."
        >
          <ModelCombobox
            aria-label="Polish model"
            className="max-w-64 max-md:w-full max-md:max-w-none"
            models={models.data ?? []}
            value={polishModel.data?.id}
            none={{ label: 'Chain (title model, then session)' }}
            effort={polish && polish.efforts.length > 0 ? { levels: polish.efforts, current: polishEffort.data, onSelect: (level) => setPolishEffort.mutate(level) } : undefined}
            disabled={polishModel.isPending || setPolishModel.isPending || setPolishEffort.isPending}
            onChange={(v) => setPolishModel.mutate(v)}
          />
        </Row>
      </Section>
    </>
  )
}

/** Voice › History: the last dictations with their audio, and the latency of the last one. */
export function VoiceHistory() {
  const api = useApi()
  const history = useQuery({ queryKey: keys.history, queryFn: () => api.dictation_history(HISTORY) })
  const dictation = useQuery({ queryKey: keys.dictation, queryFn: () => api.dictation_status() })
  const forget = useAction((id: string | null) => api.dictation_history_forget(id), { keys: [keys.history] })

  const play = async (id: string) => {
    try {
      await playPcm(pcmSamples(await api.dictation_audio_get(id, null)))
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <>
      <Section
        title="History"
        actions={
          <Button variant="outline" size="sm" disabled={!history.data?.length || forget.isPending} onClick={() => forget.mutate(null)}>
            Forget all
          </Button>
        }
      >
        <p className="pb-2 text-[13px] text-fg-secondary">
          The last {HISTORY} dictations on this engine: what was heard, what was written, and the polished text when polish ran. Audio is kept for
          120 s per dictation within a 64 MiB budget; Forget removes the row and its audio.
        </p>
        {history.isError && <Empty>Could not read the history: {history.error.message}</Empty>}
        {history.isSuccess && history.data.length === 0 && <Empty>Nothing dictated yet.</Empty>}
        {history.data?.map((row) => (
          <HistoryLine key={row.id} row={row} onPlay={() => void play(row.id)} onForget={() => forget.mutate(row.id)} />
        ))}
      </Section>
      <Section title="Debug">
        <Row label="Last dictation" description={timingsLine(dictation.data?.last)} />
      </Section>
    </>
  )
}

/** One phrase editor: commits on blur or Enter. */
function PhraseRow({ label, description, value, commit }: { label: string; description: string; value: string; commit: (raw: string) => void }) {
  const [draft, setDraft] = useState(value)
  return (
    <Row label={label} description={description}>
      <Input
        dictate={false}
        aria-label={`${label} phrases`}
        className="w-56"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => draft !== value && commit(draft)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        }}
      />
    </Row>
  )
}

/** when · surface · raw → shaped (+ polished) · play · Forget */
function HistoryLine({ row, onPlay, onForget }: { row: HistoryRow; onPlay: () => void; onForget: () => void }) {
  const surface = SURFACES.find(([s]) => s === row.surface)?.[1] ?? row.surface
  return (
    <div className="flex items-start gap-3 border-b border-border-subtle py-3 text-sm last:border-0">
      <div className="min-w-0 flex-1">
        <div className="text-xs text-fg-tertiary">
          {ago(Number(row.started_ms))} · {surface}
          {row.reason.startsWith('error') ? ` · ${row.reason}` : ''}
        </div>
        <div className="mt-0.5 break-words">
          {row.raw !== row.shaped && <span className="text-fg-tertiary line-through">{row.raw} </span>}
          {row.raw !== row.shaped && '→ '}
          {row.shaped || <span className="text-fg-tertiary">(nothing)</span>}
        </div>
        {row.polished !== null && <div className="mt-0.5 break-words text-fg-secondary">polished: {row.polished}</div>}
      </div>
      {row.has_audio && (
        <Button variant="ghost" size="icon-xs" aria-label="Play" onClick={onPlay}>
          <Play className="size-3.5" />
        </Button>
      )}
      <Button variant="ghost" size="sm" onClick={onForget}>
        Forget
      </Button>
    </div>
  )
}
