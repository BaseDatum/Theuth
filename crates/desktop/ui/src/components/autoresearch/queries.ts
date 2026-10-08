import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useAction } from '@/components/settings/shared'
import { toast } from 'sonner'
import { useApi } from '@/lib/rpc'
import type { ControlOp } from '@/rpc/bindings/ControlOp'
import type { IdeaChange } from '@/rpc/bindings/IdeaChange'
import type { StudyEvent } from '@/rpc/bindings/StudyEvent'
import { on } from '@/store/sessions'
import { autoresearchFixture } from './fixtures'
import type { Progress } from './model'

/** The connected `Api`, or in dev with `VITE_AUTORESEARCH_FIXTURE=studies|empty` the fixture's answers for the autoresearch methods. */
export function useAutoresearchApi() {
  return autoresearchFixture(useApi())
}

/** `['autoresearch']` prefixes every autoresearch query; `[..., slug]` one study's detail, experiments, experiment details and live progress. */
export const KEY = ['autoresearch'] as const
export const PAGE = 200

/** §12.3 events: state changes refetch the list and the study; `ExperimentProgress` updates the live progress cell and refetches the detail (its `tail`). */
const EVENTS = ['StudyChanged', 'ExperimentStarted', 'ExperimentProgress', 'ExperimentFinished', 'StudyFinished'] as const satisfies readonly StudyEvent['type'][]

/** Subscribe once per mounted view. */
export function useAutoresearchEvents() {
  const qc = useQueryClient()
  useEffect(() => {
    const offs = EVENTS.map((t) =>
      on(t, (e) => {
        if (e.type === 'ExperimentProgress') {
          qc.setQueryData<Progress>([...KEY, e.slug, 'progress'], { phase: e.phase, sample: e.sample, of: e.of, elapsed_ms: e.elapsed_ms })
          void qc.invalidateQueries({ queryKey: [...KEY, e.slug, 'detail'] })
          return
        }
        if (e.type === 'ExperimentStarted' || e.type === 'ExperimentFinished') qc.removeQueries({ queryKey: [...KEY, e.slug, 'progress'] })
        void qc.invalidateQueries({ queryKey: [...KEY, 'list'] })
        void qc.invalidateQueries({ queryKey: [...KEY, e.slug] })
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

/** The last `ExperimentProgress` of the study since its experiment started; `undefined` before the first. Event-fed, never fetched. */
export function useProgress(slug: string): Progress | undefined {
  return useQuery<Progress>({ queryKey: [...KEY, slug, 'progress'], queryFn: () => Promise.reject(new Error('event-fed')), enabled: false, staleTime: Infinity }).data
}

export function useAutoresearchSettings() {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, 'settings'], queryFn: () => api['autoresearch.settings.get']() })
}

/** `autoresearch.control`: every button of the header, the needs-your-eyes rows and the changesets table. A merge has no row to change, so it says so. */
export function useControl(slug: string) {
  const api = useAutoresearchApi()
  return useAction((op: ControlOp) => api['autoresearch.control'](slug, op), {
    keys: [KEY],
    onSuccess: (_info, op) => {
      if (op.op === 'merge') toast.success(`Merged changeset #${op.changeset} into your branch`)
    },
  })
}

export function useSteer(slug: string, onSuccess?: () => void) {
  const api = useAutoresearchApi()
  return useAction((text: string) => api['autoresearch.steer'](slug, text), { keys: [[...KEY, slug]], onSuccess })
}

export function useIdea(slug: string) {
  const api = useAutoresearchApi()
  return useAction((op: IdeaChange) => api['autoresearch.idea'](slug, op), { keys: [[...KEY, slug]] })
}
