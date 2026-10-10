// 53 §4.1 / 03 §3: the start form — source picker (System audio by default, or one app; apps playing
// audio first), title, a dictated context box, the one lawful sentence (158 R3, Amendment A2: no checkbox;
// Start sends `consent: true`), the session's model (the chat header's picker; remembered in
// `meeting-settings`), the permissions row. `Start` → `meeting_start` → the control center.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Check, ChevronDown, LoaderCircle, RefreshCw, X } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { useFavorites } from '@/components/chat/ModelMenu'
import { ModelPicker } from '@/components/chat/ModelPicker'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { LAWFUL } from '@/lib/detection'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PermissionState } from '@/rpc/bindings/PermissionState'
import { useConnection } from '@/store/connection'
import type { StartMeeting } from '@/rpc/bindings/StartMeeting'

/** The picker's "System audio" row; `meeting_start` gets `process: null`. */
const SYSTEM = 0
const PRIVACY = 'x-apple.systempreferences:com.apple.preference.security'

/** What `Start` sends (137 R1-D6 adds `memory`: `'endless'` with the ∞ switch on, else `null` = the engine's `default_meeting`; 158 R3: `consent` is always `true` — the sentence was shown; 158 §11.1: `template` only when picked, else the engine resolves remembered → `meeting.template`). */
export function startOptions(form: { pid: number; title: string; context: string; model: string | null; endless: boolean; template?: string | null }): StartMeeting {
  return {
    process: form.pid === SYSTEM ? null : form.pid,
    title: form.title.trim() || null,
    context: form.context.trim() || null,
    consent: true,
    session_model: form.model,
    memory: form.endless ? 'endless' : undefined,
    template: form.template ?? undefined,
  }
}

// Radix Select refuses '' as a value: "let the engine decide" is its own row.
const AUTO = 'auto'

export function StartForm() {
  const api = useApi()
  const navigate = useNavigate()
  const info = useConnection((s) => s.info)
  const processes = useQuery({ queryKey: queryKeys.meetingProcesses, queryFn: () => api.meeting_processes() })
  const perms = useQuery({ queryKey: queryKeys.meetingPermissions, queryFn: () => api.meeting_permissions() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const [pid, setPid] = useState(SYSTEM)
  const [title, setTitle] = useState('')
  const [context, setContext] = useState('')
  const saved = useMeetingSettings((s) => s.model)
  const model = saved ?? info?.default_model
  const [pickModel, setPickModel] = useState(false)
  const current = models.data?.find((m) => m.id === model)
  const { favorites, toggleFavorite } = useFavorites()

  // 137 R1-D6: the ∞ switch starts at `agent.endless.default_meeting`, remembered for this form only.
  const endlessSettings = useQuery({ queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const [endless, setEndless] = useState<boolean | null>(null)
  const endlessOn = endless ?? endlessSettings.data?.default_meeting ?? false

  // 158 §11.1: the note template; `null` = not picked here (remembered per event, else `meeting.template`).
  const templates = useQuery({ queryKey: queryKeys.meetingTemplates, queryFn: () => api.meeting_templates() })
  const [template, setTemplate] = useState<string | null>(null)

  const start = useAction(
    (_: void) => api.meeting_start(startOptions({ pid, title, context, model: saved, endless: endlessOn, template })),
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
        <Row
          label="Session model"
          description="The meeting's own chat session runs on it; a fast, inexpensive model is the sensible choice here."
        >
          <Popover open={pickModel} onOpenChange={setPickModel}>
            <PopoverTrigger asChild>
              <Button variant="outline" className="w-64 justify-between font-normal">
                <span className="truncate">{current?.name ?? model ?? 'Model'}</span>
                <ChevronDown className="size-4 opacity-70" />
              </Button>
            </PopoverTrigger>
            <PopoverContent align="start" className="w-80 p-0">
              <ModelPicker
                models={models.data ?? []}
                model={model}
                favorites={favorites}
                onToggleFavorite={(id) => void toggleFavorite(id)}
                listClassName="max-h-[min(24rem,calc(var(--visible-band)-6rem))]"
                onSelect={(id) => {
                  useMeetingSettings.getState().setModel(id)
                  setPickModel(false)
                }}
              />
            </PopoverContent>
          </Popover>
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
