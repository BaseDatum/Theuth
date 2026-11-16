import type { Api } from '@/rpc/bindings/client'
import type { AgentFile as AgentInput } from '@/rpc/bindings/method.roster.input'
import type { AgentSummary } from '@/rpc/bindings/method.roster.output'

export type { AgentFile as AgentInput } from '@/rpc/bindings/method.roster.input'
export type { AgentFile, AgentSummary } from '@/rpc/bindings/method.roster.output'

export function isIsolated(agent: AgentInput | AgentSummary): boolean {
  return agent.isolated_memory === true || ('file' in agent && agent.file.isolated_memory === true)
}

export function memoryRefusal(error: unknown): string | undefined {
  const message = error instanceof Error ? error.message : String(error)
  return message.startsWith('isolated_memory:') ? message : undefined
}

export function listMemory(api: Pick<Api, 'knowledge_list'>, agent: string, limit = 100000) {
  return api.knowledge_list(null, null, null, limit, `agent:${agent}`)
}

export function getMemory(api: Pick<Api, 'knowledge_get'>, agent: string, id: string) {
  return api.knowledge_get(id, `agent:${agent}`)
}

export function searchMemory(api: Pick<Api, 'knowledge_search'>, agent: string, query: string) {
  return api.knowledge_search(query, 50, `agent:${agent}`)
}
