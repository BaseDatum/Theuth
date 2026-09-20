// 53 §4.1 / 03 §3: the start form — process picker (active first, last bundle id remembered), title, a
// dictated context box, the consent checkbox (decision 14), the session's model, the permissions row.
// `Start` → `meeting_start` → the control center.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Check, LoaderCircle, Mic, RefreshCw, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { LevelMeter } from '@/components/chat/LevelMeter'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import * as dictation from '@/lib/dictation'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { PermissionState } from '@/rpc/bindings/PermissionState'
import { useConnection } from '@/store/connection'

const LAST_PROCESS = 'meeting-last-process'
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
  const [pid, setPid] = useState<number | null>(null)
  const [title, setTitle] = useState('')
  const [context, setContext] = useState('')
  const [consent, setConsent] = useState(false)
  const [model, setModel] = useState<string | null>(null)

  // First list: the remembered bundle id if it is running, else the first active process.
  useEffect(() => {
    const list = processes.data
    if (!list || pid !== null) return
    const last = localStorage.getItem(LAST_PROCESS)
    const pick = list.find((p) => p.bundle_id !== null && p.bundle_id === last) ?? list.find((p) => p.active) ?? list[0]
    if (pick) setPid(pick.pid)
  }, [processes.data, pid])

  const start = useAction(
    (_: void) =>
      api.meeting_start({
        process: pid,
        title: title.trim() || null,
        context: context.trim() || null,
        consent,
        session_model: model,
      }),
    {
      keys: [queryKeys.meetings, queryKeys.meetingStatus],
      onSuccess: (started) => {
        const p = processes.data?.find((x) => x.pid === pid)
        if (p?.bundle_id) localStorage.setItem(LAST_PROCESS, p.bundle_id)
        void navigate({ to: '/meetings/$id', params: { id: started.meeting } })
      },
    },
  )

  const disabled = processes.isError
  return (
    <div className="mx-auto w-full max-w-2xl px-6 pb-10">
      <Section title="Start meeting">
        {disabled && <Empty>{errorText(processes.error)}</Empty>}
        <Row
          label="Meeting app"
          description="The process whose audio is recorded (Core Audio tap); apps playing audio first."
        >
          <Select value={pid === null ? undefined : String(pid)} disabled={disabled} onValueChange={(v) => setPid(Number(v))}>
            <SelectTrigger className="w-64">
              <SelectValue placeholder="Choose an app" />
            </SelectTrigger>
            <SelectContent>
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
          <ContextBox value={context} onChange={setContext} />
        </div>
        <Row
          label="Session model"
          description="The meeting's own chat session runs on it; a fast, inexpensive model is the sensible choice here."
        >
          <Select value={model ?? info?.default_model} onValueChange={setModel}>
            <SelectTrigger className="w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {models.data?.map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {m.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
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
          <Button disabled={!consent || pid === null || disabled || start.isPending} onClick={() => start.mutate()}>
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

/**
 * Context textarea with dictation (54): the store is global with one `active`, so this box takes the
 * `owner` flag exactly as `Composer.tsx` does and splices the finished text at its caret.
 */
function ContextBox({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const api = useApi()
  const ref = useRef<HTMLTextAreaElement>(null)
  const dict = dictation.useDictation()
  const [owner, setOwner] = useState(false)
  const ownerRef = useRef(owner)
  ownerRef.current = owner
  const dictating = owner && dict.active !== null
  const reason = dictation.reason(dict) ?? (dict.active !== null && !owner ? 'Dictating in another box' : null)
  useEffect(
    () =>
      dictation.onFinish(({ value }) => {
        if (ownerRef.current) onChange(value)
      }),
    [onChange],
  )
  useEffect(() => {
    if (dict.phase === 'idle') setOwner(false)
  }, [dict.phase])
  const toggle = () => {
    if (dictating) {
      if (dict.phase === 'dictating') void dictation.stop(api)
      return
    }
    if (reason) {
      toast(reason)
      return
    }
    if (dict.phase !== 'idle') return
    const at = ref.current?.selectionStart ?? value.length
    setOwner(true)
    void dictation.start(api, value.slice(0, at), value.slice(at), false)
  }
  const shown = dictating && dict.active ? dictation.splice(dict.active.before, dict.active.after, dict.active.utts).value : value
  return (
    <div className="relative mt-2">
      <Textarea
        ref={ref}
        rows={3}
        className="pr-12"
        value={shown}
        readOnly={dictating}
        onChange={(e) => onChange(e.target.value)}
      />
      <Button
        variant="ghost"
        className={cn('absolute right-1.5 bottom-1.5 rounded-full', dictating ? 'h-8 w-20 bg-fg-accent/10 p-0' : 'size-8', reason && !dictating && 'opacity-50')}
        aria-label={dictating ? 'Stop dictation' : 'Dictate'}
        aria-disabled={reason !== null && !dictating}
        title={reason ?? undefined}
        onClick={toggle}
      >
        {dictating && dict.active ? <LevelMeter levels={dict.active.levels} active /> : <Mic className="size-4" />}
      </Button>
    </div>
  )
}
