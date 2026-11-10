// 53 decisions 9–10 / 03 §3, 158 §6.3: Settings › Meetings, one component per tab. Defaults, Detection and Live edit
// the engine's `meeting` block whole (`meeting-queries.ts`, §6.2). Live adds the eye (hidden from capture, desktop-local
// in `lib/meeting-settings.ts`), the live speaker labels switch (07 §6, `meeting.live_diarization`) and, outside macOS,
// the 158 §13 (W6) Echo row; Speakers lists the daemon's enrolled profiles (04 §7). 188: the eye and push-to-talk
// hotkeys moved to General › Shortcuts, the decision-oracle pointer went (Learning › Decision models holds it).

import { useQuery } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { SAME_AS_NEW } from '@/components/settings/ModelDefaults'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { useProjects } from '@/components/chat/ProjectPicker'
import { SETTINGS_KEY, useMeetingBlock } from '@/components/settings/meeting-queries'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { offerMeetingDefault } from '@/lib/meeting-model-offer'
import { useProjectStyles } from '@/lib/project-style'
import { queryKeys, useApi } from '@/lib/rpc'
import { setHiddenFromCapture } from '@/lib/tauri'
import type { Manifest } from '@/rpc/bindings/Manifest'

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

/** 158 §13: the echo ratio over the meetings that carry `stats`; the gate counts speaker meetings separately. */
export function echoSummary(manifests: Manifest[]): string {
  const rows = manifests.filter((m) => m.stats && m.stats.mic_segments > 0)
  if (rows.length === 0) return 'No meeting measured yet — the count starts with the next meeting\u2019s post pass.'
  const dupes = rows.reduce((n, m) => n + m.stats!.echo_dupes, 0)
  const mic = rows.reduce((n, m) => n + m.stats!.mic_segments, 0)
  const speakers = rows.filter((m) => m.stats!.speakers === true).length
  return `${Math.round((100 * dupes) / mic)} % of your microphone segments repeated the other side (${dupes} of ${mic}) over the last ${rows.length} meeting${rows.length === 1 ? '' : 's'}, ${speakers} on speakers.`
}

/** Meetings › Defaults: the models, template, language and project a meeting starts with. */
export function MeetingDefaults() {
  const api = useApi()
  const { settings, block: s, save, set, busy } = useMeetingBlock()
  const templates = useQuery({ queryKey: queryKeys.meetingTemplates, queryFn: () => api.meeting_templates() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const projects = [...new Set([...useProjects(), ...Object.keys(useProjectStyles())])].sort()
  // §6.4: once per install, offer the start form's remembered model as `meeting.model` when the engine has none.
  useEffect(() => {
    if (!s || !models.data) return
    const saved = useMeetingSettings.getState().model
    offerMeetingDefault({ block: s, saved, label: models.data.find((m) => m.id === saved)?.name, write: (next) => save.mutate(next) })
  }, [s, models.data, save.mutate])
  // W4 fills `meeting_templates`; until then the one slug on file reads "General".
  const templateRows = templates.data?.length ? templates.data : [{ slug: s?.template ?? 'meeting', name: 'General' }]
  return (
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
  )
}

/** Meetings › Detection: whether another app holding the microphone prompts, and which apps never do. */
export function MeetingDetection() {
  const { settings, block: s, set, busy } = useMeetingBlock()
  return (
    <Section title="Detection">
      {settings.isError && <Empty>{errorText(settings.error)}</Empty>}
      <Row label="Detect meetings from the microphone" description="Ask when another app has held the microphone for 15 seconds. Nothing is recorded until you start.">
        <Switch aria-label="Detect meetings" checked={s?.detect.enabled ?? true} disabled={busy} onCheckedChange={(enabled) => s && set({ detect: { ...s.detect, enabled } })} />
      </Row>
      <Row wide label="Ignored apps" description="Apps on this list never prompt. theuth's own dictation is always ignored. Bundle id on macOS (com.apple.FaceTime), image name on Windows (obs64.exe), application name on Linux.">
        <Chips
          label="Ignored app"
          items={s?.detect.ignore ?? []}
          disabled={busy}
          onChange={(ignore) => s && set({ detect: { ...s.detect, ignore } })}
        />
      </Row>
    </Section>
  )
}

/** Meetings › Live: what runs while a meeting records — echo count, live tools, hidden from capture, speaker labels. */
export function MeetingLive() {
  const api = useApi()
  const { settings, block: s, set, busy } = useMeetingBlock()
  const hidden = useMeetingSettings((st) => st.hidden)
  const local = isMacDesktop()
  const liveDiar = useQuery({ queryKey: queryKeys.meetingLiveDiarization, queryFn: () => api.meeting_live_diarization() })
  // The block carries `live_diarization` too, so the switch refreshes the block and the block refreshes the switch.
  const setLiveDiar = useAction((on: boolean) => api.set_meeting_live_diarization(on), { keys: [queryKeys.meetingLiveDiarization, SETTINGS_KEY] })
  // 158 §13: the last 10 manifests, read only where the mic has no echo cancellation (Windows/Linux).
  const echo = useQuery({
    queryKey: [...queryKeys.meetings, 'echo'],
    queryFn: async () => Promise.all((await api.meeting_list(10)).map((m) => api.meeting_get(m.id))),
    enabled: !local,
  })
  return (
    <>
      {!local && (
        <Section title="Echo">
          <Row wide label="Echo in your microphone" description="Without echo cancellation the other side can leak back in through your microphone and be transcribed twice. This counts it; nothing is removed. Review asks per meeting whether you used speakers.">
            <span className="text-[13px] text-fg-secondary">{echo.isError ? errorText(echo.error) : echo.data ? echoSummary(echo.data) : null}</span>
          </Row>
        </Section>
      )}
      <Section title="Live tools" actions={<Button variant="outline" size="sm" disabled={busy || JSON.stringify(s?.live_tools) === JSON.stringify(DEFAULT_LIVE_TOOLS)} onClick={() => set({ live_tools: DEFAULT_LIVE_TOOLS })}>Reset to default</Button>}>
        {settings.isError && <Empty>{errorText(settings.error)}</Empty>}
        <Row wide label="Allowed while a meeting runs" description="The meeting session's toolset: topics, briefs, knowledge and the web by default. Tool names as the Tools page lists them.">
          <Chips label="Live tool" items={s?.live_tools ?? []} disabled={busy} onChange={(live_tools) => set({ live_tools })} />
        </Row>
      </Section>
      <Section title="Screen capture">
        <Row
          label="Hidden from capture"
          description="Both windows are excluded from screen sharing and screenshots (NSWindow sharingType none). Never automatic: theuth does not detect screen sharing. Your own ⌘⇧3/4 screenshots of the windows come out black while this is on. The eye hotkey that flips it is in General › Shortcuts."
        >
          <Switch checked={hidden} disabled={!local} onCheckedChange={(on) => void setHiddenFromCapture(on)} />
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
    </>
  )
}

/** Meetings › Speakers: the voice profiles enrolled from meeting reviews. */
export function MeetingSpeakers() {
  const api = useApi()
  const speakers = useQuery({ queryKey: queryKeys.meetingSpeakers, queryFn: () => api.meeting_speakers() })
  const del = useAction((profile: string) => api.meeting_speaker_delete(profile), { keys: [queryKeys.meetingSpeakers] })
  return (
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
