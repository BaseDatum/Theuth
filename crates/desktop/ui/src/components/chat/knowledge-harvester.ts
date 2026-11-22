import { useQuerySupport } from '@/rpc/capabilities'
// The knowledge harvester (the model of the knowledge harvest): the chat's own pick, the default it falls back to, and the setter.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { routed } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ModelInfo, SessionKnowledgeHarvester } from '@/rpc/bindings/method.agent.output'

/** `session_knowledge_harvester(session)`: `model: null` = the default. `undefined` while loading, for the draft and on an engine without the method. */
export function useKnowledgeHarvester(session: string | null): SessionKnowledgeHarvester | undefined {
  const supported = useQuerySupport(['session_knowledge_harvester'])
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.sessionKnowledgeHarvester(session ?? ''),
    queryFn: () => api.session_knowledge_harvester(session ?? ''),
    enabled: supported && !!session,
    retry: false,
  }).data
}

/** `knowledge_harvester_model()`: the default every chat without its own pick uses; `null` = each chat's own model. */
export function useKnowledgeHarvesterDefault(): ModelInfo | null | undefined {
  const supported = useQuerySupport(['knowledge_harvester_model'])
  const api = useApi()
  return useQuery({ queryKey: queryKeys.knowledgeHarvesterModel, queryFn: () => api.knowledge_harvester_model(), enabled: supported }).data
}

/** `set_session_knowledge_harvester`: a model is this chat's own pick, `null` resets it to the default. The reply is the stored pick. */
export function useSetKnowledgeHarvester(session: string) {
  const api = useApi()
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ model, effort }: { model: string | null; effort: string | null }) => routed(session, () => api.set_session_knowledge_harvester(session, model, effort)),
    onSuccess: (pick) => pick && qc.setQueryData(queryKeys.sessionKnowledgeHarvester(session), pick),
    onError: (e) => toast.error(e instanceof Error ? e.message : String(e)),
  })
}
