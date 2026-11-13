import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { dreamingFixture } from '@/dev/dreaming-fixture'
import { useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'

/** The connected `Api`, or in dev with `VITE_DREAMING_FIXTURE=<state>` the fixture's answers for the dreaming methods. */
export function useDreamingApi() {
  return dreamingFixture(useApi())
}

/** `['dreaming']` is the prefix of every dreaming query: one invalidation after a night, an undo, a settings write. */
export const KEY = ['dreaming'] as const
export const NIGHTS_LIMIT = 10

/** `dreaming.status`, refetched when a night starts, finishes or is undone. Reads `state.json` — cheap. */
export function useDreamStatus() {
  const canQuery1 = useQuerySupport(["dreaming.status"])
  const api = useDreamingApi()
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = [on("dreaming.started@1", 'DreamNightStarted', invalidate), on("dreaming.finished@1", 'DreamNightFinished', invalidate), on("dreaming.undone@1", 'DreamUndone', invalidate)]
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return useQuery({ enabled: canQuery1, queryKey: [...KEY, 'status'], queryFn: () => api['dreaming.status']() })
}

export function useDreamSettings() {
  const canQuery2 = useQuerySupport(["dreaming.settings"])
  const api = useDreamingApi()
  return useQuery({ enabled: canQuery2, queryKey: [...KEY, 'settings'], queryFn: () => api['dreaming.settings']() })
}

export function useDreamNights() {
  const canQuery3 = useQuerySupport(["dreaming.nights"])
  const api = useDreamingApi()
  return useQuery({ enabled: canQuery3, queryKey: [...KEY, 'nights'], queryFn: () => api['dreaming.nights'](NIGHTS_LIMIT) })
}

export function useDreamProposals() {
  const canQuery4 = useQuerySupport(["dreaming.proposals"])
  const api = useDreamingApi()
  return useQuery({ enabled: canQuery4, queryKey: [...KEY, 'proposals'], queryFn: () => api['dreaming.proposals']() })
}
