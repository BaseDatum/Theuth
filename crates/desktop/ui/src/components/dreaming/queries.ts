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
  const api = useDreamingApi()
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: KEY })
    const offs = [on('DreamNightStarted', invalidate), on('DreamNightFinished', invalidate), on('DreamUndone', invalidate)]
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return useQuery({ queryKey: [...KEY, 'status'], queryFn: () => api['dreaming.status']() })
}

export function useDreamSettings() {
  const api = useDreamingApi()
  return useQuery({ queryKey: [...KEY, 'settings'], queryFn: () => api['dreaming.settings']() })
}

export function useDreamNights() {
  const api = useDreamingApi()
  return useQuery({ queryKey: [...KEY, 'nights'], queryFn: () => api['dreaming.nights'](NIGHTS_LIMIT) })
}

export function useDreamProposals() {
  const api = useDreamingApi()
  return useQuery({ queryKey: [...KEY, 'proposals'], queryFn: () => api['dreaming.proposals']() })
}
