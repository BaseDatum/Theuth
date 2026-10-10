// 53 decisions 9–10 / 03 §3: Settings › Meetings — the eye (hidden from capture) with its hotkey, the
// push-to-talk hotkey, and the black-screenshot note. All desktop-local (`lib/meeting-settings.ts`) — except
// the live speaker labels switch (07 §6, `meeting.live_diarization` in the daemon's config) and the enrolled
// speaker profiles (04 §7), which are the daemon's. The decision-oracle switch moved to Labs (57 §7.1).
// 158 §6.3 (W1): the engine's one-tap defaults lead — Defaults (models, template, language, project),
// Detection (switch + ignore list), Live tools — bound whole to `meeting_settings_get/set` (§6.2).

import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { SAME_AS_NEW } from '@/components/settings/Chat'
import { MeetingTemplates } from '@/components/settings/MeetingTemplates'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { useProjects } from '@/components/chat/ProjectPicker'
import { ChordInput, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { DEFAULT_EYE, DEFAULT_PTT, useMeetingSettings } from '@/lib/meeting-settings'
import { offerMeetingDefault } from '@/lib/meeting-model-offer'
import { useProjectStyles } from '@/lib/project-style'
import { queryKeys, useApi } from '@/lib/rpc'
import { probeShortcut } from '@/lib/shortcuts'
import { setHiddenFromCapture } from '@/lib/tauri'
import type { MeetingSettings } from '@/rpc/bindings/MeetingSettings'

export const SETTINGS_KEY = ['meeting-settings'] as const
const TEMPLATES_KEY = queryKeys.meetingTemplates
/** `MeetingConfig::default().live_tools` (`crates/meeting/src/lib.rs`); "Reset to default" writes this list. */
export const DEFAULT_LIVE_TOOLS = [
  'meeting_topic',
  'meeting_brief',
  'brief_search',
  'brief_put',
  'knowledge_search',
  'knowledge_get',
  'meeting_read',
  'meeting_search',
  'search_web',
  'read_url',
]
const SAME_AS_MEETING = { label: 'Same as meeting model' }
// Radix Select refuses '' as a value: the project rows are `p:<label>`, "none" is its own value.
const NONE = 'none'

/** Append one trimmed, deduplicated entry; unchanged for blanks and repeats. */
export function added(list: string[], entry: string): string[] {
  const v = entry.trim()
  return v && !list.includes(v) ? [...list, v] : list
}

export function Meetings() {
  const api = useApi()
  const hidden = useMeetingSettings((s) => s.hidden)
  const ptt = useMeetingSettings((s) => s.ptt)
  const eye = useMeetingSettings((s) => s.eye)
  const local = isMacDesktop()
  const speakers = useQuery({ queryKey: queryKeys.meetingSpeakers, queryFn: () => api.meeting_speakers() })
  const del = useAction((profile: string) => api.meeting_speaker_delete(profile), { keys: [queryKeys.meetingSpeakers] })
  const liveDiar = useQuery({ queryKey: queryKeys.meetingLiveDiarization, queryFn: () => api.meeting_live_diarization() })
  // The block carries `live_diarization` too, so the switch refreshes the block and the block refreshes the switch.
  const setLiveDiar = useAction((on: boolean) => api.set_meeting_live_diarization(on), { keys: [queryKeys.meetingLiveDiarization, SETTINGS_KEY] })
  const settings = useQuery({ queryKey: SETTINGS_KEY, queryFn: () => api.meeting_settings_get() })
  const templates = useQuery({ queryKey: TEMPLATES_KEY, queryFn: () => api.meeting_templates() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const projects = [...new Set([...useProjects(), ...Object.keys(useProjectStyles())])].sort()
  const save = useAction((next: MeetingSettings) => api.meeting_settings_set(next), { keys: [SETTINGS_KEY, queryKeys.meetingLiveDiarization] })
  const s = settings.data
  const busy = !s || save.isPending
  const set = (patch: Partial<MeetingSettings>) => s && save.mutate({ ...s, ...patch })
  // §6.4: once per install, offer the start form's remembered model as `meeting.model` when the engine has none.
  useEffect(() => {
    if (!s || !models.data) return
    const saved = useMeetingSettings.getState().model
    offerMeetingDefault({ block: s, saved, label: models.data.find((m) => m.id === saved)?.name, write: (next) => save.mutate(next) })
  }, [s, models.data, save.mutate])
  // W4 fills `meeting_templates`; until then the one slug on file reads "General".
  const templateRows = templates.data?.length ? templates.data : [{ slug: s?.template ?? 'meeting', name: 'General' }]
  return (
    <>
      <Section title="Defaults">
        {settings.isError && <Empty>{errorText(settings.error)}</Empty>}
        <Row stack label="Meeting model" description="The meeting's own session: topics and briefs while it runs, the pre-meeting brief, speaker-name suggestions.">
          <ModelCombobox
            aria-label="Meeting model"
            className="max-w-64 max-md:w-full max-md:max-w-none"
            models={models.data ?? []}
            value={s?.model}
            none={SAME_AS_NEW}
            disabled={busy}
            onChange={(model) => set({ model })}
          />
        </Row>
        <Row stack label="Summary model" description="The post-meeting summary turn only.">
          <ModelCombobox
            aria-label="Summary model"
            className="max-w-64 max-md:w-full max-md:max-w-none"
            models={models.data ?? []}
            value={s?.summary_model}
            none={SAME_AS_MEETING}
            disabled={busy}
            onChange={(summary_model) => set({ summary_model })}
          />
        </Row>
        <Row label="Template" description="The note template a meeting gets unless the start form, the prompt or the event picks another.">
          <Select value={s?.template} disabled={busy} onValueChange={(template) => set({ template })}>
            <SelectTrigger className="w-64" aria-label="Template">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {templateRows.map((t) => (
                <SelectItem key={t.slug} value={t.slug}>
                  {t.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Transcript language" description="Stored for the post-meeting pass. Live transcription is English only until the multilingual tier.">
          <Select value={s?.language ?? 'en'} disabled={busy} onValueChange={(language) => set({ language })}>
            <SelectTrigger className="w-64" aria-label="Transcript language">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="en">English</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row label="Default project" description="The project a meeting gets when nothing is remembered for its calendar series.">
          <Select value={s?.project ? `p:${s.project}` : NONE} disabled={busy} onValueChange={(v) => set({ project: v.startsWith('p:') ? v.slice(2) : null })}>
            <SelectTrigger className="w-64" aria-label="Default project">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>None</SelectItem>
              {projects.map((p) => (
                <SelectItem key={p} value={`p:${p}`}>
                  <ProjectIcon project={p} className="size-3.5" />
                  {p}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
      </Section>
      <Section title="Detection">
        <Row label="Detect meetings from the microphone" description="Ask when another app has held the microphone for 15 seconds. Nothing is recorded until you start.">
          <Switch aria-label="Detect meetings" checked={s?.detect.enabled ?? true} disabled={busy} onCheckedChange={(enabled) => s && set({ detect: { ...s.detect, enabled } })} />
        </Row>
        <Row stack label="Ignored apps" description="Apps on this list never prompt. theuth's own dictation is always ignored. Bundle id on macOS (com.apple.FaceTime), image name on Windows (obs64.exe), application name on Linux.">
          <Chips
            label="Ignored app"
            items={s?.detect.ignore ?? []}
            disabled={busy}
            onChange={(ignore) => s && set({ detect: { ...s.detect, ignore } })}
          />
        </Row>
      </Section>
      <Section title="Live tools" actions={<Button variant="outline" size="sm" disabled={busy || JSON.stringify(s?.live_tools) === JSON.stringify(DEFAULT_LIVE_TOOLS)} onClick={() => set({ live_tools: DEFAULT_LIVE_TOOLS })}>Reset to default</Button>}>
        <Row stack label="Allowed while a meeting runs" description="The meeting session's toolset: topics, briefs, knowledge and the web by default. Tool names as the Tools page lists them.">
          <Chips label="Live tool" items={s?.live_tools ?? []} disabled={busy} onChange={(live_tools) => set({ live_tools })} />
        </Row>
      </Section>
      <Section title="Screen capture">
        <Row
          label="Hidden from capture"
          description="Both windows are excluded from screen sharing and screenshots (NSWindow sharingType none). Never automatic: theuth does not detect screen sharing. Your own ⌘⇧3/4 screenshots of the windows come out black while this is on."
        >
          <Switch checked={hidden} disabled={!local} onCheckedChange={(on) => void setHiddenFromCapture(on)} />
        </Row>
        <Row label="Eye hotkey" description="Flips hidden from capture while a meeting records. Click the field and press a chord.">
          <Hotkey kind="eye" value={eye} fallback={DEFAULT_EYE} />
        </Row>
      </Section>
      <Section title="Push-to-talk">
        <Row
          label="Push-to-talk hotkey"
          description="Hold to ask the meeting session a question; what you say while it is held is tagged as a request and answered as a brief. Registered only while a meeting records or is paused."
        >
          <Hotkey kind="ptt" value={ptt} fallback={DEFAULT_PTT} />
        </Row>
      </Section>
      <Section title="Live transcript">
        <Row
          label="Live speaker labels"
          description="Label the other side's lines by voice (Speaker 1, Speaker 2…) while the meeting runs. Labels arrive about eight seconds after the words and cost roughly a quarter of a CPU core. Off: lines read `them:` until the post-meeting pass labels speakers. Applies from the next meeting."
        >
          <Switch
            checked={liveDiar.data ?? true}
            disabled={liveDiar.isLoading || liveDiar.isError || setLiveDiar.isPending}
            onCheckedChange={(on) => setLiveDiar.mutate(on)}
          />
        </Row>
        {liveDiar.isError && <Empty>{errorText(liveDiar.error)}</Empty>}
      </Section>
      <Section title="Local decision oracle">
        <OraclePointer />
      </Section>
      <Section title="Enrolled speakers">
        {speakers.isError && <Empty>{errorText(speakers.error)}</Empty>}
        {speakers.data?.length === 0 && <Empty>No enrolled speakers yet. Enroll one from a meeting's review pane.</Empty>}
        {speakers.data?.map((p) => (
          <Row key={p.id} label={p.name} description={`${p.entity ?? '—'} · ${p.samples} sample${p.samples === 1 ? '' : 's'}`}>
            <Button variant="outline" size="sm" disabled={del.isPending} onClick={() => del.mutate(p.id)}>
              Delete
            </Button>
          </Row>
        ))}
      </Section>
      <MeetingTemplates />
    </>
  )
}

/** A removable-chip list with an add field (Enter or the button adds; blanks and repeats are ignored). */
function Chips({ label, items, disabled, onChange }: { label: string; items: string[]; disabled?: boolean; onChange(next: string[]): void }) {
  const [draft, setDraft] = useState('')
  const add = () => {
    const next = added(items, draft)
    if (next !== items) onChange(next)
    setDraft('')
  }
  return (
    <div className="flex max-w-xl flex-col gap-2 max-md:w-full">
      <div className="flex flex-wrap gap-1.5">
        {items.length === 0 && <span className="text-xs text-fg-tertiary">None</span>}
        {items.map((v) => (
          <Badge key={v} variant="outline" className="gap-1 pr-1 font-mono">
            {v}
            <button type="button" aria-label={`Remove ${v}`} disabled={disabled} className="rounded-full hover:bg-accent" onClick={() => onChange(items.filter((x) => x !== v))}>
              <X className="size-3" />
            </button>
          </Badge>
        ))}
      </div>
      <div className="flex gap-2">
        <Input
          className="h-8 w-64"
          aria-label={`Add ${label.toLowerCase()}`}
          placeholder={label}
          value={draft}
          disabled={disabled}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') add()
          }}
        />
        <Button variant="outline" size="sm" disabled={disabled || !draft.trim()} onClick={add}>
          Add
        </Button>
      </div>
    </div>
  )
}

/** 57 §7.1: the switch lives in Labs; this line shows the effective state and links there (same query key, so both update together). */
function OraclePointer() {
  const api = useApi()
  const info = useQuery({ queryKey: queryKeys.decisionMode('meeting'), queryFn: () => api.decision_mode('meeting') })
  const state = info.isError ? errorText(info.error) : info.data ? info.data.effective : '…'
  return (
    <Row label={`Local decision oracle: ${state}`} description="Off · Shadow · Active, counters and training-data consent are in Labs.">
      <Button asChild variant="outline" size="sm">
        <Link to="/settings/$tab" params={{ tab: 'labs' }} search={{}}>
          Change in Labs
        </Link>
      </Button>
    </Row>
  )
}

/** A meeting global shortcut: probed with the plugin (a conflict rejects) before it is saved. */
function Hotkey({ kind, value, fallback }: { kind: 'ptt' | 'eye'; value: string; fallback: string }) {
  const setHotkey = useMeetingSettings((s) => s.setHotkey)
  return (
    <ChordInput
      value={value}
      fallback={fallback}
      commit={(chord) => (chord === fallback ? setHotkey(kind, chord) : probeShortcut(chord).then(() => setHotkey(kind, chord)))}
    />
  )
}
