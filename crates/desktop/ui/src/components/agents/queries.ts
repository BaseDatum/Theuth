import { useQuerySupport } from '@/rpc/capabilities'
import { receiveComponent } from '@/rpc/bindings/receive'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { notify as toast } from '@/lib/notify'
import { useAction } from '@/components/settings/shared'
import { AGENTS, mockOn } from '@/components/tasks/mock'
import { THEUTH } from '@/components/tasks/helpers'
import { keys } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import { hiring } from './hiring'
import type { AgentFile } from "@/rpc/bindings/method.roster.output"

/** `agent_get`; the roster synthesises `theuth` from `prompt.md`, the mock does the same. */
export function useAgentFile(name: string) {
  const canQuery1 = useQuerySupport(["agent_get"])
  const api = useApi()
  return useQuery({ enabled: canQuery1,
    queryKey: [...keys.agents, name],
    queryFn: (): Promise<AgentFile> | AgentFile => {
      if (!mockOn()) return api.agent_get(name)
      const f = AGENTS.find((a) => a.file.name === name)?.file
      if (f) return f
      if (name === THEUTH)
        return receiveComponent('method.roster.output.AgentFile', { name, title: name, reports_to: 'user', model: null, effort: null, tools: 'destructive', mcp: null, budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, runs_in: 'any', body: 'Standing instructions (prompt.md).' })
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
    put: useAction((f: Parameters<typeof api.agent_put>[0]) => api.agent_put(f), { keys: roster, onSuccess: (_, f) => toast(`Saved ${f.name}`) }),
    pause: useAction(({ name, paused }: { name: string; paused: boolean }) => api.agent_pause(name, paused), {
      keys: roster,
      onSuccess: (_, v) => toast(v.paused ? `Paused ${v.name} — its work is suspended` : `${v.name} resumed`),
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
  const canQuery2 = useQuerySupport(["list_models"])
  const api = useApi()
  return useQuery({ enabled: canQuery2, queryKey: queryKeys.models, queryFn: () => api.list_models() })
}

export function useMcpServers() {
  const canQuery3 = useQuerySupport(["list_mcp"])
  const api = useApi()
  return useQuery({ enabled: canQuery3, queryKey: queryKeys.mcp(null), queryFn: () => api.list_mcp(null) })
}

/** Settings › Harnesses: the ACP entries (`agents.yaml`); `native` is implicit. */
export function useHarnesses() {
  const canQuery4 = useQuerySupport(["list_acp"])
  const api = useApi()
  return useQuery({ enabled: canQuery4, queryKey: queryKeys.acp, queryFn: () => api.list_acp(false) })
}

/** 201 R3-12: Settings › Context (`endless_settings_get`): the tail marks a lone per-agent mark pairs with. */
export function useEndlessSettings() {
  const canQuery5 = useQuerySupport(['endless_settings_get'])
  const api = useApi()
  return useQuery({ enabled: canQuery5, queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
}
