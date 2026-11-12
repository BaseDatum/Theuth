import { useQuerySupport } from '@/rpc/capabilities'
// 125 §8.1: the advisor's live status and the one RPC that changes it, shared by the header row and the pill.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { queryKeys, useApi } from '@/lib/rpc'
import type { AdvisorStatus } from "@/rpc/bindings/method.agent.output"

/**
 * `advisor_status(session)`: read once, then kept current by `AdvisorChanged` (`useInvalidateOn` writes the event's
 * status into this query). `undefined` while loading, for the draft (no session) and on an engine without the method.
 */
export function useAdvisor(session: string | null): AdvisorStatus | undefined {
  const canQuery1 = useQuerySupport(["advisor_status"])
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.advisorStatus(session ?? ''),
    queryFn: () => api.advisor_status(session ?? ''),
    enabled: canQuery1 && (!!session),
    retry: false,
  }).data
}

/** `set_advisor` (125 §6.3): a model turns the advisor on or re-picks it, `null` turns it off. The reply is the new status. */
export function useSetAdvisor(session: string) {
  const api = useApi()
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ model, effort }: { model: string | null; effort: string | null }) => api.set_advisor(session, model, effort),
    onSuccess: (status) => qc.setQueryData(queryKeys.advisorStatus(session), status),
    onError: (e) => toast.error(e instanceof Error ? e.message : String(e)),
  })
}
