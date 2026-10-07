// 53 §4.1 / 03 §3: the start form — source picker (System audio by default, or one app; apps playing
// audio first), title, a dictated context box, the consent checkbox (decision 14), the session's
// model (the chat header's picker; remembered in `meeting-settings`), the permissions row.
// `Start` → `meeting_start` → the control center.

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
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { isMacDesktop } from '@/lib/hotkeys'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PermissionState } from '@/rpc/bindings/PermissionState'
import { useConnection } from '@/store/connection'

/** The picker's "System audio" row; `meeting_start` gets `process: null`. */
const SYSTEM = 0
const PRIVACY = 'x-apple.systempreferences:com.apple.preference.security'
/** 53 decision 14, verbatim. */
export const CONSENT = 'I acknowledge I understand the law and this recording is legal.'

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
  const [consent, setConsent] = useState(false)
  const saved = useMeetingSettings((s) => s.model)
  const model = saved ?? info?.default_model
  const [pickModel, setPickModel] = useState(false)
  const current = models.data?.find((m) => m.id === model)
  const { favorites, toggleFavorite } = useFavorites()

  const start = useAction(
    (_: void) =>
      api.meeting_start({
        process: pid === SYSTEM ? null : pid,
        title: title.trim() || null,
        context: context.trim() || null,
        consent,
        session_model: saved,
      }),
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
        <Permission label="Microphone" state={perms.data?.microphone} pane="Privacy_Microphone" />
        <Permission label="System audio recording" state={perms.data?.audio_capture} pane="Privacy_AudioCapture" />
        <div className="py-4">
          <Label className="items-start gap-3 leading-5">
            <input
              type="checkbox"
              className="mt-0.5 size-4 shrink-0 accent-fg-accent"
              checked={consent}
              onChange={(e) => setConsent(e.target.checked)}
            />
            <span className="font-normal">{CONSENT}</span>
          </Label>
        </div>
        <div className="flex justify-end">
          <Button disabled={!consent || disabled || start.isPending} onClick={() => start.mutate()}>
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
