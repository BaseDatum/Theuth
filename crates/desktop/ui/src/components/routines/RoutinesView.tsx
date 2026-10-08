import { Link, useNavigate } from '@tanstack/react-router'
import { FlaskConical, MessageSquarePlus, Play, Plus } from 'lucide-react'
import { toast } from 'sonner'
import { Pulse } from '@/components/chat/Pulse'
import { useDreamStatus } from '@/components/dreaming/queries'
import { KEY as AUTORESEARCH_KEY, useAutoresearchApi, useStudies } from '@/components/autoresearch/queries'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { nightLine } from '@/lib/dreaming'
import { cn } from '@/lib/utils'
import type { ControlOp } from '@/rpc/bindings/ControlOp'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SensorInfo } from '@/rpc/bindings/SensorInfo'
import { type Kind, type Result, fromInfo, kindOf, lastCheckText, relative, resultKind, resultLabel, runCount, sentence, sortRoutines, verdictText } from './helpers'
import { KEY, useNotHub, useRoutines, useRoutinesApi, useSensors } from './queries'

export const RESULT_CLASS: Record<Result, string> = {
  ok: 'border-success/40 text-success',
  failed: 'border-danger/40 text-danger',
  skipped: 'border-warning/40 text-warning',
  check: 'border-border text-fg-tertiary',
}

const KIND_CLASS: Record<Kind, string> = {
  Clock: 'bg-layer-2 text-fg-secondary',
  Once: 'bg-layer-2 text-fg-secondary',
  Watcher: 'bg-info/15 text-info',
  Loop: 'bg-warning/15 text-warning',
}

/** `/routines`: every schedule file as a row — badge · name · sentence · health; open a row, run it, pause it, or describe a new one. */
export function RoutinesView() {
  const routines = useRoutines()
  const sensors = useSensors().data
  const list = routines.data && sortRoutines(routines.data)
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      <SectionRow>
        <Button asChild size="sm" className="ml-auto">
          <Link to="/routines/new" search={{ chat: '1' }}>
            <MessageSquarePlus /> Describe it
          </Link>
        </Button>
        <Button asChild size="sm" variant="outline">
          <Link to="/routines/new" search={{}}>
            <Plus /> New routine
          </Link>
        </Button>
      </SectionRow>
      <main className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[880px] p-4">
          {routines.isPending && (
            <div className="space-y-2">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          )}
          {routines.isError && <p className="text-sm text-fg-tertiary">{routines.error.message}</p>}
          {list?.length === 0 && (
            <div className="flex flex-col items-start gap-3 py-6">
              <p className="text-sm text-fg-secondary">A routine is a prompt that runs on a clock, when a sensor says so, or again until done.</p>
              <div className="flex gap-2">
                <Button asChild size="sm">
                  <Link to="/routines/new" search={{ chat: '1' }}>
                    <MessageSquarePlus /> Describe it
                  </Link>
                </Button>
                <Button asChild size="sm" variant="outline">
                  <Link to="/routines/new" search={{}}>
                    Use the form
                  </Link>
                </Button>
              </div>
            </div>
          )}
          {list && list.length > 0 && (
            <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
              {list.map((s) => (
                <RoutineRow key={s.name} s={s} sensors={sensors} />
              ))}
            </ul>
          )}
          <p className="pt-4 text-[13px] text-fg-tertiary">
            A routine is a markdown file in ~/.theuth/schedules/: front matter says when, the body is the prompt. With <code>until</code> or{' '}
            <code>max_runs</code> it is a loop that runs again until done.
          </p>
        </div>
      </main>
    </article>
  )
}

/** D6 badge: what wakes it. */
export function KindBadge({ kind }: { kind: Kind }) {
  return <span className={cn('rounded px-1.5 py-px text-[11px] font-medium', KIND_CLASS[kind])}>{kind}</span>
}

/** The stop chip (§3 line 1): `until` · `runs 3/10` · `expires 14d`; nothing for a Loop (its trigger is the stop) or no stop. */
export function StopChip({ s }: { s: ScheduleInfo }) {
  if (kindOf(fromInfo(s)) === 'Loop') return null
  const parts: { text: string; title?: string }[] = []
  if (s.until !== null) parts.push({ text: 'until', title: s.until })
  if (s.max_runs !== null) parts.push({ text: `runs ${runCount(s)}` })
  if (s.expires !== null) parts.push({ text: `expires ${s.expires}` })
  if (parts.length === 0) return null
  return (
    <span className="text-xs text-fg-tertiary">
      {parts.map((p, i) => (
        <span key={p.text} title={p.title}>
          {i > 0 && ' · '}
          {p.text}
        </span>
      ))}
    </span>
  )
}

/** Chips shared by the list row and the detail header: last result, running, not-hub, agent, project, approvals off. */
export function RoutineChips({ s, notHub }: { s: ScheduleInfo; notHub: boolean }) {
  const result = resultKind(s.last_result)
  return (
    <>
      {s.running && (
        <span className="flex items-center gap-1.5 text-xs text-success">
          <Pulse className="bg-success" /> running
        </span>
      )}
      {result && (
        <Badge variant="outline" className={cn('font-normal', RESULT_CLASS[result])} title={s.last_result ?? undefined}>
          {resultLabel(s.last_result)}
        </Badge>
      )}
      {notHub && s.enabled && (
        <Badge variant="outline" className="font-normal text-fg-tertiary" title="Ticks are the hub's duty; this node does not fire routines">
          not hub
        </Badge>
      )}
      {s.agent && (
        <Badge variant="secondary" className="font-normal">
          @{s.agent}
        </Badge>
      )}
      {s.project && (
        <Badge variant="secondary" className="font-normal">
          {s.project}
        </Badge>
      )}
      {!s.approvals && (
        <Badge variant="outline" className="font-normal">
          approvals off
        </Badge>
      )}
    </>
  )
}

/** The `# arg:` header of the routine's sensor when `list_sensors` knows it (the sentence then names required args only). */
export const sensorArgs = (s: Pick<ScheduleInfo, 'sensor' | 'when_sensor'>, sensors: SensorInfo[] | undefined) =>
  sensors?.find((x) => x.name === (s.sensor ?? s.when_sensor?.split(' ')[0]))?.args

function RoutineRow({ s, sensors }: { s: ScheduleInfo; sensors: SensorInfo[] | undefined }) {
  const api = useRoutinesApi()
  const autoresearch = useAutoresearchApi()
  const navigate = useNavigate()
  const notHub = useNotHub()
  // 140 §13.1 / R1-D4: an `autoresearch-<slug>` schedule is a study — `study` badge, opens the Study view, says its headline.
  const study = s.name.startsWith('autoresearch-') ? s.name.slice('autoresearch-'.length) : null
  const fire = useAction((name: string) => api.fire_schedule(name), {
    keys: [KEY],
    onSuccess: (session) => void navigate({ to: '/c/$session', params: { session } }),
  })
  const enable = useAction((enabled: boolean) => api.enable_schedule(s.name, enabled), { keys: [KEY] })
  // R-86: a study's Run now / switch go through the plugin (`run_once`, `pause`, `resume`), which flips the file's
  // `enabled:` *and* the ledger (§8.6: the toggle is the same line); a bare `enable_schedule` would desync the state pill.
  const control = useAction((op: ControlOp) => autoresearch['autoresearch.control'](study ?? '', op), {
    keys: [KEY, AUTORESEARCH_KEY],
    onSuccess: (_info, op) => {
      if (op.op === 'run_once' && study !== null) void navigate({ to: '/autoresearch/$slug', params: { slug: study }, search: {} })
    },
  })
  // 40 §6: the §1.12 verdict on demand; refusals (`not a watcher`, a broken dry run) toast as errors.
  const test = useAction((name: string) => api.test_schedule(name), {
    keys: [KEY],
    onSuccess: (v) => toast.message(`Test ${s.name}`, { description: verdictText(v) }),
  })
  const r = fromInfo(s)
  const title = [s.when, s.when_sensor].filter(Boolean).join(' · ')
  const body = (
    <>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {study !== null ? <span className="rounded bg-info/15 px-1.5 py-px text-[11px] font-medium text-info">study</span> : <KindBadge kind={kindOf(r)} />}
        <span className="truncate font-medium">{s.name}</span>
        {s.error !== null ? (
          <span className="text-xs text-danger">broken</span>
        ) : !s.enabled ? (
          <span className="text-xs text-fg-tertiary">paused</span>
        ) : null}
        <StopChip s={s} />
      </div>
      <div className="mt-0.5 truncate text-[13px] text-fg-secondary" title={title}>
        {s.error !== null ? <span className="text-danger">{s.error}</span> : s.name === 'dreaming' ? <DreamingSentence fallback={sentence(r, sensorArgs(s, sensors))} /> : study !== null ? <StudySentence slug={study} fallback={sentence(r, sensorArgs(s, sensors))} /> : sentence(r, sensorArgs(s, sensors))}
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
        {s.next && (
          <span className="tabular-nums" title={new Date(s.next).toLocaleString()}>
            next {relative(s.next)}
          </span>
        )}
        {s.last && (
          <span className="tabular-nums" title={new Date(s.last).toLocaleString()}>
            last {relative(s.last)}
          </span>
        )}
        {s.last_check && (
          <span className="tabular-nums" title={s.last_check.hash ?? undefined}>
            last check {lastCheckText(s.last_check)}
          </span>
        )}
        <RoutineChips s={s} notHub={notHub} />
      </div>
    </>
  )
  return (
    <li className={cn('flex flex-col gap-2 px-3 py-2.5 hover:bg-layer-1 md:flex-row md:items-center md:gap-3', !s.enabled && 'text-fg-tertiary')}>
      {study !== null ? (
        <Link to="/autoresearch/$slug" params={{ slug: study }} search={{}} className="min-w-0 flex-1">
          {body}
        </Link>
      ) : (
        <Link to="/routines/$name/$section" params={{ name: s.name, section: 'overview' }} search={{}} className="min-w-0 flex-1">
          {body}
        </Link>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" size="xs" disabled={fire.isPending || control.isPending || s.error !== null || s.running} onClick={() => (study !== null ? control.mutate({ op: 'run_once' }) : fire.mutate(s.name))}>
          <Play /> Run now
        </Button>
        {s.when_sensor !== null && (
          <Button variant="outline" size="xs" className="hidden md:inline-flex" disabled={test.isPending || s.error !== null} onClick={() => test.mutate(s.name)}>
            <FlaskConical /> Test
          </Button>
        )}
        <Switch
          size="sm"
          checked={s.enabled}
          disabled={enable.isPending || control.isPending || s.error !== null}
          aria-label={`${s.enabled ? 'Pause' : 'Resume'} ${s.name}`}
          onCheckedChange={(v) => (study !== null ? control.mutate({ op: v ? 'resume' : 'pause' }) : enable.mutate(v))}
        />
      </div>
    </li>
  )
}

/** 140 §13.1: a study's row says its headline (`autoresearch.list`) instead of the loop's trigger sentence. */
function StudySentence({ slug, fallback }: { slug: string; fallback: string }) {
  const studies = useStudies()
  const line = studies.data?.find((s) => s.slug === slug)?.headline
  if (!line) return <>{fallback}</>
  return <span title={line}>{line}</span>
}

/** 136 §8: the `dreaming` row says what last night did (`dreaming.status`) instead of its trigger sentence. */
function DreamingSentence({ fallback }: { fallback: string }) {
  const status = useDreamStatus()
  if (!status.data) return <>{fallback}</>
  const line = `runs at night when the Mac is idle; ${nightLine(status.data.last_night)}`
  return <span title={line}>{line}</span>
}
