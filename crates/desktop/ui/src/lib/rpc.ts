import { useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import type { Api } from '@/rpc/api'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

/** The connected `Api`. Routes render only once connected, so a missing client is a bug. */
export function useApi(): Api {
  const api = useConnection((s) => s.api)
  if (!api) throw new Error('useApi before the connection opened')
  return api
}

export const queryKeys = {
  sessions: ['sessions'] as const,
  session: (id: string) => ['session', id] as const,
  search: (q: string) => ['search', q] as const,
  models: ['models'] as const,
  mcp: (s: string | null) => ['mcp', s] as const,
  skills: ['skills'] as const,
  skill: (name: string) => ['skill', name] as const,
  logins: ['logins'] as const,
  providers: ['providers'] as const,
  acp: ['acp'] as const,
  acpRegistry: ['acp', 'registry'] as const,
  agents: (id: string) => ['agents', id] as const,
  jobs: (id: string) => ['jobs', id] as const,
  prompt: (id: string) => ['prompt', id] as const,
  knowledgeList: ['knowledge', 'list'] as const,
  knowledgeDoc: (id: string) => ['knowledge', 'doc', id] as const,
  knowledgeHistory: (id: string) => ['knowledge', 'history', id] as const,
  knowledgeSearch: (q: string) => ['knowledge', 'search', q] as const,
  artifacts: ['artifacts'] as const,
  info: ['info'] as const,
  titleModel: ['title-model'] as const,
  signal: ['signal'] as const,
  slack: ['slack'] as const,
  computer: ['computer'] as const,
  secrets: ['secrets'] as const,
  updateStatus: ['update-status'] as const,
  voice: ['voice'] as const,
}

const SESSIONS_DEBOUNCE_MS = 500

/** Wire events to query invalidation (25 §6.2): the session list debounced, the rest immediate. */
export function useInvalidateOn(): void {
  const qc = useQueryClient()
  useEffect(() => {
    let timer: number | undefined
    const sessions = () => {
      clearTimeout(timer)
      timer = window.setTimeout(() => void qc.invalidateQueries({ queryKey: queryKeys.sessions }), SESSIONS_DEBOUNCE_MS)
    }
    const offs = [
      on('TurnFinished', sessions),
      on('SubagentSpawned', sessions),
      on('TitleChanged', sessions),
      on('SessionDeleted', () => void qc.invalidateQueries({ queryKey: queryKeys.sessions })),
      on('McpProgress', () => void qc.invalidateQueries({ queryKey: ['mcp'] })),
      on('LoginFinished', () => void qc.invalidateQueries({ queryKey: queryKeys.logins })),
      on('KnowledgeChanged', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
        void qc.invalidateQueries({ queryKey: queryKeys.knowledgeDoc(e.id) })
      }),
      on('ArtifactShown', () => void qc.invalidateQueries({ queryKey: queryKeys.artifacts })),
      on('SecretsChanged', () => void qc.invalidateQueries({ queryKey: queryKeys.secrets })),
      on('SecretResolved', () => void qc.invalidateQueries({ queryKey: queryKeys.secrets })),
    ]
    return () => {
      clearTimeout(timer)
      for (const off of offs) off()
    }
  }, [qc])
}
