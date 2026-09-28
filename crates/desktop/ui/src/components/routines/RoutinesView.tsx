import { Link, useNavigate } from '@tanstack/react-router'
import { FlaskConical, PanelLeft, Play, Plus } from 'lucide-react'
import { toast } from 'sonner'
import { Pulse } from '@/components/chat/Pulse'
import { Empty, useAction } from '@/components/settings/shared'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import { type Result, cadence, lastCheckText, relative, resultKind, runCount, sortRoutines, verdictText } from './helpers'
import { KEY, useNotHub, useRoutines } from './queries'

const RESULT_CLASS: Record<Result, string> = {
  ok: 'border-success/40 text-success',
  failed: 'border-danger/40 text-danger',
  skipped: 'border-warning/40 text-warning',
}

/** `/routines`: every schedule file as a row (paperclip's RoutineList shape) — open a row, run it, pause it, or start a new one. */
export function RoutinesView() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const routines = useRoutines()
  const list = routines.data && sortRoutines(routines.data)
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
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Routines</h2>
        <Button asChild size="sm">
          <Link to="/routines/new">
            <Plus /> New routine
          </Link>
        </Button>
      </header>
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
            <Empty>
              No routine yet. Create one, ask the model to <code>schedule</code> one, or drop a markdown file in ~/.theuth/schedules/.
            </Empty>
          )}
          {list && list.length > 0 && (
            <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
              {list.map((s) => (
                <RoutineRow key={s.name} s={s} />
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
          {result}
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

function RoutineRow({ s }: { s: ScheduleInfo }) {
  const api = useApi()
  const navigate = useNavigate()
  const notHub = useNotHub()
  const fire = useAction((name: string) => api.fire_schedule(name), {
    keys: [KEY],
    onSuccess: (session) => void navigate({ to: '/c/$session', params: { session } }),
  })
  const enable = useAction((enabled: boolean) => api.enable_schedule(s.name, enabled), { keys: [KEY] })
  // 40 §6: the §1.12 verdict on demand; refusals (`not a watcher`, a broken dry run) toast as errors.
  const test = useAction((name: string) => api.test_schedule(name), {
    keys: [KEY],
    onSuccess: (v) => toast.message(`Test ${s.name}`, { description: verdictText(v) }),
  })
  const loop = s.until !== null || s.max_runs !== null
  return (
    <li className={cn('flex items-center gap-3 px-3 py-2.5 hover:bg-layer-1', !s.enabled && 'text-fg-tertiary')}>
      <Link to="/routines/$name/$section" params={{ name: s.name, section: 'overview' }} className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm">
          <span className="truncate font-medium">{s.name}</span>
          {s.error !== null ? (
            <span className="text-xs text-danger">broken</span>
          ) : !s.enabled ? (
            <span className="text-xs text-fg-tertiary">paused</span>
          ) : null}
          {loop && (
            <span className="text-xs text-fg-tertiary" title={s.until ?? undefined}>
              loop · runs {runCount(s)}
            </span>
          )}
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
          {s.error !== null ? <span className="text-danger">{s.error}</span> : <span title={s.when}>{cadence(s)}</span>}
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
              check {lastCheckText(s.last_check)}
            </span>
          )}
          <RoutineChips s={s} notHub={notHub} />
        </div>
      </Link>
      <Button variant="outline" size="xs" disabled={fire.isPending || s.error !== null || s.running} onClick={() => fire.mutate(s.name)}>
        <Play /> Run now
      </Button>
      {s.when_sensor !== null && (
        <Button variant="outline" size="xs" disabled={test.isPending || s.error !== null} onClick={() => test.mutate(s.name)}>
          <FlaskConical /> Test
        </Button>
      )}
      <Switch
        size="sm"
        checked={s.enabled}
        disabled={enable.isPending || s.error !== null}
        aria-label={`${s.enabled ? 'Pause' : 'Resume'} ${s.name}`}
        onCheckedChange={(v) => enable.mutate(v)}
      />
    </li>
  )
}
