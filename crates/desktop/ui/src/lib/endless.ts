import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'

/** 137 R1-D4: the first view line of a capped backfill; the Memory panel renders it as a note, not a band line. */
export const EARLIER_HISTORY = '(earlier history, not summarized; recall searches it)'

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
