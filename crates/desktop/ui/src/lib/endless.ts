import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import type { EndlessConfig } from '@/rpc/bindings/EndlessConfig'

/** 137F/137G keys optional on the wire: an engine before them omits them and its defaults (`ENDLESS_DEFAULTS`) apply. */
export type EndlessAdvanced = {
  attempt_secs: number
  local_attempt_secs: number
  local_ctx_bytes: number
  local_jobs: number
  raw_bytes: number
  compactor_ctx_bytes: number
  batch_units: number
  max_usd_per_call: number
  max_usd_per_session_day: number
}
export type EndlessConfigX = EndlessConfig & Partial<EndlessAdvanced>

/** The engine's defaults for the optional keys (and `jobs`), shown as placeholders and used while a key is absent. */
export const ENDLESS_DEFAULTS: EndlessAdvanced & { jobs: number } = {
  attempt_secs: 120,
  local_attempt_secs: 600,
  local_ctx_bytes: 24_000,
  local_jobs: 1,
  raw_bytes: 32_000,
  compactor_ctx_bytes: 48_000,
  batch_units: 16,
  max_usd_per_call: 0.1,
  max_usd_per_session_day: 5,
  jobs: 8,
}

/** 137 R1-D4: the first view line of a capped backfill; the Memory panel renders it as a note, not a band line. */
export const EARLIER_HISTORY = '(earlier history, not summarized; recall searches it)'

/** 137G: the view's one line for the waiting lines it folds, `(4,512 lines not yet summarized — recall searches them)`. No marker in the bindings: matched on the text. */
export const isFoldedNote = (text: string): boolean => /^\([\d,]+ lines? not yet summarized\b/.test(text)

/**
 * 137 R1-D3, the compactor chain as the UI names it: `agent.endless.model` when set, else
 * "Same as title model · <agent.title_model>" when one is set, else "Same as the session".
 */
export function useCompactorLabel(model: string | null | undefined): string {
  const api = useApi()
  const title = useQuery({ queryKey: queryKeys.titleModel, queryFn: () => api.title_model(), enabled: !model })
  if (model) return model
  return title.data ? `Same as title model · ${title.data.id}` : 'Same as the session'
}

/** `≈ 21 k tokens` for 64,000 bytes: content runs at ~3 bytes per token (137 §2). */
export const tokenEstimate = (bytes: number): string => `≈ ${Math.round(bytes / 3 / 1000)} k tokens`
