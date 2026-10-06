import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useSearch } from '@tanstack/react-router'
import { ChevronRight, FlaskConical, MessageSquarePlus, Play } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useModels } from '@/components/agents/queries'
import { Field } from '@/components/agents/RuntimeAccessFields'
import { Divider } from '@/components/chat/Panes'
import { Pulse } from '@/components/chat/Pulse'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { NativeSelect } from '@/components/routines/NativeSelect'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { useAgents, useBoardProjects } from '@/components/tasks/queries'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useIsMobile } from '@/hooks/use-mobile'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { RunInfo } from '@/rpc/bindings/RunInfo'
import type { ScheduleFile } from '@/rpc/bindings/ScheduleFile'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SensorInfo } from '@/rpc/bindings/SensorInfo'
import type { Verdict } from '@/rpc/bindings/Verdict'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { RoutineChat } from './RoutineChat'
import { KindBadge, RESULT_CLASS, RoutineChips, sensorArgs } from './RoutinesView'
import { type Draft, NEW, draftToFile, fileText, latestDraft } from './draft'
import { type Kind, SECTIONS, type Section, cronWords, fromFile, fromInfo, kindOf, lastCheckText, parseWhen, plainError, relative, resultKind, resultLabel, runCount, sentence, took, verdictText } from './helpers'
import { KEY, useNotHub, useRoutineFile, useRoutineHistory, useRoutines, useRoutinesApi, useSensors } from './queries'

// Radix Select refuses '' as a value.
const NONE = '__none'

/** Seconds as a short humantime: whole hours → `2h`, whole minutes → `30m`, else `90s`; unknown/0 → `30m`. */
function shortDuration(secs: number | undefined): string {
  if (!secs) return '30m'
  if (secs % 3600 === 0) return `${secs / 3600}h`
  if (secs % 60 === 0) return `${secs / 60}m`
  return `${secs}s`
}

const UNIT_MS: Record<string, number> = { s: 1000, m: 60_000, h: 3_600_000, d: 86_400_000, w: 604_800_000 }

/** `14d` / `1h30m` → ms, `null` when a token is not humantime. */
function durationMs(s: string): number | null {
  let total = 0
  for (const m of s.trim().matchAll(/(\d+)\s*([smhdw])/g)) total += Number(m[1]) * (UNIT_MS[m[2] ?? ''] ?? 0)
  return total > 0 ? total : null
}

/** `12d 4h` / `3h 20m` / `5m` remaining. */
function leftText(ms: number): string {
  if (ms <= 0) return 'passed'
  const d = Math.floor(ms / 86_400_000)
  const h = Math.floor((ms % 86_400_000) / 3_600_000)
  const m = Math.floor((ms % 3_600_000) / 60_000)
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`
}

const TITLES: Record<Section, string> = { overview: 'Overview', prompt: 'Prompt', history: 'History', danger: 'Danger' }

const PRESS = /^routine draft r[0-9]+(?:-[a-z0-9]{1,4})?: (create paused|create and start|open in form)$/m

/**
 * `/routines/new` (`name` null) and `/routines/$name/$section`: one file as a form, saved whole with `put_schedule`;
 * `chat` (§6.3) opens the Describe-it pane beside it — `'1'` the intro, else that session.
 */
export function RoutineDetail({ name, section, chat }: { name: string | null; section: Section; chat: string | undefined }) {
  const api = useRoutinesApi()
  const navigate = useNavigate()
  const mobile = useIsMobile()
  const notHub = useNotHub()
  const routines = useRoutines()
  const info = routines.data?.find((s) => s.name === name) ?? null
  const file = useRoutineFile(name)
  const sensors = useSensors().data
  const [ratio, setRatio] = useState(0.5)
  const [verdict, setVerdict] = useState<Verdict | null>(null)
  const fire = useAction((n: string) => api.fire_schedule(n), {
    keys: [KEY],
    onSuccess: (session) => void navigate({ to: '/c/$session', params: { session }, search: {} }),
  })
  const enable = useAction((v: { name: string; enabled: boolean }) => api.enable_schedule(v.name, v.enabled), { keys: [KEY] })
  // 40 §6: the §1.12 verdict on demand for a `when:` file; refusals toast as errors; the card lands under the Trigger row.
  const test = useAction((n: string) => api.test_schedule(n), { keys: [KEY], onSuccess: setVerdict })
  const setChat = (c: string | undefined) =>
    void (name === null
      ? navigate({ to: '/routines/new', search: c ? { chat: c } : {} })
      : navigate({ to: '/routines/$name/$section', params: { name, section }, search: c ? { chat: c } : {} }))
  const sessionId = chat && chat !== '1' ? chat : null
  // §6.6: on a detail page the pane edits this routine — the file as the engine returned it rides along with the first send.
  const seed = name !== null && file.data ? { name, text: fileText(file.data) } : undefined

  // §6.4: the newest `routine_draft` card in the pane's session fills the form live.
  const entries = useSessions((s) => (sessionId ? s.sessions[sessionId]?.entries : undefined))
  const draft = useMemo(() => (entries ? latestDraft(entries) : null), [entries])

  // §6.5: a press on the card. Create → once the list has the name, land on its Overview with the pane open;
  // open in form → focus the form (desktop: scroll the name field into view; phone: close the sheet).
  const [pressed, setPressed] = useState<string | null>(null)
  const [focusForm, setFocusForm] = useState(0)
  useEffect(() => {
    const hear = (e: Event) => {
      const d = (e as CustomEvent<{ session: string; app: string; message: string }>).detail
      if (d.session !== sessionId || d.app !== 'routine-drafts') return
      const choice = PRESS.exec(d.message)?.[1]
      if (choice === 'open in form') {
        if (mobile) setChat(undefined)
        setFocusForm((n) => n + 1)
      } else if (choice) setPressed(draft?.draft.name ?? null)
    }
    window.addEventListener('theuth:app-press', hear)
    return () => window.removeEventListener('theuth:app-press', hear)
  })
  useEffect(() => {
    if (pressed && name !== pressed && routines.data?.some((s) => s.name === pressed)) {
      setPressed(null)
      void navigate({ to: '/routines/$name/$section', params: { name: pressed, section: 'overview' }, search: sessionId ? { chat: sessionId } : {} })
    }
  }, [pressed, name, routines.data, navigate, sessionId])

  const pane = chat !== undefined && (
    <RoutineChat
      session={sessionId}
      seed={seed}
      onSession={(id) => setChat(id)}
      onClose={() => setChat(undefined)}
    />
  )
  const paneOpen = pane !== false
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      <SectionRow back={{ to: '/routines', label: 'Back to routines' }} title={name ?? 'New routine'}>
        {!paneOpen && (
          <Button variant="outline" size="xs" onClick={() => setChat('1')}>
            <MessageSquarePlus /> {name === null ? 'Describe it' : 'Edit by chat'}
          </Button>
        )}
        {info && (
          <>
            <Button variant="outline" size="xs" disabled={fire.isPending || info.error !== null || info.running} onClick={() => fire.mutate(info.name)}>
              <Play /> Run now
            </Button>
            {info.when_sensor !== null && (
              <Button variant="outline" size="xs" disabled={test.isPending || info.error !== null} onClick={() => test.mutate(info.name)}>
                <FlaskConical /> Test
              </Button>
            )}
            <Switch
              size="sm"
              checked={info.enabled}
              disabled={enable.isPending || info.error !== null}
              aria-label={`${info.enabled ? 'Pause' : 'Resume'} ${info.name}`}
              onCheckedChange={(enabled) => enable.mutate({ name: info.name, enabled })}
            />
          </>
        )}
      </SectionRow>
      <div className="flex min-h-0 min-w-0 flex-1">
        <main className="min-h-0 min-w-0 overflow-y-auto" style={paneOpen && !mobile ? { flex: `${ratio} 1 0%` } : { flex: '1 1 0%' }}>
          <div className="mx-auto flex w-full max-w-[720px] flex-col gap-4 p-4">
            {name === null ? (
              <Editor initial={NEW} info={null} section="overview" isNew sensors={sensors} draft={draft} focusForm={focusForm} />
            ) : (
              <>
                {info && <Facts s={info} notHub={notHub} verdict={verdict} sensors={sensors} prompt={file.data?.prompt ?? null} />}
                <nav className="flex gap-1 border-b border-border-subtle" aria-label="Routine sections">
                  {SECTIONS.map((s) => (
                    <Link
                      key={s}
                      to="/routines/$name/$section"
                      params={{ name, section: s }}
                      search={chat ? { chat } : {}}
                      className={cn(
                        '-mb-px border-b-2 px-3 py-1.5 text-sm',
                        s === section ? 'border-foreground font-medium' : 'border-transparent text-fg-secondary hover:text-foreground',
                      )}
                    >
                      {TITLES[s]}
                    </Link>
                  ))}
                </nav>
                {section === 'history' && <History name={name} watcher={info?.when_sensor !== null} />}
                {section === 'danger' && <Danger name={name} />}
                {(section === 'overview' || section === 'prompt') &&
                  (file.isPending ? (
                    <p className="text-sm text-fg-tertiary">Loading…</p>
                  ) : file.isError ? (
                    <p className="text-sm text-danger">{file.error.message}</p>
                  ) : (
                    <Editor key={name} initial={file.data} info={info} section={section} isNew={false} sensors={sensors} draft={draft} focusForm={focusForm} />
                  ))}
              </>
            )}
          </div>
        </main>
        {paneOpen && !mobile && (
          <>
            <Divider dir="row" onRatio={setRatio} />
            <div className="flex min-h-0 min-w-0 flex-col" style={{ flex: `${1 - ratio} 1 0%` }}>
              {pane}
            </div>
          </>
        )}
      </div>
      {mobile && (
        <Sheet open={paneOpen} onOpenChange={(o) => !o && setChat(undefined)}>
          <SheetContent side="bottom" className="flex h-[100dvh] flex-col gap-0 p-0" showCloseButton={false}>
            <SheetTitle className="sr-only">Describe it</SheetTitle>
            {pane}
          </SheetContent>
        </Sheet>
      )}
    </article>
  )
}

const MODE_TEXT: Record<string, string> = {
  skip: 'mode skip — a check that fires while a run is going is a `skipped: busy` row',
  queue: 'mode queue — a fire during a run waits and runs once the run ends',
  restart: 'mode restart — a fire during a run cancels it and starts over on the new payload',
}

/** §4 Overview: the sentence with its prompt gist, then one labelled row per part — Trigger · Stop · Run · Concurrency. */
function Facts({ s, notHub, verdict, sensors, prompt }: { s: ScheduleInfo; notHub: boolean; verdict: Verdict | null; sensors: SensorInfo[] | undefined; prompt: string | null }) {
  const r = fromInfo(s)
  const kind = kindOf(r)
  const title = [s.when, s.when_sensor].filter(Boolean).join(' · ')
  const trigger: string[] = [s.error !== null ? s.error : s.sensor ?? s.when_sensor ? `Every ${r.every} · checks ${s.sensor ?? s.when_sensor?.split(' ')[0]}` : sentence({ ...r, until: null, max_runs: null, expires: null, quiet: false, agent: null, approvals: true, model: null, cwd: null, run_timeout: null })]
  if (s.next) trigger.push(`next ${relative(s.next)}`)
  else if (!s.enabled) trigger.push('paused')
  if (s.last_check) trigger.push(`last check ${lastCheckText(s.last_check)}`)
  const stop: string[] = []
  if (s.until !== null) stop.push(`until \`${s.until}\` passes`)
  if (s.max_runs !== null) stop.push(`at most ${s.max_runs} times (${s.runs} so far)`)
  else if (s.until !== null) stop.push(`${s.runs} so far`)
  if (s.expires !== null) {
    const ms = s.created ? durationMs(s.expires) : null
    stop.push(`expires ${s.expires}${ms !== null && s.created ? ` (${leftText(Date.parse(s.created) + ms - Date.now())} left)` : ''}`)
  }
  const run: string[] = []
  if (s.quiet) run.push('quietly')
  if (!s.approvals) run.push('no approvals')
  run.push(`model: ${s.model ?? 'engine default'}`)
  if (s.cwd) run.push(`in ${s.cwd}`)
  if (s.run_timeout) run.push(`${s.run_timeout} per run`)
  const last = s.running ? 'running' : s.last ? `last ${relative(s.last)}${s.last_result ? ` · ${resultLabel(s.last_result)}` : ''}` : 'never ran'
  return (
    <div className="flex flex-col gap-2 text-sm">
      <h3 className="text-base font-medium leading-snug" title={title}>
        {s.error !== null ? <span className="text-danger">{s.error}</span> : sentence({ ...r, prompt }, sensorArgs(s, sensors))}
      </h3>
      <div className="grid gap-x-3 gap-y-1.5 sm:grid-cols-[auto_1fr]">
        <Fact label="Trigger" badge={kind}>
          {trigger.join(' · ')}
        </Fact>
        {verdict && (
          <div className="sm:col-start-2">
            <VerdictCard v={verdict} />
          </div>
        )}
        <Fact label="Stop">{stop.length ? stop.join(' · ') : kind === 'Once' ? 'after it fires' : 'never — runs until paused or removed'}</Fact>
        <Fact label="Run">
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span>
              {s.session ? (
                <>
                  session:{' '}
                  <Link to="/c/$session" params={{ session: s.session }} search={{}} className="underline underline-offset-2">
                    open
                  </Link>
                </>
              ) : (
                'session: none yet'
              )}
              {` · ${last}`}
              {run.length ? ` · ${run.join(' · ')}` : ''}
            </span>
            <RoutineChips s={{ ...s, last_result: null, running: false }} notHub={notHub} />
          </span>
        </Fact>
        <Fact label="Concurrency">{kind === 'Watcher' ? (MODE_TEXT[s.mode ?? 'skip'] ?? `mode ${s.mode}`) : 'a fire due mid-run waits for the first tick after the run ends; a forced Run now while running is `skipped: busy`'}</Fact>
      </div>
    </div>
  )
}

function Fact({ label, badge, children }: { label: string; badge?: Kind; children: React.ReactNode }) {
  return (
    <>
      <div className="flex items-center gap-2 text-xs text-fg-tertiary sm:pt-0.5">
        <span className="w-20">{label}</span>
        {badge && <KindBadge kind={badge} />}
      </div>
      <div className="min-w-0 break-words text-[13px]">{children}</div>
    </>
  )
}

/** §4 / §5.5: the Test verdict, line plus the three fields spelled out. */
function VerdictCard({ v }: { v: Verdict }) {
  return (
    <div className={cn('rounded-lg border p-2.5 text-xs', v.error !== null ? 'border-danger/40' : 'border-border-subtle')}>
      <div className="font-mono text-fg-secondary">{verdictText(v)}</div>
      {v.error === null && (
        <ul className="mt-1.5 flex flex-wrap gap-x-4 gap-y-1 text-fg-secondary">
          <li className={v.would_fire ? 'text-success' : ''}>{v.would_fire ? 'would fire now' : 'would not fire now'}</li>
          <li className={v.flaps ? 'text-warning' : ''}>{v.flaps ? 'payload flaps — include only stable fields' : 'payload stable'}</li>
          <li>probe: {v.probe ?? '—'}</li>
          {v.needs_approvals_off && <li className="text-warning">needs approvals off (the body mutates)</li>}
        </ul>
      )}
    </div>
  )
}

type Trigger = 'cron' | 'every' | 'at' | 'when'
const TRIGGERS: Record<Trigger, string> = { cron: 'On a clock', every: 'Every so often', at: 'Once', when: 'When a sensor says so' }
const FIRES: [string, string][] = [
  ['on_change', 'on change — each time the sensor’s payload differs from the last fire'],
  ['while_true', 'while true — every check that says so (cooldown spaces them)'],
  ['once', 'once — the first time it says so, then the watcher is done'],
]
const MODES: [string, string][] = [
  ['skip', 'skip — a fire during a run is dropped (`skipped: busy`)'],
  ['queue', 'queue — a fire during a run waits for the run to end'],
  ['restart', 'restart — a fire during a run cancels it and starts over'],
]
const WATCHER_KEYS = ['when', 'fire', 'for', 'cooldown', 'mode', 'memory', 'timeout', 'expires'] as const

/** Which §5 section shows a `plainError` field. */
const SECTION_OF: Record<string, 'trigger' | 'stop' | 'run' | 'concurrency' | 'name'> = {
  trigger: 'trigger',
  every: 'trigger',
  cron: 'trigger',
  at: 'trigger',
  when: 'trigger',
  missed_run: 'trigger',
  fire: 'trigger',
  for: 'trigger',
  cooldown: 'trigger',
  timeout: 'trigger',
  memory: 'trigger',
  stop: 'stop',
  until: 'stop',
  max_runs: 'stop',
  expires: 'stop',
  prompt: 'run',
  cwd: 'run',
  run_timeout: 'run',
  agent: 'run',
  project: 'run',
  model: 'run',
  mode: 'concurrency',
  name: 'name',
}

/**
 * §5: Trigger · Stop · Run · Concurrency, each a fieldset with its common fields and an Advanced disclosure; one
 * `ScheduleFile` state saved whole. Overview and Prompt are two views of the same form on the detail page; `isNew`
 * adds the name, puts the prompt under Run and creates paused / started. A chat `draft` (§6.4) fills the state live.
 */
function Editor({
  initial,
  info,
  section,
  isNew,
  sensors,
  draft,
  focusForm,
}: {
  initial: ScheduleFile
  info: ScheduleInfo | null
  section: Section
  isNew: boolean
  sensors: SensorInfo[] | undefined
  draft: Draft | null
  focusForm: number
}) {
  const api = useRoutinesApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const chat = (useSearch({ strict: false }) as { chat?: string }).chat
  const agents = useAgents().data ?? []
  const projects = useBoardProjects()
  const models = useModels()
  const runTimeoutDefault = shortDuration(Number(useConnection((s) => s.info)?.schedule_run_timeout_secs ?? 0))
  const [f, setF] = useState<ScheduleFile>(initial)
  const [applied, setApplied] = useState<{ callId: string; at: number; file: ScheduleFile } | null>(null)
  useEffect(() => setF(initial), [initial])
  const dirty = isNew ? JSON.stringify(f) !== JSON.stringify(applied?.file ?? NEW) : JSON.stringify(f) !== JSON.stringify(initial)
  // §6.4: every revision of the card follows into the form; the user's own edits are kept unless they agree.
  useEffect(() => {
    if (!draft || draft.callId === applied?.callId) return
    const next = draftToFile(draft.draft)
    if (dirty && !window.confirm('Replace your edits with the new draft?')) {
      setApplied((a) => (a ? { ...a, callId: draft.callId } : { callId: draft.callId, at: Date.now(), file: f }))
      return
    }
    setF(isNew ? next : { ...next, enabled: f.enabled, created: f.created })
    setApplied({ callId: draft.callId, at: Date.now(), file: next })
  }, [draft, applied, dirty, f, isNew])
  const nameRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (focusForm) nameRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }, [focusForm])

  const set = <K extends keyof ScheduleFile>(k: K, v: ScheduleFile[K]) => setF((x) => ({ ...x, [k]: v }))
  const trigger: Trigger = f.when !== null ? 'when' : f.every !== null ? 'every' : f.at !== null ? 'at' : 'cron'
  const setTrigger = (k: Trigger) =>
    setF((x) => ({
      ...x,
      ...Object.fromEntries(WATCHER_KEYS.map((w) => [w, null])),
      cron: k === 'cron' ? '0 9 * * 1-5' : null,
      every: k === 'every' ? '1h' : k === 'when' ? '60s' : null,
      at: k === 'at' ? '' : null,
      when: k === 'when' ? '' : null,
      missed_run: k === 'cron' ? x.missed_run : false,
    }))
  const [verdict, setVerdict] = useState<{ v: Verdict; when: string } | null>(null)
  const testFile = useMutation({ mutationFn: () => api.test_schedule_file(f), onSuccess: (v) => setVerdict({ v, when: f.when ?? '' }), onError: (e) => toast.error(errorText(e)) })
  const save = useMutation({
    mutationFn: (enabled: boolean) => api.put_schedule({ ...f, enabled }),
    onSuccess: (s) => {
      void qc.invalidateQueries({ queryKey: KEY })
      toast(isNew ? `Created ${s.name}` : `Saved ${s.name}`)
      // The pane (if open) stays beside the new routine's Overview (§6.5).
      if (isNew) void navigate({ to: '/routines/$name/$section', params: { name: s.name, section: 'overview' }, search: chat ? { chat } : {} })
    },
  })
  const err = save.error ? plainError(errorText(save.error)) : null
  const errAt = (field: string) => (err?.field === field ? err.text : undefined)
  const errIn = (sec: string) => (err?.field && SECTION_OF[err.field] === sec && !FIELD_OWN[err.field] ? err.text : undefined)
  const str = (v: string) => v.trim() || null

  // Sensor picker state, derived from `when` (§5.1).
  const parsed = f.when !== null ? parseWhen(f.when) : null
  const sensor = parsed ? sensors?.find((x) => x.name === parsed.file) : undefined
  const argOf = (k: string) => parsed?.args.find(([a]) => a === k)?.[1].replace(/^"(.*)"$/, '$1') ?? ''
  const setArg = (k: string, v: string) => {
    if (!parsed) return
    const rest = parsed.args.filter(([a]) => a !== k)
    const val = v.trim()
    const next = val ? [...rest, [k, /\s/.test(val) ? JSON.stringify(val) : val] as [string, string]] : rest
    // header order first, unknown keys after, so the file reads like the docstring.
    const order = (a: string) => sensor?.args.findIndex((x) => x.name === a) ?? -1
    next.sort((a, b) => (order(a[0]) === -1 ? 1e9 : order(a[0])) - (order(b[0]) === -1 ? 1e9 : order(b[0])))
    set('when', [parsed.file, ...next.map(([a, b]) => `${a}=${b}`)].join(' '))
  }
  const argsReady = !!sensor && sensor.args.filter((a) => a.required).every((a) => argOf(a.name) !== '')
  const tested = verdict !== null && verdict.when === f.when && verdict.v.error === null
  const ready = !!f.prompt.trim() && (!isNew || !!f.name.trim())
  const live = sentence(fromFile(f), sensor?.args)

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault()
        save.mutate(isNew ? false : f.enabled)
      }}
    >
      {(isNew || section === 'overview') && (
        <>
          {isNew && (
            <Field label="Name" id="rt-name" hint="a-z, 0-9 and dashes; the file name" error={errAt('name')}>
              <Input ref={nameRef} id="rt-name" value={f.name} placeholder="daily-digest" autoFocus onChange={(e) => set('name', e.target.value)} />
            </Field>
          )}
          <div aria-live="polite" className="rounded-md bg-layer-1 px-3 py-2 text-[13px]">
            <span className="text-fg-secondary">{live || <span className="text-fg-tertiary">The sentence builds as you fill the form.</span>}</span>
            {applied && (
              <span className="ml-2 text-xs text-fg-tertiary">
                from the chat draft · {new Date(applied.at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}
              </span>
            )}
          </div>
          {err?.field === null && <p className="text-xs text-danger">{err.text}</p>}

          <Part legend="Trigger" hint="what wakes it" error={errIn('trigger')}>
            <Field label="Wake it" id="rt-trigger">
              <select id="rt-trigger" className="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm sm:max-w-xs" value={trigger} onChange={(e) => setTrigger(e.target.value as Trigger)}>
                {(Object.keys(TRIGGERS) as Trigger[]).map((k) => (
                  <option key={k} value={k}>
                    {TRIGGERS[k]}
                  </option>
                ))}
              </select>
            </Field>
            {trigger === 'cron' && (
              <Field label="Cron" id="rt-cron" hint={`${(f.cron ?? '').trim() ? (cronWords(f.cron ?? '') ?? 'words after save') : 'five fields: minute hour day month weekday'} · local time`} error={errAt('cron')}>
                <Input id="rt-cron" className="font-mono" value={f.cron ?? ''} placeholder="0 9 * * 1-5" onChange={(e) => set('cron', e.target.value)} />
              </Field>
            )}
            {(trigger === 'every' || trigger === 'when') && (
              <Field label="Interval" id="rt-every" hint={trigger === 'when' ? 'how often the sensor is checked; at least 30s' : 'e.g. 30m or 6h; 0s = again as soon as a run ends'} error={errAt('every')}>
                <Input id="rt-every" className="font-mono sm:max-w-xs" value={f.every ?? ''} placeholder={trigger === 'when' ? '60s' : '30m'} onChange={(e) => set('every', e.target.value)} />
              </Field>
            )}
            {trigger === 'at' && (
              <Field label="When" id="rt-at" error={errAt('at')}>
                <Input id="rt-at" type="datetime-local" className="sm:max-w-xs" value={f.at ?? ''} onChange={(e) => set('at', e.target.value)} />
              </Field>
            )}
            {trigger === 'when' && parsed && (
              <>
                <Field label="Sensor" id="rt-sensor" hint={sensor?.usage ?? 'a Python file in ~/.theuth/sensors/'} error={errAt('when')}>
                  <select
                    id="rt-sensor"
                    className="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                    value={parsed.file}
                    onChange={(e) => {
                      set('when', e.target.value)
                      setVerdict(null)
                    }}
                  >
                    <option value="">pick a sensor…</option>
                    {sensors?.map((x) => {
                      const why = x.error ? 'bad header' : x.needs_cpython && !x.cpython_available ? 'needs CPython' : null
                      return (
                        <option key={x.name} value={x.name} disabled={why !== null}>
                          {x.name}
                          {x.description ? ` · ${x.description}` : ''}
                          {x.edited ? ' · edited' : ''}
                          {why ? ` · ${why}` : ''}
                        </option>
                      )
                    })}
                    {parsed.file && !sensors?.some((x) => x.name === parsed.file) && <option value={parsed.file}>{parsed.file}</option>}
                  </select>
                </Field>
                {sensor && sensor.args.length > 0 ? (
                  <div className="grid gap-3 sm:grid-cols-2">
                    {sensor.args.map((a) => (
                      <Field key={a.name} label={`${a.name}${a.required ? ' *' : ''}`} id={`rt-arg-${a.name}`} hint={a.help}>
                        <Input id={`rt-arg-${a.name}`} className="font-mono" list={a.example === 'secret' ? 'rt-secrets' : undefined} value={argOf(a.name)} placeholder={a.example ?? ''} onChange={(e) => setArg(a.name, e.target.value)} />
                      </Field>
                    ))}
                    {sensor.args.some((a) => a.example === 'secret') && <SecretNames />}
                  </div>
                ) : (
                  parsed.file && (
                    <Field label="Arguments" id="rt-args" hint="key=value pairs, space-separated; quote a value with spaces">
                      <Input id="rt-args" className="font-mono" value={f.when?.slice(parsed.file.length).trim() ?? ''} onChange={(e) => set('when', `${parsed.file} ${e.target.value}`.trim())} />
                    </Field>
                  )
                )}
                <Radio id="rt-fire" label="Fire rule" value={f.fire ?? 'on_change'} options={FIRES} onChange={(v) => set('fire', v === 'on_change' ? null : v)} />
                <div className="flex flex-wrap items-center gap-2">
                  <Button type="button" variant="outline" size="xs" disabled={!argsReady || !f.every?.trim() || testFile.isPending} onClick={() => testFile.mutate()}>
                    <FlaskConical /> Test now
                  </Button>
                  <span className="text-xs text-fg-tertiary">{argsReady ? 'a dry run of the sensor; nothing is saved' : 'set the sensor and its required arguments first'}</span>
                </div>
                {verdict && <VerdictCard v={verdict.v} />}
                <Advanced>
                  <div className="grid gap-3 sm:grid-cols-3">
                    <Field label="Hold for" id="rt-for" hint="true this long before it fires; 0s = at once" error={errAt('for')}>
                      <Input id="rt-for" className="font-mono" value={f.for ?? ''} placeholder="0s" onChange={(e) => set('for', str(e.target.value))} />
                    </Field>
                    <Field label="Cooldown" id="rt-cooldown" hint="quiet time after a fire" error={errAt('cooldown')}>
                      <Input id="rt-cooldown" className="font-mono" value={f.cooldown ?? ''} placeholder="0s" onChange={(e) => set('cooldown', str(e.target.value))} />
                    </Field>
                    <Field label="Check limit" id="rt-timeout" hint="the sensor's wall clock; ≤ 60s and under half the interval" error={errAt('timeout')}>
                      <Input id="rt-timeout" className="font-mono" value={f.timeout ?? ''} placeholder="20s" onChange={(e) => set('timeout', str(e.target.value))} />
                    </Field>
                  </div>
                  <label className="flex items-center gap-2 text-[13px] text-fg-secondary">
                    <Switch size="sm" checked={f.memory !== 'clear'} onCheckedChange={(v) => set('memory', v ? null : 'clear')} /> keep the session between fires
                  </label>
                  <p className="text-xs text-fg-tertiary">What happens when it fires mid-run is the Concurrency section below.</p>
                </Advanced>
              </>
            )}
            {trigger === 'cron' && (
              <Advanced>
                <label className="flex items-center gap-2 text-[13px] text-fg-secondary">
                  <Switch size="sm" checked={f.missed_run} onCheckedChange={(v) => set('missed_run', v)} /> run a missed occurrence at boot
                </label>
                {errAt('missed_run') && <p className="text-xs text-danger">{errAt('missed_run')}</p>}
              </Advanced>
            )}
          </Part>

          {trigger !== 'at' && (
            <Part legend="Stop" hint={trigger === 'when' ? 'when it is over — a watcher needs at least one of these' : 'when it is over; nothing here = it runs until paused'} error={errIn('stop')}>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Stop after N runs" id="rt-max" hint={info && (f.until !== null || f.max_runs !== null) ? `runs so far: ${runCount(info)}` : 'empty = no cap'} error={errAt('max_runs')}>
                  <Input id="rt-max" type="number" min={1} value={f.max_runs ?? ''} onChange={(e) => set('max_runs', e.target.value ? Math.max(1, Math.floor(Number(e.target.value))) : null)} />
                </Field>
                {trigger === 'when' && <ExpiresField f={f} set={set} error={errAt('expires')} required />}
              </div>
              <Advanced>
                <div className="grid gap-3 sm:grid-cols-2">
                  <Field label="Until a command passes" id="rt-until" hint="runs in the directory before each fire; exit 0 = done" error={errAt('until')}>
                    <Input id="rt-until" className="font-mono" value={f.until ?? ''} placeholder='test "$(gh pr view 42 -q .state)" = MERGED' onChange={(e) => set('until', str(e.target.value))} />
                  </Field>
                  {trigger !== 'when' && <ExpiresField f={f} set={set} error={errAt('expires')} />}
                </div>
              </Advanced>
            </Part>
          )}

          <Part legend="Run" hint="what the turn is like" error={errIn('run')}>
            {isNew && <PromptField f={f} set={set} rows={8} error={errAt('prompt')} />}
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="flex items-center gap-2 text-[13px] text-fg-secondary" title="the run's reply is not delivered; only message_user reaches you">
                <Switch size="sm" checked={f.quiet} onCheckedChange={(v) => set('quiet', v)} /> quietly
              </label>
              <label className="flex items-center gap-2 text-[13px] text-fg-secondary" title="off: the run never asks permission; a watcher whose prompt pushes, merges, commits or writes needs this off">
                <Switch size="sm" checked={f.approvals} onCheckedChange={(v) => set('approvals', v)} /> approvals
              </label>
            </div>
            <Advanced>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Agent" id="rt-agent" hint="each fire creates a task owned by them" error={errAt('agent')}>
                  <NativeSelect
                    id="rt-agent"
                    value={f.agent ?? ''}
                    onChange={(v) => set('agent', v || null)}
                    options={[...new Set([...(f.agent ? [f.agent] : []), ...agents.map((a) => a.file.name)])]}
                    none="run in its own session"
                  />
                </Field>
                <Field label="Project" id="rt-project" hint="label of the task; optional">
                  <Input id="rt-project" list="rt-projects" value={f.project ?? ''} onChange={(e) => set('project', str(e.target.value))} />
                  <datalist id="rt-projects">
                    {projects.map((p) => (
                      <option key={p} value={p} />
                    ))}
                  </datalist>
                </Field>
                <Field label="Model" id="rt-model" hint="the run's session; default = the engine's default model">
                  <Select value={f.model ?? NONE} onValueChange={(v) => set('model', v === NONE ? null : v)}>
                    <SelectTrigger id="rt-model" size="sm" className="w-full">
                      <SelectValue placeholder="Engine default" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE}>Engine default</SelectItem>
                      {models.data?.map((m) => (
                        <SelectItem key={m.id} value={m.id}>
                          {m.name} <span className="text-fg-tertiary">{m.provider}</span>
                        </SelectItem>
                      ))}
                      {f.model && !models.data?.some((m) => m.id === f.model) && <SelectItem value={f.model}>{f.model}</SelectItem>}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Directory" id="rt-cwd" hint="where until runs and the run starts" error={errAt('cwd')}>
                  <Input id="rt-cwd" className="font-mono" value={f.cwd ?? ''} placeholder="~/src/app" onChange={(e) => set('cwd', str(e.target.value))} />
                </Field>
                <Field label="Time limit per run" id="rt-run-timeout" hint={`45m, 2h; empty = the Settings default (${runTimeoutDefault})`} error={errAt('run_timeout')}>
                  <Input id="rt-run-timeout" className="font-mono" value={f.run_timeout ?? ''} placeholder={runTimeoutDefault} onChange={(e) => set('run_timeout', str(e.target.value))} />
                </Field>
              </div>
            </Advanced>
          </Part>

          <Part legend="Concurrency" hint="what happens if it is due while a run is going" error={errIn('concurrency')}>
            {trigger === 'when' ? (
              <Radio id="rt-mode" label="Mode" value={f.mode ?? 'skip'} options={MODES} onChange={(v) => set('mode', v === 'skip' ? null : v)} />
            ) : (
              <p className="text-[13px] text-fg-secondary">
                {trigger === 'at' ? 'A one-off fires once; nothing to collide with.' : 'A fire due mid-run waits for the first tick after the run ends; a forced Run now while running is `skipped: busy`.'}
              </p>
            )}
          </Part>
        </>
      )}
      {!isNew && section === 'prompt' && <PromptField f={f} set={set} rows={18} error={errAt('prompt')} />}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {isNew ? (
          <>
            <Button type="submit" size="sm" variant="outline" disabled={!ready || save.isPending}>
              Create paused
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!ready || save.isPending || (trigger === 'when' && !tested)}
              title={trigger === 'when' && !tested ? 'test the sensor first' : undefined}
              onClick={() => save.mutate(true)}
            >
              Create and start
            </Button>
          </>
        ) : (
          <>
            <Button type="button" variant="ghost" size="sm" disabled={!dirty || save.isPending} onClick={() => setF(initial)}>
              Revert
            </Button>
            <Button type="submit" size="sm" disabled={!dirty || save.isPending || !ready}>
              Save
            </Button>
          </>
        )}
      </div>
    </form>
  )
}

/** Fields that render their own `plainError` line; the section line is for the rest. */
const FIELD_OWN: Record<string, true> = { name: true, cron: true, every: true, at: true, when: true, for: true, cooldown: true, timeout: true, missed_run: true, max_runs: true, expires: true, until: true, prompt: true, agent: true, cwd: true, run_timeout: true }

function Part({ legend, hint, error, children }: { legend: string; hint: string; error?: string; children: React.ReactNode }) {
  return (
    <fieldset className="flex flex-col gap-3 rounded-lg border border-border-subtle p-3">
      <legend className="px-1 text-[13px] font-medium">
        {legend} <span className="font-normal text-fg-tertiary">· {hint}</span>
      </legend>
      {error && <p className="text-xs text-danger">{error}</p>}
      {children}
    </fieldset>
  )
}

function Advanced({ children }: { children: React.ReactNode }) {
  return (
    <Collapsible>
      <CollapsibleTrigger className="flex items-center gap-1 text-[13px] text-fg-secondary [&[data-state=open]>svg]:rotate-90">
        <ChevronRight className="size-3.5 transition-transform" /> Advanced
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col gap-3 pt-3">{children}</CollapsibleContent>
    </Collapsible>
  )
}

function Radio({ id, label, value, options, onChange }: { id: string; label: string; value: string; options: [string, string][]; onChange: (v: string) => void }) {
  return (
    <fieldset className="flex flex-col gap-1.5">
      <legend className="text-[13px]">{label}</legend>
      {options.map(([v, text]) => (
        <label key={v} className="flex items-start gap-2 text-[13px] text-fg-secondary">
          <input type="radio" name={id} value={v} checked={value === v} onChange={() => onChange(v)} className="mt-1" /> {text}
        </label>
      ))}
    </fieldset>
  )
}

type Set = <K extends keyof ScheduleFile>(k: K, v: ScheduleFile[K]) => void

function ExpiresField({ f, set, error, required }: { f: ScheduleFile; set: Set; error?: string; required?: boolean }) {
  return (
    <Field label={`Expires after${required ? ' *' : ''}`} id="rt-expires" hint="14d, 90d; longer than the interval" error={error}>
      <Input id="rt-expires" className="font-mono" value={f.expires ?? ''} placeholder="14d" onChange={(e) => set('expires', e.target.value.trim() || null)} />
    </Field>
  )
}

function PromptField({ f, set, rows, error }: { f: ScheduleFile; set: Set; rows: number; error?: string }) {
  return (
    <Field label="Prompt" id="rt-prompt" hint="markdown; what the run does, written for nobody being there" error={error}>
      <Textarea id="rt-prompt" rows={rows} className="font-mono text-[13px]" value={f.prompt} onChange={(e) => set('prompt', e.target.value)} />
    </Field>
  )
}

/** `list_secrets` names as a datalist for a `secret` sensor arg (a vault name, never the value). */
function SecretNames() {
  const api = useApi()
  const [names, setNames] = useState<string[]>([])
  useEffect(() => {
    api
      .list_secrets()
      .then((l) => setNames(l.secrets.map((s) => s.name)))
      .catch(() => undefined)
  }, [api])
  return (
    <datalist id="rt-secrets">
      {names.map((n) => (
        <option key={n} value={n} />
      ))}
    </datalist>
  )
}

/** Consecutive `check` rows folded into one line (§4 History). */
type Row = { kind: 'one'; r: RunInfo } | { kind: 'checks'; rows: RunInfo[] }

function fold(runs: RunInfo[]): Row[] {
  const out: Row[] = []
  for (const r of runs) {
    const last = out[out.length - 1]
    if (r.kind === 'check' && last?.kind === 'checks') last.rows.push(r)
    else if (r.kind === 'check') out.push({ kind: 'checks', rows: [r] })
    else out.push({ kind: 'one', r })
  }
  return out
}

const hhmm = (iso: string) => new Date(iso).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false })

/** The runs of one routine, newest first: a fired run with its envelope and report, folded checks, end lines, skips. */
function History({ name, watcher }: { name: string; watcher: boolean }) {
  const runs = useRoutineHistory(name)
  const [shown, setShown] = useState<string | null>(null)
  if (runs.isPending) return <p className="text-sm text-fg-tertiary">Loading…</p>
  if (runs.isError) return <p className="text-sm text-danger">{runs.error.message}</p>
  if (!runs.data?.length) return <Empty>No run yet.</Empty>
  return (
    <ul className="divide-y divide-border-subtle text-xs">
      {fold(runs.data).map((row, i) => {
        const key = row.kind === 'one' ? `${row.r.fired_at}${row.r.session ?? ''}` : `checks-${i}`
        if (row.kind === 'checks') {
          const counts: Record<string, number> = {}
          for (const r of row.rows) {
            const k = r.result?.split(':')[0]?.trim() ?? 'check'
            counts[k] = (counts[k] ?? 0) + 1
          }
          const first = row.rows[row.rows.length - 1]
          const last = row.rows[0]
          return (
            <li key={key} className="py-1.5 text-fg-tertiary">
              <button type="button" className="flex items-center gap-1.5 text-left [&[aria-expanded=true]>svg]:rotate-90" aria-expanded={shown === key} onClick={() => setShown((v) => (v === key ? null : key))}>
                <ChevronRight className="size-3 transition-transform" />
                {row.rows.length} {row.rows.length === 1 ? 'check' : 'checks'} ·{' '}
                {Object.entries(counts)
                  .map(([k, n]) => `${k} ×${n}`)
                  .join(', ')}
                {first && last && ` · ${hhmm(first.fired_at)}–${hhmm(last.fired_at)}`}
              </button>
              {shown === key && (
                <ul className="mt-1 ml-5 flex flex-col gap-0.5">
                  {row.rows.map((r) => (
                    <li key={r.fired_at} className="flex gap-3 tabular-nums" title={r.fired_at}>
                      <span>{new Date(r.fired_at).toLocaleString()}</span>
                      <span className="font-mono">{r.result}</span>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          )
        }
        const r = row.r
        if (r.kind === 'end')
          return (
            <li key={key} className="py-1.5 text-fg-tertiary" title={r.fired_at}>
              {r.result} · {new Date(r.fired_at).toLocaleString()}
            </li>
          )
        const kind = resultKind(r.result, r.kind)
        return (
          <li key={key} className="flex flex-col gap-1 py-1.5">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="tabular-nums" title={r.fired_at}>
                {new Date(r.fired_at).toLocaleString()}
              </span>
              <span className="tabular-nums text-fg-tertiary">{r.finished_at ? took(r.fired_at, r.finished_at) : r.kind === 'run' ? <Pulse className="bg-success" /> : null}</span>
              {kind ? (
                <Badge variant="outline" className={cn('font-normal', RESULT_CLASS[kind])} title={r.result ?? undefined}>
                  {resultLabel(r.result, r.kind)}
                </Badge>
              ) : (
                <span className="text-fg-tertiary">running</span>
              )}
              {r.report !== null && (
                <button type="button" className="min-w-0 flex-1 truncate text-left text-fg-secondary" title={r.report} aria-expanded={shown === key} onClick={() => setShown((v) => (v === key ? null : key))}>
                  {r.report.split('\n', 1)[0]?.slice(0, 80)}
                </button>
              )}
              {r.session && (
                <Link to="/c/$session" params={{ session: r.session }} search={{}} className="ml-auto underline underline-offset-2">
                  session
                </Link>
              )}
            </div>
            {watcher && r.envelope && <Envelope line={r.envelope} />}
            {shown === key && r.report !== null && <pre className="whitespace-pre-wrap font-mono text-xs text-fg-secondary">{r.report}</pre>}
          </li>
        )
      })}
    </ul>
  )
}

/** `subject=pr/482 stage=waiting_ci events=ci_failed,new_commits …` → chips for the three envelope keys, the rest as mono text. */
function Envelope({ line }: { line: string }) {
  const chips: [string, string][] = []
  const rest: string[] = []
  for (const tok of line.trim().split(/\s+/)) {
    const m = /^(subject|stage|events)=(.*)$/.exec(tok)
    if (m) chips.push([m[1] ?? '', m[2] ?? ''])
    else rest.push(tok)
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {chips.map(([k, v]) => (
        <Badge key={k} variant="secondary" className="font-normal">
          <span className="text-fg-tertiary">{k}</span> {v}
        </Badge>
      ))}
      {rest.length > 0 && <span className="font-mono text-fg-tertiary">{rest.join(' ')}</span>}
    </div>
  )
}

function Danger({ name }: { name: string }) {
  const api = useRoutinesApi()
  const navigate = useNavigate()
  const del = useAction((n: string) => api.delete_schedule(n), { keys: [KEY], onSuccess: () => void navigate({ to: '/routines' }) })
  return (
    <div className="flex items-center justify-between gap-4 rounded-lg border border-danger/40 p-3 text-sm">
      <div>
        <div className="font-medium">Delete this routine</div>
        <div className="text-fg-secondary">Removes ~/.theuth/schedules/{name}.md. Its history keeps a `removed` row; a running turn is not stopped.</div>
      </div>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="destructive" size="sm" disabled={del.isPending}>
            Delete
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {name}?</AlertDialogTitle>
            <AlertDialogDescription>The file is removed and the routine never fires again. This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => del.mutate(name)}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
