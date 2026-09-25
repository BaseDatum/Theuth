import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, PanelLeft, Play } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Field } from '@/components/agents/RuntimeAccessFields'
import { NativeSelect } from '@/components/routines/NativeSelect'
import { Pulse } from '@/components/chat/Pulse'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
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
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { RunInfo } from '@/rpc/bindings/RunInfo'
import type { ScheduleFile } from '@/rpc/bindings/ScheduleFile'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import { RoutineChips } from './RoutinesView'
import { SECTIONS, type Section, cadence, cronWords, relative, resultKind, runCount, took } from './helpers'
import { KEY, useNotHub, useRoutineFile, useRoutineHistory, useRoutines } from './queries'

const NEW: ScheduleFile = {
  name: '',
  cron: '0 9 * * 1-5',
  every: null,
  at: null,
  quiet: false,
  missed_run: false,
  model: null,
  enabled: true,
  until: null,
  max_runs: null,
  cwd: null,
  approvals: true,
  agent: null,
  project: null,
  prompt: '',
}

const TITLES: Record<Section, string> = { overview: 'Overview', prompt: 'Prompt', history: 'History', danger: 'Danger' }

/** `/routines/new` (`name` null) and `/routines/$name/$section`: one file as a form, saved whole with `put_schedule`. */
export function RoutineDetail({ name, section }: { name: string | null; section: Section }) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const api = useApi()
  const navigate = useNavigate()
  const notHub = useNotHub()
  const info = useRoutines().data?.find((s) => s.name === name) ?? null
  const file = useRoutineFile(name)
  const fire = useAction((n: string) => api.fire_schedule(n), {
    keys: [KEY],
    onSuccess: (session) => void navigate({ to: '/c/$session', params: { session } }),
  })
  const enable = useAction((v: { name: string; enabled: boolean }) => api.enable_schedule(v.name, v.enabled), { keys: [KEY] })
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <Button asChild variant="ghost" size="icon-sm" aria-label="Back to routines">
          <Link to="/routines">
            <ChevronLeft />
          </Link>
        </Button>
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{name ?? 'New routine'}</h2>
        {info && (
          <>
            <Button variant="outline" size="xs" disabled={fire.isPending || info.error !== null || info.running} onClick={() => fire.mutate(info.name)}>
              <Play /> Run now
            </Button>
            <Switch
              size="sm"
              checked={info.enabled}
              disabled={enable.isPending || info.error !== null}
              aria-label={`${info.enabled ? 'Pause' : 'Resume'} ${info.name}`}
              onCheckedChange={(enabled) => enable.mutate({ name: info.name, enabled })}
            />
          </>
        )}
      </header>
      <main className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[720px] flex-col gap-4 p-4">
          {name === null ? (
            <Editor initial={NEW} info={null} section="overview" isNew />
          ) : (
            <>
              {info && <Facts s={info} notHub={notHub} />}
              <nav className="flex gap-1 border-b border-border-subtle" aria-label="Routine sections">
                {SECTIONS.map((s) => (
                  <Link
                    key={s}
                    to="/routines/$name/$section"
                    params={{ name, section: s }}
                    className={cn(
                      '-mb-px border-b-2 px-3 py-1.5 text-sm',
                      s === section ? 'border-foreground font-medium' : 'border-transparent text-fg-secondary hover:text-foreground',
                    )}
                  >
                    {TITLES[s]}
                  </Link>
                ))}
              </nav>
              {section === 'history' && <History name={name} />}
              {section === 'danger' && <Danger name={name} />}
              {(section === 'overview' || section === 'prompt') &&
                (file.isPending ? (
                  <p className="text-sm text-fg-tertiary">Loading…</p>
                ) : file.isError ? (
                  <p className="text-sm text-danger">{file.error.message}</p>
                ) : (
                  <Editor key={name} initial={file.data} info={info} section={section} isNew={false} />
                ))}
            </>
          )}
        </div>
      </main>
    </article>
  )
}

/** Paperclip's overview facts: cadence · next · last · runs, plus the chips. */
function Facts({ s, notHub }: { s: ScheduleInfo; notHub: boolean }) {
  const loop = s.until !== null || s.max_runs !== null
  return (
    <div className="grid gap-2 text-sm sm:grid-cols-4">
      <Fact label="Cadence" title={s.when}>
        {s.error !== null ? <span className="text-danger">{s.error}</span> : cadence(s)}
      </Fact>
      <Fact label="Next run" title={s.next ? new Date(s.next).toLocaleString() : undefined}>
        {s.next ? relative(s.next) : s.enabled ? '—' : 'paused'}
      </Fact>
      <Fact label="Last run" title={s.last ? new Date(s.last).toLocaleString() : undefined}>
        {s.running ? (
          <span className="flex items-center gap-1.5">
            <Pulse className="bg-success" /> running
          </span>
        ) : s.last ? (
          `${relative(s.last)}${s.last_result ? ` · ${resultKind(s.last_result)}` : ''}`
        ) : (
          'never'
        )}
      </Fact>
      <Fact label={loop ? 'Loop runs' : 'Session'}>
        {loop ? (
          runCount(s)
        ) : s.session ? (
          <Link to="/c/$session" params={{ session: s.session }} className="underline underline-offset-2">
            open
          </Link>
        ) : (
          '—'
        )}
      </Fact>
      <div className="flex flex-wrap items-center gap-1.5 sm:col-span-4">
        <RoutineChips s={s} notHub={notHub} />
      </div>
    </div>
  )
}

function Fact({ label, title, children }: { label: string; title?: string; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border-subtle p-2.5" title={title}>
      <div className="text-xs text-fg-tertiary">{label}</div>
      <div className="mt-0.5 truncate tabular-nums">{children}</div>
    </div>
  )
}

type Kind = 'cron' | 'every' | 'at'
const KINDS: readonly Kind[] = ['cron', 'every', 'at']

/** `every 5m` / `once at …` / cron in words, live, from the form's clause; the engine's croner text lands after save. */
function preview(f: ScheduleFile): string {
  if (f.every !== null) return f.every.trim() ? `every ${f.every.trim()}` : 'interval, e.g. 30m or 6h'
  if (f.at !== null) return f.at ? cadence({ when: `at ${f.at}`, describe: '' }) : 'pick an instant'
  const c = (f.cron ?? '').trim()
  return c ? (cronWords(c) ?? `cron ${c} (words after save)`) : 'five fields: minute hour day month weekday'
}

/**
 * Overview (cadence + flags + owner + loop) and Prompt (the body) are two views of one form: edits survive the tab
 * switch and Save writes the whole file. `isNew` adds the name field and creates the file instead.
 */
function Editor({ initial, info, section, isNew }: { initial: ScheduleFile; info: ScheduleInfo | null; section: Section; isNew: boolean }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const agents = useAgents().data ?? []
  const projects = useBoardProjects()
  const [f, setF] = useState<ScheduleFile>(initial)
  useEffect(() => setF(initial), [initial])
  const set = <K extends keyof ScheduleFile>(k: K, v: ScheduleFile[K]) => setF((x) => ({ ...x, [k]: v }))
  const kind: Kind = f.every !== null ? 'every' : f.at !== null ? 'at' : 'cron'
  const setKind = (k: Kind) => setF((x) => ({ ...x, cron: k === 'cron' ? '0 9 * * 1-5' : null, every: k === 'every' ? '1h' : null, at: k === 'at' ? '' : null }))
  const dirty = isNew || JSON.stringify(f) !== JSON.stringify(initial)
  const save = useMutation({
    mutationFn: () => api.put_schedule(f),
    onSuccess: (s) => {
      void qc.invalidateQueries({ queryKey: KEY })
      toast(isNew ? `Created ${s.name}` : `Saved ${s.name}`)
      if (isNew) void navigate({ to: '/routines/$name/$section', params: { name: s.name, section: 'overview' } })
    },
    onError: (e) => toast.error(errorText(e)),
  })
  const str = (v: string) => v.trim() || null
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault()
        save.mutate()
      }}
    >
      {(isNew || section === 'overview') && (
        <>
          {isNew && (
            <Field label="Name" id="rt-name" hint="a-z, 0-9 and dashes; the file name">
              <Input id="rt-name" value={f.name} placeholder="daily-digest" autoFocus onChange={(e) => set('name', e.target.value)} />
            </Field>
          )}
          <fieldset className="grid gap-3 rounded-lg border border-border-subtle p-3 sm:grid-cols-[auto_1fr]">
            <legend className="px-1 text-[13px] text-fg-secondary">Cadence</legend>
            <NativeSelect id="rt-kind" value={kind} onChange={(v) => setKind(v as Kind)} options={KINDS} />
            {kind === 'cron' && (
              <Input id="rt-cron" className="font-mono" value={f.cron ?? ''} placeholder="0 9 * * 1-5" onChange={(e) => set('cron', e.target.value)} />
            )}
            {kind === 'every' && <Input id="rt-every" className="font-mono" value={f.every ?? ''} placeholder="30m" onChange={(e) => set('every', e.target.value)} />}
            {kind === 'at' && (
              <Input id="rt-at" type="datetime-local" value={f.at ?? ''} onChange={(e) => set('at', e.target.value)} />
            )}
            <p className="text-[13px] text-fg-tertiary sm:col-span-2" aria-live="polite">
              {preview(f)}
              {kind === 'cron' && ' · local time'}
            </p>
            {kind === 'cron' && (
              <label className="flex items-center gap-2 text-[13px] text-fg-secondary sm:col-span-2">
                <Switch size="sm" checked={f.missed_run} onCheckedChange={(v) => set('missed_run', v)} /> run a missed occurrence at boot
              </label>
            )}
          </fieldset>
          <div className="grid gap-3 sm:grid-cols-3">
            <label className="flex items-center gap-2 text-[13px] text-fg-secondary">
              <Switch size="sm" checked={f.enabled} onCheckedChange={(v) => set('enabled', v)} /> enabled
            </label>
            <label className="flex items-center gap-2 text-[13px] text-fg-secondary" title="the run's reply is not delivered; only message_user reaches you">
              <Switch size="sm" checked={f.quiet} onCheckedChange={(v) => set('quiet', v)} /> quiet
            </label>
            <label className="flex items-center gap-2 text-[13px] text-fg-secondary" title="off: the run never asks permission">
              <Switch size="sm" checked={f.approvals} onCheckedChange={(v) => set('approvals', v)} /> approvals
            </label>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Agent" id="rt-agent" hint="each fire creates a task owned by them">
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
            <Field label="Model" id="rt-model" hint="optional">
              <Input id="rt-model" value={f.model ?? ''} placeholder="provider/model" onChange={(e) => set('model', str(e.target.value))} />
            </Field>
            <Field label="Directory" id="rt-cwd" hint="where until runs and the run starts">
              <Input id="rt-cwd" className="font-mono" value={f.cwd ?? ''} placeholder="~/src/app" onChange={(e) => set('cwd', str(e.target.value))} />
            </Field>
          </div>
          {kind !== 'at' && (
            <fieldset className="grid gap-3 rounded-lg border border-border-subtle p-3 sm:grid-cols-2">
              <legend className="px-1 text-[13px] text-fg-secondary">Loop · stop condition (33 §2)</legend>
              <Field label="Until" id="rt-until" hint="shell command; exit 0 ends the loop">
                <Input id="rt-until" className="font-mono" value={f.until ?? ''} placeholder='test "$(gh pr view 42 -q .state)" = MERGED' onChange={(e) => set('until', str(e.target.value))} />
              </Field>
              <Field label="Max runs" id="rt-max" hint="cap; empty = none">
                <Input
                  id="rt-max"
                  type="number"
                  min={1}
                  value={f.max_runs ?? ''}
                  onChange={(e) => set('max_runs', e.target.value ? Math.max(1, Math.floor(Number(e.target.value))) : null)}
                />
              </Field>
              {info && (f.until !== null || f.max_runs !== null) && <p className="text-[13px] text-fg-tertiary sm:col-span-2">runs so far: {runCount(info)}</p>}
            </fieldset>
          )}
        </>
      )}
      {(isNew || section === 'prompt') && (
        <Field label="Prompt" id="rt-prompt" hint="markdown; what the run does, written for nobody being there">
          <Textarea id="rt-prompt" rows={isNew ? 8 : 18} className="font-mono text-[13px]" value={f.prompt} onChange={(e) => set('prompt', e.target.value)} />
        </Field>
      )}
      <div className="flex justify-end gap-2">
        {!isNew && (
          <Button type="button" variant="ghost" size="sm" disabled={!dirty || save.isPending} onClick={() => setF(initial)}>
            Revert
          </Button>
        )}
        <Button type="submit" size="sm" disabled={!dirty || save.isPending || !f.prompt.trim() || (isNew && !f.name.trim())}>
          {isNew ? 'Create' : 'Save'}
        </Button>
      </div>
    </form>
  )
}

/** The runs of one routine, newest first: fired · took · result · report; a row opens its session. */
function History({ name }: { name: string }) {
  const runs = useRoutineHistory(name)
  const [shown, setShown] = useState<number | null>(null)
  if (runs.isPending) return <p className="text-sm text-fg-tertiary">Loading…</p>
  if (runs.data?.length === 0) return <Empty>No run yet.</Empty>
  return (
    <table className="w-full text-xs">
      <thead>
        <tr className="text-left text-fg-tertiary">
          <th className="px-2 py-1 font-medium">fired</th>
          <th className="px-2 py-1 font-medium">took</th>
          <th className="px-2 py-1 font-medium">result</th>
          <th className="px-2 py-1 font-medium">report</th>
          <th className="px-2 py-1 font-medium" />
        </tr>
      </thead>
      <tbody>
        {runs.data?.map((r: RunInfo, i) => (
          <Fragment key={`${r.fired_at}${r.session ?? ''}`}>
            <tr className="border-t border-border-subtle">
              <td className="px-2 py-1.5 tabular-nums" title={r.fired_at}>
                {new Date(r.fired_at).toLocaleString()}
              </td>
              <td className="px-2 py-1.5 tabular-nums">{r.finished_at ? took(r.fired_at, r.finished_at) : <Pulse className="bg-success" />}</td>
              <td className={cn('px-2 py-1.5 font-mono', resultKind(r.result) === 'failed' && 'text-danger')}>{r.result ?? 'running'}</td>
              <td className="px-2 py-1.5 text-fg-secondary">
                {r.report !== null && (
                  <button type="button" className="text-left" title={r.report} aria-expanded={shown === i} onClick={() => setShown((v) => (v === i ? null : i))}>
                    {r.report.split('\n', 1)[0]?.slice(0, 80)}
                  </button>
                )}
              </td>
              <td className="px-2 py-1.5 text-right">
                {r.session && (
                  <Link to="/c/$session" params={{ session: r.session }} className="underline underline-offset-2">
                    session
                  </Link>
                )}
              </td>
            </tr>
            {shown === i && r.report !== null && (
              <tr>
                <td colSpan={5} className="px-2 pb-2">
                  <pre className="whitespace-pre-wrap font-mono text-xs text-fg-secondary">{r.report}</pre>
                </td>
              </tr>
            )}
          </Fragment>
        ))}
      </tbody>
    </table>
  )
}

function Danger({ name }: { name: string }) {
  const api = useApi()
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
