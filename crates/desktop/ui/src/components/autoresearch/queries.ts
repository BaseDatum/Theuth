import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useAction } from '@/components/settings/shared'
import { useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'
import { autoresearchFixture } from './fixtures'
import type { AutoresearchEvent, ControlOp, IdeaChange } from './types'

/** The connected `Api`, or in dev with `VITE_AUTORESEARCH_FIXTURE=studies|empty` the fixture's answers for the autoresearch methods. */
export function useAutoresearchApi() {
  return autoresearchFixture(useApi())
}

/** `['autoresearch']` prefixes every autoresearch query; `[..., slug]` one study's detail, experiments and experiment details. */
export const KEY = ['autoresearch'] as const
export const PAGE = 200

/** §12.3 events; `StudyChanged` / `ExperimentFinished` / `StudyFinished` invalidate the study; `ExperimentProgress` only its detail (phase, tail). */
const EVENTS = ['StudyChanged', 'ExperimentStarted', 'ExperimentProgress', 'ExperimentFinished', 'StudyFinished'] as const satisfies readonly AutoresearchEvent['type'][]

// ponytail: `on` is typed over `WireEvent`, which gains these variants with the generated bindings; until then the
// subscription goes through an untyped seam. Replace with plain `on(t, cb)` at the bindings swap.
const subscribe = on as unknown as (type: string, cb: (e: AutoresearchEvent) => void) => () => void

/** Subscribe once per mounted view: the list refetches on state changes, a study on every event that names it. */
export function useAutoresearchEvents() {
  const qc = useQueryClient()
  useEffect(() => {
    const offs = EVENTS.map((t) =>
      subscribe(t, (e) => {
        if (e.type === 'ExperimentProgress') void qc.invalidateQueries({ queryKey: [...KEY, e.slug, 'detail'] })
        else {
          void qc.invalidateQueries({ queryKey: [...KEY, 'list'] })
          void qc.invalidateQueries({ queryKey: [...KEY, e.slug] })
        }
      }),
    )
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
}

export function useStudies() {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, 'list'], queryFn: () => api['autoresearch.list']() })
}

export function useStudy(slug: string) {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, slug, 'detail'], queryFn: () => api['autoresearch.get'](slug) })
}

export function useExperiments(slug: string) {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, slug, 'experiments'], queryFn: () => api['autoresearch.experiments'](slug, null, PAGE) })
}

export function useExperiment(slug: string, n: number | null) {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, slug, 'experiment', n], queryFn: () => api['autoresearch.experiment'](slug, n ?? 0), enabled: n !== null })
}

export function useAutoresearchSettings() {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, 'settings'], queryFn: () => api['autoresearch.settings']() })
}

/** `autoresearch.control`: every button of the header, the attention rows and the changesets table. */
export function useControl(slug: string) {
  const api = useAutoresearchApi()
  return useAction((op: ControlOp) => api['autoresearch.control'](slug, op), { keys: [KEY] })
}

export function useSteer(slug: string, onSuccess?: () => void) {
  const api = useAutoresearchApi()
  return useAction((text: string) => api['autoresearch.steer'](slug, text), { keys: [[...KEY, slug]], onSuccess })
}

export function useIdea(slug: string) {
  const api = useAutoresearchApi()
  return useAction((op: IdeaChange) => api['autoresearch.idea'](slug, op), { keys: [[...KEY, slug]] })
}
