import { useQuery } from '@tanstack/react-query'
import { useApi } from '@/lib/rpc'
import { useSupported } from '@/rpc/capabilities'
import type { Project } from '@/rpc/bindings/method.agent.output'

/** 223: `projects_list` (rows plus implicit labels); every project write and `agent.projects` invalidate it. */
export const projectsKey = ['projects'] as const

/**
 * The raw `projects_list` query; `select` shapes it per caller. Kept free of the task and note lists so the project
 * picker can read it without an import cycle. Strict support (no legacy probe): an engine without the method has no
 * projects to list, and every label picker reads this.
 */
export function useProjectRows<T = Project[]>(select?: (rows: Project[]) => T) {
  const can = useSupported({ methods: ['projects_list'] })
  const api = useApi()
  return useQuery({ enabled: can, queryKey: projectsKey, queryFn: () => api.projects_list(), select })
}
