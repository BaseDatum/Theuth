import { useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { useAction } from '@/components/settings/shared'
import { AGENTS, mockOn } from '@/components/tasks/mock'
import { THEUTH } from '@/components/tasks/helpers'
import { keys } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import { hiring } from './hiring'
import type { AgentFile } from '@/rpc/bindings/AgentFile'

/** `agent_get`; the roster synthesises `theuth` from `prompt.md`, the mock does the same. */
export function useAgentFile(name: string) {
  const api = useApi()
  return useQuery({
    queryKey: [...keys.agents, name],
    queryFn: (): Promise<AgentFile> | AgentFile => {
      if (!mockOn()) return api.agent_get(name)
      const f = AGENTS.find((a) => a.file.name === name)?.file
      if (f) return f
      if (name === THEUTH)
        return { name, title: name, reports_to: 'user', model: null, effort: null, tools: 'destructive', mcp: null, budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, runs_in: 'any', body: 'Standing instructions (prompt.md).' }
      return Promise.reject(new Error('no such agent'))
    },
  })
}

/** Every write the Agents screens make, each refetching the roster and toasting. */
export function useAgentActions() {
  const api = useApi()
  const qc = useQueryClient()
  const roster = [keys.agents, keys.tasks] as const
  const h = hiring(api)
  return {
    put: useAction((f: AgentFile) => api.agent_put(f), { keys: roster, onSuccess: (_, f) => toast(`Saved ${f.name}`) }),
    pause: useAction(({ name, paused }: { name: string; paused: boolean }) => api.agent_pause(name, paused), {
      keys: roster,
      onSuccess: (_, v) => toast(v.paused ? `Paused ${v.name}` : `Resumed ${v.name}`),
    }),
    approve: useAction(h.approve, { keys: roster, onSuccess: (_, name) => toast(`Hired ${name}`) }),
    reject: useAction(({ name, note }: { name: string; note: string }) => h.reject(name, note), { keys: roster, onSuccess: (_, v) => toast(`Declined ${v.name}`) }),
    remove: useAction((name: string) => api.agent_remove(name, null), {
      keys: roster,
      onSuccess: (_, name) => {
        qc.removeQueries({ queryKey: [...keys.agents, name] })
        toast(`Removed ${name}`)
      },
    }),
  }
}

export function useModels() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
}

export function useMcpServers() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.mcp(null), queryFn: () => api.list_mcp(null) })
}

/** Settings › Harnesses: the ACP entries (`agents.yaml`); `native` is implicit. */
export function useHarnesses() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.acp, queryFn: () => api.list_acp(false) })
}
