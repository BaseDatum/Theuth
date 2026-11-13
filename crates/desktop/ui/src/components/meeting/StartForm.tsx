// 53 §4.1 / 03 §3: the start form — source picker (System audio by default, or one app; apps playing
// audio first), title, a dictated context box, the one lawful sentence (158 R3, Amendment A2: no checkbox;
// Start sends `consent: true`), the project (158 §9.2: a combobox; "Default" leaves the engine's remembered /
// `meeting.project` choice), the session's model (the standard `ModelCombobox`; remembered in `meeting-settings`),
// the permissions row. `Start` → `meeting_start` → the control center.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Check, LoaderCircle, RefreshCw, X } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { ProjectChip } from '@/components/meeting/ProjectChip'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { LAWFUL } from '@/lib/detection'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PermissionState } from "@/rpc/bindings/method.meeting.output"
import type { StartMeeting } from '@/rpc/bindings/method.meeting.input'

/** The picker's "System audio" row; `meeting_start` gets `process: null`. */
const SYSTEM = 0
const PRIVACY = 'x-apple.systempreferences:com.apple.preference.security'

/** What `Start` sends (137 R1-D6 adds `memory`: `'endless'` with the ∞ switch on, else `null` = the engine's `default_meeting`; 158 R3: `consent` is always `true` — the sentence was shown; 158 §9.2: `project` only when picked — absent = remembered or `meeting.project`; 158 §11.1: `template` only when picked, else the engine resolves remembered → `meeting.template`; 194: `session_effort` rides with `session_model`, absent = Default). */
export function startOptions(form: { pid: number; title: string; context: string; model: string | null; effort?: string | null; endless: boolean; project: string | null; template?: string | null }): StartMeeting {
  return {
    process: form.pid === SYSTEM ? null : form.pid,
    title: form.title.trim() || null,
    context: form.context.trim() || null,
    consent: true,
    session_model: form.model,
    session_effort: (form.model && form.effort) || undefined,
    memory: form.endless ? 'endless' : undefined,
    project: form.project ?? undefined,
    template: form.template ?? undefined,
  }
}

// Radix Select refuses '' as a value: "let the engine decide" is its own row.
const AUTO = 'auto'
/** 194: the model picker's `none` row — no override, the engine's `meeting.model` at `meeting.effort`. */
const MEETING_DEFAULT = { label: 'Meeting default' }

export function StartForm() {
  const api = useApi()
  const navigate = useNavigate()
  const processes = useQuery({ queryKey: queryKeys.meetingProcesses, queryFn: () => api.meeting_processes() })
  const perms = useQuery({ queryKey: queryKeys.meetingPermissions, queryFn: () => api.meeting_permissions() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const [pid, setPid] = useState(SYSTEM)
  const [title, setTitle] = useState('')
  const [context, setContext] = useState('')
  const [project, setProject] = useState<string | null>(null)
  // 194: the start's override (`null` = Meeting default); the engine's fallback is never shown as a pick.
  const saved = useMeetingSettings((s) => s.model)
  const savedEffort = useMeetingSettings((s) => s.effort)
  const current = saved ? models.data?.find((m) => m.id === saved) : undefined

  // 137 R1-D6: the ∞ switch starts at `agent.endless.default_meeting`, remembered for this form only.
  const endlessSettings = useQuery({ queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const [endless, setEndless] = useState<boolean | null>(null)
  const endlessOn = endless ?? endlessSettings.data?.default_meeting ?? false

  // 158 §11.1: the note template; `null` = not picked here (remembered per event, else `meeting.template`).
  const templates = useQuery({ queryKey: queryKeys.meetingTemplates, queryFn: () => api.meeting_templates() })
  const [template, setTemplate] = useState<string | null>(null)

  const start = useAction(
    (_: void) => api.meeting_start(startOptions({ pid, title, context, model: saved, effort: savedEffort, endless: endlessOn, project, template })),
    {
      keys: [queryKeys.meetings, queryKeys.meetingStatus],
      onSuccess: (started) => void navigate({ to: '/meetings/$id', params: { id: started.meeting } }),
    },
  )

  const disabled = processes.isError
  return (
    <div className="mx-auto w-full max-w-2xl px-6 pb-10">
      <Section title="Start meeting">
        {processes.isError && <Empty>{errorText(processes.error)}</Empty>}
        <Row
          label="Source"
          description="System audio records every app on the default output; pick one app to record only it (Core Audio tap)."
        >
          <Select value={String(pid)} onValueChange={(v) => setPid(Number(v))}>
            <SelectTrigger className="w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={String(SYSTEM)}>System audio</SelectItem>
              {processes.data?.map((p) => (
                <SelectItem key={p.pid} value={String(p.pid)}>
                  {p.name}
                  {p.active ? ' · playing' : ''}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button variant="ghost" size="icon-sm" aria-label="Refresh" disabled={processes.isFetching} onClick={() => void processes.refetch()}>
            <RefreshCw className={processes.isFetching ? 'animate-spin' : undefined} />
          </Button>
        </Row>
        <Row label="Title" description="Optional; the summary sets one later.">
          <Input className="w-64" value={title} onChange={(e) => setTitle(e.target.value)} />
        </Row>
        <div className="border-b border-border-subtle py-4">
          <div className="text-sm">Context</div>
          <div className="mt-0.5 text-[13px] text-fg-secondary">
            What the meeting is about, who is in it — the first line the session reads. Type or dictate.
          </div>
          <Textarea rows={3} className="mt-2" value={context} onChange={(e) => setContext(e.target.value)} />
        </div>
        <Row label="Project" description="Groups the meeting and its chat with the project's sessions. Default = the project remembered for this calendar series, else Settings › Meetings.">
          <ProjectChip value={project} onChange={setProject} empty="Default" className="w-64 justify-between" />
        </Row>
        <Row
          label="Session model"
          description="The meeting's own chat session runs on it; a fast, inexpensive model is the sensible choice here. Meeting default: the model in Settings › Meetings."
        >
          <ModelCombobox
            aria-label="Session model"
            className="w-64"
            models={models.data ?? []}
            value={saved}
            none={MEETING_DEFAULT}
            effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: savedEffort, onSelect: (level) => useMeetingSettings.getState().setEffort(level) } : undefined}
            onChange={(id) => useMeetingSettings.getState().setModel(id, id && models.data?.find((m) => m.id === id)?.efforts.includes(savedEffort ?? '') ? savedEffort : null)}
          />
        </Row>
        <Row label="Note template" description="The headings the summary is written under. Automatic: the one remembered for this calendar event, else the default from Settings › Meetings.">
          <Select value={template ?? AUTO} onValueChange={(v) => setTemplate(v === AUTO ? null : v)}>
            <SelectTrigger className="w-64" aria-label="Note template">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={AUTO}>Automatic</SelectItem>
              {templates.data?.map((t) => (
                <SelectItem key={t.slug} value={t.slug}>
                  {t.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label={
            <span className="flex items-center gap-2">
              <span aria-hidden className="text-base leading-5 [font-feature-settings:normal]">
                ∞
              </span>
              Endless memory
            </span>
          }
          description="Keeps a compressed memory of the whole meeting behind the live transcript; zoom and recall bring any earlier moment back."
        >
          <Switch aria-label="Endless memory" checked={endlessOn} onCheckedChange={setEndless} />
        </Row>
        <Permission label="Microphone" state={perms.data?.microphone} pane="Privacy_Microphone" />
        <Permission label="System audio recording" state={perms.data?.audio_capture} pane="Privacy_AudioCapture" />
        <p className="py-4 text-sm text-fg-secondary">{LAWFUL}</p>
        <div className="flex justify-end">
          <Button disabled={disabled || start.isPending} onClick={() => start.mutate()}>
            {start.isPending && <LoaderCircle className="animate-spin" />} Start
          </Button>
        </div>
      </Section>
    </div>
  )
}

/** The Computer pane's opener; `NotDetermined` for audio capture until a first start (01 §0.6). */
function Permission({ label, state, pane }: { label: string; state: PermissionState | undefined; pane: string }) {
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          {state === 'Granted' ? <Check className="size-4 text-fg-secondary" /> : <X className="size-4 text-fg-secondary" />}
          {label}
          {state === 'NotDetermined' && <span className="text-[12px] text-fg-tertiary">asked on first start</span>}
        </span>
      }
    >
      {isMacDesktop() && state === 'Denied' && (
        <Button variant="outline" size="sm" onClick={() => openUrl(`${PRIVACY}?${pane}`).catch((e) => toast.error(errorText(e)))}>
          Open System Settings
        </Button>
      )}
    </Row>
  )
}
