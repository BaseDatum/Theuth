import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useMesh } from '@/components/tasks/queries'
import { routinesFixture } from '@/dev/routines-fixture'
import { useApi } from '@/lib/rpc'
import type { ScheduleEvent } from "@/rpc/bindings/event.schedule_2eevents.output"
import type { ScheduleInfo } from "@/rpc/bindings/method.schedule.output"
import { on } from '@/store/sessions'

/** The connected `Api`, or in dev with `VITE_ROUTINES_FIXTURE=1` the fixture's answers for the routine methods (108 §11). */
export function useRoutinesApi() {
  return routinesFixture(useApi())
}

/** `['schedules']` is the table; `['schedules', 'file', name]` one editor, `['schedules', 'history', name]` one row's runs: one prefix invalidates all. */
export const KEY = ['schedules'] as const
export const HISTORY_LIMIT = 50

/** Every schedule bus event; any of them means the table is stale (34 §8). */
const SCHEDULE_EVENTS = ['Fired', 'Skipped', 'Changed'] as const satisfies readonly ScheduleEvent['type'][]

/** `list_schedules`, refetched on every schedule event and on the end of a routine's turn. */
export function useRoutines() {
  const canQuery1 = useQuerySupport(["list_schedules"])
  const api = useRoutinesApi()
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = SCHEDULE_EVENTS.map((t) => on("schedule.events@1", t, invalidate))
    offs.push(
      on("agent.events@1", 'TurnFinished', (e) => {
        if (qc.getQueryData<ScheduleInfo[]>(KEY)?.some((s) => s.session === e.session)) invalidate()
      }),
    )
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return useQuery({ enabled: canQuery1, queryKey: KEY, queryFn: () => api.list_schedules() })
}

export function useRoutineFile(name: string | null) {
  const canQuery2 = useQuerySupport(["get_schedule"])
  const api = useRoutinesApi()
  return useQuery({ queryKey: [...KEY, 'file', name], queryFn: () => api.get_schedule(name ?? ''), enabled: canQuery2 && (name !== null) })
}

export function useRoutineHistory(name: string) {
  const canQuery3 = useQuerySupport(["schedule_history"])
  const api = useRoutinesApi()
  return useQuery({ enabled: canQuery3, queryKey: [...KEY, 'history', name], queryFn: () => api.schedule_history(name, HISTORY_LIMIT) })
}

/** `list_sensors` (108 §9.2): the form's Sensor picker and the sentence's subject; sensors change on disk, rarely. */
export function useSensors() {
  const canQuery4 = useQuerySupport(["list_sensors"])
  const api = useRoutinesApi()
  return useQuery({ enabled: canQuery4, queryKey: ['sensors'], queryFn: () => api.list_sensors(), staleTime: 60_000 })
}

/**
 * Whether a schedule fires on this node (182 D6): one stamped `engine:` fires only there; an unstamped one ticks on
 * the hub (67 §4.4). `true` = shown but never fired here.
 */
export function useNotHub(): (s: { engine?: string | null }) => boolean {
  const m = useMesh().data
  return (s) => (s.engine ? !!m && s.engine !== m.self_id : !!m && m.lease.holder !== null && m.lease.holder !== m.self_id)
}
