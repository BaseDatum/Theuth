import type { Api, MethodMap } from '@/rpc/bindings/client'
import type { AgentFile as Input } from '@/rpc/bindings/method.roster.input'
import type { AgentFile as File, AgentSummary as Summary } from '@/rpc/bindings/method.roster.output'

type Isolation = { isolated_memory?: boolean } // 196-adapter
export type AgentInput = Input & Isolation // 196-adapter
export type AgentFile = File & Isolation // 196-adapter
export type AgentSummary = Summary & Isolation & { file: AgentFile } // 196-adapter

export function isIsolated(agent: AgentInput | AgentSummary): boolean {
  return agent.isolated_memory === true || ('file' in agent && agent.file.isolated_memory === true)
}

export function memoryRefusal(error: unknown): string | undefined {
  const message = error instanceof Error ? error.message : String(error)
  return message.startsWith('isolated_memory:') ? message : undefined
}

export function listMemory(api: Pick<Api, 'knowledge_list'>, agent: string, limit = 100000) {
  const list = api.knowledge_list as (prefix: string | null, type: string | null, tag: string | null, limit: number, bundle: string) => Promise<MethodMap['knowledge_list']['result']> // 196-adapter
  return list(null, null, null, limit, `agent:${agent}`)
}

export function getMemory(api: Pick<Api, 'knowledge_get'>, agent: string, id: string) {
  const get = api.knowledge_get as (id: string, bundle: string) => Promise<MethodMap['knowledge_get']['result']> // 196-adapter
  return get(id, `agent:${agent}`)
}

export function searchMemory(api: Pick<Api, 'knowledge_search'>, agent: string, query: string) {
  const search = api.knowledge_search as (query: string, limit: number, bundle: string) => Promise<MethodMap['knowledge_search']['result']> // 196-adapter
  return search(query, 50, `agent:${agent}`)
}
