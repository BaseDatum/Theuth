import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo } from 'react'
import { useAction } from '@/components/settings/shared'
import { toast } from 'sonner'
import { useApi } from '@/lib/rpc'
import type { ControlOp } from '@/rpc/bindings/ControlOp'
import type { IdeaChange } from '@/rpc/bindings/IdeaChange'
import type { StartWhen } from '@/rpc/bindings/StartWhen'
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

/** §12.3 events: state changes refetch the list and the study; `ExperimentProgress` (re-published every ~3 s while a sample runs, R-87) updates the live progress cell and refetches the detail, whose `tail` is the newest sample log. */
const EVENTS = ['StudyChanged', 'ExperimentStarted', 'ExperimentProgress', 'ExperimentFinished', 'StudyFinished'] as const satisfies readonly StudyEvent['type'][]

/** Subscribe once: mounted by the Explorer (the Study view is its outlet, so it must not mount a second one). */
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

/** The detail (with the live `tail`) is refetched by `ExperimentProgress` while a sample runs; no polling. `enabled` false = the setup pane before Start. */
export function useStudy(slug: string, enabled = true) {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, slug, 'detail'], queryFn: () => api['autoresearch.get'](slug), enabled })
}

/** Every experiment, newest first, paged by `before` until a page comes back short: the table, the tally and the Restore lookup see the whole study. */
export function useExperiments(slug: string) {
  const api = useAutoresearchApi()
  const q = useInfiniteQuery({
    queryKey: [...KEY, slug, 'experiments'],
    queryFn: ({ pageParam }) => api['autoresearch.experiments'](slug, pageParam, PAGE),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => (last.length < PAGE ? null : (last[last.length - 1]?.n ?? null)),
  })
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = q
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage()
  }, [hasNextPage, isFetchingNextPage, fetchNextPage])
  const data = useMemo(() => q.data?.pages.flat(), [q.data])
  return { data, isPending: q.isPending, isError: q.isError }
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

// ── 151 setup (§4.7) ──────────────────────────────────────────────────────────────────────────────────────────────

/** The rail's setting-up rows (D6); invalidated by edit / confirm / discard / start. */
export function useDrafts() {
  const api = useAutoresearchApi()
  return useQuery({ queryKey: [...KEY, 'drafts'], queryFn: () => api['autoresearch.drafts']() })
}

/**
 * One draft's `DraftView` (D3). `resultId` = `lastSetupResultId(entries)`: every new `autoresearch_draft` /
 * `autoresearch_probe` result in the setup session refetches it — no new event type. The rail's `drafts` row
 * (`n of m settled`) changes with the same results, so it refetches too (live proof Z: it stayed at the last edit).
 */
export function useDraft(draftId: string | null, resultId: string | null) {
  const api = useAutoresearchApi()
  const qc = useQueryClient()
  useEffect(() => {
    if (draftId && resultId) for (const queryKey of draftKeys(draftId)) void qc.invalidateQueries({ queryKey })
  }, [qc, draftId, resultId])
  return useQuery({ queryKey: [...KEY, 'draft', draftId], queryFn: () => api['autoresearch.draft'](draftId ?? ''), enabled: draftId !== null })
}

const draftKeys = (draftId: string) => [[...KEY, 'draft', draftId], [...KEY, 'drafts']]

export function useDraftEdit(draftId: string, onSuccess?: (v: { field: string; value: string }) => void) {
  const api = useAutoresearchApi()
  return useAction((v: { field: string; value: string }) => api['autoresearch.draft_edit'](draftId, v.field, v.value), { keys: draftKeys(draftId), onSuccess: (_view, v) => onSuccess?.(v) })
}

export function useDraftConfirm(draftId: string) {
  const api = useAutoresearchApi()
  return useAction((field: string) => api['autoresearch.draft_confirm'](draftId, field), { keys: draftKeys(draftId) })
}

export function useDiscardDraft(onSuccess?: (draftId: string) => void) {
  const api = useAutoresearchApi()
  return useAction((draftId: string) => api['autoresearch.discard_draft'](draftId), { keys: [[...KEY, 'drafts'], [...KEY, 'draft']], onSuccess: (_r, id) => onSuccess?.(id) })
}

/** The panel's Start buttons: the existing `autoresearch.create {draft_id, start}`; the list, the drafts and the draft (`started`) all change. */
export function useStartDraft(draftId: string) {
  const api = useAutoresearchApi()
  return useAction((start: StartWhen) => api['autoresearch.create'](draftId, start), { keys: [KEY] })
}
