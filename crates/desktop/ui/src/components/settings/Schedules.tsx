import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronRight, Play } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
import { Pulse } from '@/components/chat/Pulse'
import { Empty, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { useApi } from '@/lib/rpc'
import type { RunInfo } from '@/rpc/bindings/RunInfo'
import type { ScheduleEvent } from '@/rpc/bindings/ScheduleEvent'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import { on } from '@/store/sessions'

/** `['schedules']` is the table; `['schedules', 'history', name]` one row's runs, so one prefix invalidates both. */
const KEY = ['schedules'] as const
const HISTORY_LIMIT = 20

/** Every schedule bus event; any of them means the table is stale (34 §8). */
const SCHEDULE_EVENTS = ['Fired', 'Skipped', 'Changed'] as const satisfies readonly ScheduleEvent['type'][]

/** `in 14h` / `3m ago` / `now`: future-aware, unlike the sidebar's past-only `ago` (34 §8). */
export function relative(iso: string, now = Date.now()): string {
  const s = (Date.parse(iso) - now) / 1000
  const a = Math.abs(s)
  if (a < 60) return 'now'
  const n = a < 3600 ? `${Math.floor(a / 60)}m` : a < 86400 ? `${Math.floor(a / 3600)}h` : `${Math.floor(a / 86400)}d`
  return s > 0 ? `in ${n}` : `${n} ago`
}

/** `4s` / `2m` / `1h` between two instants. */
function took(from: string, to: string): string {
  const s = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000))
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`
}

const TH = 'px-2 py-1.5 font-semibold'
const TD = 'px-2 py-1.5 align-top'

/** 34 §8: Settings › Schedules — `list_schedules` as a table, Run → `fire_schedule`, a row expands to its runs. */
export function Schedules() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const schedules = useQuery({ queryKey: KEY, queryFn: () => api.list_schedules() })
  const [open, setOpen] = useState<string | null>(null)
  const openSession = (session: string) => void navigate({ to: '/c/$session', params: { session } })
  const fire = useAction((name: string) => api.fire_schedule(name), { keys: [KEY], onSuccess: openSession })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = SCHEDULE_EVENTS.map((t) => on(t, invalidate))
    offs.push(
      on('TurnFinished', (e) => {
        if (qc.getQueryData<ScheduleInfo[]>(KEY)?.some((s) => s.session === e.session)) invalidate()
      }),
    )
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  const list = schedules.data
  return (
    <Section title="Schedules">
      {list?.length === 0 && <Empty>No schedule yet. Ask the model to `schedule` one, or drop a markdown file in ~/.wth/schedules/.</Empty>}
      {list && list.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border-subtle text-left text-fg-secondary">
                <th className={TH} />
                <th className={TH}>name</th>
                <th className={TH}>when</th>
                <th className={TH}>next</th>
                <th className={TH}>last</th>
                <th className={TH}>result</th>
                <th className={TH}>state</th>
                <th className={TH} />
              </tr>
            </thead>
            <tbody>
              {list.map((s) => (
                <Fragment key={s.name}>
                  <ScheduleRow
                    s={s}
                    open={open === s.name}
                    firing={fire.isPending && fire.variables === s.name}
                    onToggle={() => setOpen((v) => (v === s.name ? null : s.name))}
                    onOpen={() => s.session && openSession(s.session)}
                    onRun={() => fire.mutate(s.name)}
                  />
                  {open === s.name && (
                    <tr className="border-b border-border-subtle last:border-0">
                      <td />
                      <td colSpan={7} className="px-2 pb-3">
                        <History name={s.name} onOpen={openSession} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="pt-4 text-[13px] text-fg-tertiary">
        Schedules are markdown files in ~/.wth/schedules/ — edit or delete them there, or ask the model to `schedule`.
      </p>
    </Section>
  )
}

function ScheduleRow({
  s,
  open,
  firing,
  onToggle,
  onOpen,
  onRun,
}: {
  s: ScheduleInfo
  open: boolean
  firing: boolean
  onToggle: () => void
  onOpen: () => void
  onRun: () => void
}) {
  const link = s.session !== null
  return (
    <tr
      className={`border-b border-border-subtle last:border-0 ${link ? 'cursor-pointer hover:bg-layer-1' : ''} ${s.enabled ? '' : 'text-fg-tertiary'}`}
      onClick={onOpen}
    >
      <td className={TD}>
        <button
          type="button"
          className="text-fg-secondary"
          aria-expanded={open}
          aria-label={`${open ? 'Hide' : 'Show'} runs of ${s.name}`}
          onClick={(e) => {
            e.stopPropagation()
            onToggle()
          }}
        >
          <ChevronRight className={`size-4 transition-transform ${open ? 'rotate-90' : ''}`} />
        </button>
      </td>
      <td className={`${TD} font-medium`}>{s.name}</td>
      <td className={`${TD} text-fg-secondary`} title={s.when}>
        {s.describe}
      </td>
      <td className={TD}>
        {s.error !== null ? (
          <span className="text-danger">{s.error}</span>
        ) : s.next ? (
          <span title={new Date(s.next).toLocaleString()} className="tabular-nums">
            {relative(s.next)}
          </span>
        ) : (
          '—'
        )}
      </td>
      <td className={`${TD} tabular-nums`} title={s.last ? new Date(s.last).toLocaleString() : undefined}>
        {s.last ? relative(s.last) : '—'}
      </td>
      <td className={`${TD} font-mono text-xs`}>{s.last_result ?? '—'}</td>
      <td className={TD}>
        {s.running ? (
          <span className="flex items-center gap-1.5">
            <Pulse className="bg-emerald-500" /> running
          </span>
        ) : s.error !== null ? (
          <span className="text-danger">error</span>
        ) : s.enabled ? (
          ''
        ) : (
          'off'
        )}
      </td>
      <td className={`${TD} text-right`}>
        <Button
          variant="outline"
          size="xs"
          disabled={firing || s.error !== null}
          onClick={(e) => {
            e.stopPropagation()
            onRun()
          }}
        >
          <Play /> Run
        </Button>
      </td>
    </tr>
  )
}

/** The last runs of one schedule: fired · took · result; a row opens its session. */
function History({ name, onOpen }: { name: string; onOpen: (session: string) => void }) {
  const api = useApi()
  const runs = useQuery({ queryKey: ['schedules', 'history', name], queryFn: () => api.schedule_history(name, HISTORY_LIMIT) })
  if (runs.data?.length === 0) return <Empty>No run yet.</Empty>
  return (
    <table className="w-full text-xs">
      <thead>
        <tr className="text-left text-fg-tertiary">
          <th className="px-2 py-1 font-medium">fired</th>
          <th className="px-2 py-1 font-medium">took</th>
          <th className="px-2 py-1 font-medium">result</th>
        </tr>
      </thead>
      <tbody>
        {runs.data?.map((r: RunInfo) => (
          <tr
            key={`${r.fired_at}${r.session ?? ''}`}
            className={r.session ? 'cursor-pointer hover:bg-layer-1' : ''}
            onClick={() => r.session && onOpen(r.session)}
          >
            <td className="px-2 py-1 tabular-nums" title={r.fired_at}>
              {new Date(r.fired_at).toLocaleString()}
            </td>
            <td className="px-2 py-1 tabular-nums">{r.finished_at ? took(r.fired_at, r.finished_at) : <Pulse className="bg-emerald-500" />}</td>
            <td className="px-2 py-1 font-mono">{r.result ?? 'running'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
