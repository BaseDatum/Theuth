// 224: the cloud-node queries and mutations, and the `cloudnodes.events` fold. Like `store/sandboxes.ts`: the node list
// lives in the `['cloud', 'list']` cache and is patched in place (`CloudNodeChanged` upserts, `CloudNodeRemoved`
// deletes); `CloudNodeProgress` folds into a per-node progress entry (stage, pct, eta, lines) the wizard's step ④ reads.

import { type QueryClient, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useAction } from '@/components/settings/shared'
import { notify } from '@/lib/notify'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { on } from '@/store/sessions'
import { type CloudProgress, foldProgress } from './format'
import type { CloudEvent, CloudList, CloudNode, K8sInfo, NodeDesktop, NodeLogs, NodeSpec, ProviderInfo, ProviderKind, SizeList, TerminalOpen } from './types'

export const cloudKeys = {
  all: ['cloud'] as const,
  list: ['cloud', 'list'] as const,
  providers: ['cloud', 'providers'] as const,
  sizes: (provider: ProviderKind | null, region: string | null) => ['cloud', 'sizes', provider, region] as const,
  k8s: (context: string | null) => ['cloud', 'k8s', context] as const,
  logs: (name: string) => ['cloud', 'logs', name] as const,
  progress: (name: string) => ['cloud', 'progress', name] as const,
}

/** What the cloud section needs of the engine; an engine without `cloudnodes` hides it (the Machines tab stays). */
export const CLOUD_REQUIRED = {
  methods: ['cloud_providers', 'cloud_list', 'cloud_create', 'cloud_sizes', 'cloud_destroy'],
  topics: ['cloudnodes.events@1'],
} as const

type Changed = Extract<CloudEvent, { type: 'CloudNodeChanged' }>
type Progress = Extract<CloudEvent, { type: 'CloudNodeProgress' }>
type Removed = Extract<CloudEvent, { type: 'CloudNodeRemoved' }>
type Credential = Extract<CloudEvent, { type: 'CloudCredentialChanged' }>

export function upsertNode(list: CloudList | undefined, node: CloudNode): CloudList {
  const nodes = list?.nodes ?? []
  return { errors: list?.errors ?? [], nodes: nodes.some((n) => n.name === node.name) ? nodes.map((n) => (n.name === node.name ? node : n)) : [...nodes, node] }
}

export function removeNode(list: CloudList | undefined, name: string): CloudList {
  return { errors: list?.errors ?? [], nodes: (list?.nodes ?? []).filter((n) => n.name !== name) }
}

/** Fold one `cloudnodes.events` event into the caches; a create that finishes or fails says so in the status bar. */
export function applyCloudEvent(qc: QueryClient, e: CloudEvent): void {
  switch (e.type) {
    case 'CloudNodeChanged': {
      const before = qc.getQueryData<CloudList>(cloudKeys.list)?.nodes.find((n) => n.name === e.node.name)
      qc.setQueryData<CloudList>(cloudKeys.list, (list) => upsertNode(list, e.node))
      if (before?.state === 'creating' && e.node.state === 'ready') notify.success(`${e.node.name} is ready`, { id: `cloud-${e.node.name}` })
      if (before?.state === 'creating' && e.node.state === 'failed') notify.error(`${e.node.name} could not be created`, { id: `cloud-${e.node.name}`, description: e.node.error ?? undefined })
      return
    }
    case 'CloudNodeRemoved':
      qc.setQueryData<CloudList>(cloudKeys.list, (list) => removeNode(list, e.name))
      qc.removeQueries({ queryKey: cloudKeys.progress(e.name) })
      return
    case 'CloudNodeProgress':
      qc.setQueryData<CloudProgress>(cloudKeys.progress(e.name), (p) => foldProgress(p, e))
      return
    case 'CloudCredentialChanged':
      void qc.invalidateQueries({ queryKey: cloudKeys.providers })
      return
  }
}

/** One subscription per QueryClient however many cloud views are mounted (a progress line must fold once). */
const subscribed = new WeakMap<QueryClient, { count: number; off: () => void }>()

/** Keep the caches following `cloudnodes.events` while any cloud view is mounted. */
export function useCloudEvents(): void {
  const qc = useQueryClient()
  useEffect(() => {
    const held = subscribed.get(qc)
    if (held) held.count++
    else {
      const offs = [
        on('cloudnodes.events@1', 'CloudNodeChanged', (e: Changed) => applyCloudEvent(qc, e)),
        on('cloudnodes.events@1', 'CloudNodeProgress', (e: Progress) => applyCloudEvent(qc, e)),
        on('cloudnodes.events@1', 'CloudNodeRemoved', (e: Removed) => applyCloudEvent(qc, e)),
        on('cloudnodes.events@1', 'CloudCredentialChanged', (e: Credential) => applyCloudEvent(qc, e)),
      ]
      subscribed.set(qc, { count: 1, off: () => offs.forEach((off) => off()) })
    }
    return () => {
      const s = subscribed.get(qc)
      if (!s) return
      if (--s.count === 0) {
        s.off()
        subscribed.delete(qc)
      }
    }
  }, [qc])
}

/** `CloudCredentialChanged` for one provider (the OAuth wait); `cb` sees each change. */
export function useCredentialChanged(provider: ProviderKind | null, cb: (e: Credential) => void): void {
  useEffect(() => {
    if (!provider) return
    return on('cloudnodes.events@1', 'CloudCredentialChanged', (e: Credential) => {
      if (e.provider === provider) cb(e)
    })
  }, [provider, cb])
}

export function useCloudProviders() {
  const can = useQuerySupport(['cloud_providers'])
  const api = useApi()
  return useQuery<ProviderInfo[]>({ enabled: can, queryKey: cloudKeys.providers, queryFn: () => api.cloud_providers(), staleTime: 30_000 })
}

/** `cloud_list`, patched live by the events; the poll catches provider-side changes (a node destroyed in its console). */
export function useCloudList() {
  const can = useQuerySupport(['cloud_list'])
  const api = useApi()
  return useQuery<CloudList>({ enabled: can, queryKey: cloudKeys.list, queryFn: () => api.cloud_list(), refetchInterval: 60_000 })
}

export function useCloudNode(name: string | null): CloudNode | null {
  const list = useCloudList()
  return (name && list.data?.nodes.find((n) => n.name === name)) || null
}

/**
 * `cloud_sizes(provider, region)`: the live sizes and prices. Fetched whenever the wizard opens or the provider or
 * region changes (the engine caches the provider's prices for an hour, so `staleTime: 0` costs nothing).
 */
export function useCloudSizes(provider: ProviderKind | null, region: string | null, enabled = true) {
  const can = useQuerySupport(['cloud_sizes'])
  const api = useApi()
  return useQuery<SizeList>({
    enabled: can && enabled && !!provider,
    queryKey: cloudKeys.sizes(provider, region),
    queryFn: () => api.cloud_sizes(provider as ProviderKind, region),
    staleTime: 0,
    retry: false,
  })
}

export function useK8sInfo(context: string | null, enabled = true) {
  const can = useQuerySupport(['cloud_k8s_info'])
  const api = useApi()
  return useQuery<K8sInfo>({ enabled: can && enabled, queryKey: cloudKeys.k8s(context), queryFn: () => api.cloud_k8s_info(context), staleTime: 30_000 })
}

export function useCloudLogs(name: string, tail = 200) {
  const api = useApi()
  return useQuery<NodeLogs>({ queryKey: cloudKeys.logs(name), queryFn: () => api.cloud_logs(name, tail), staleTime: 0 })
}

/** The folded `CloudNodeProgress` of one node; fed only by events (the fixture seeds it). */
export function useCloudProgress(name: string | null): CloudProgress | undefined {
  const q = useQuery<CloudProgress | undefined>({ queryKey: cloudKeys.progress(name ?? ''), queryFn: () => undefined, enabled: false, staleTime: Infinity, gcTime: Infinity })
  return name ? q.data : undefined
}

/** Every mutation of the cloud UI; failures land in the status bar (`useAction` → `notify.error`). */
export function useCloudActions() {
  const api = useApi()
  const qc = useQueryClient()
  const keys = [cloudKeys.list] as const
  const providers = [cloudKeys.providers] as const
  return {
    create: useAction((spec: NodeSpec): Promise<CloudNode> => api.cloud_create(spec), {
      keys,
      // The row shows at once in `creating`; the events then own it.
      onSuccess: (node) => qc.setQueryData<CloudList>(cloudKeys.list, (list) => upsertNode(list, node)),
    }),
    restart: useAction((name: string) => api.cloud_restart(name), { keys }),
    resize: useAction(({ name, memory_gb }: { name: string; memory_gb: number }) => api.cloud_resize(name, memory_gb), { keys }),
    update: useAction((name: string) => api.cloud_update(name), { keys }),
    destroy: useAction(({ name, keep }: { name: string; keep: boolean }): Promise<string[]> => api.cloud_destroy(name, keep), { keys }),
    importCredential: useAction(({ provider, id }: { provider: ProviderKind; id: string }): Promise<ProviderInfo> => api.cloud_import_credential(provider, id), { keys: providers }),
    setToken: useAction(({ provider, token }: { provider: ProviderKind; token: string }): Promise<ProviderInfo> => api.cloud_set_token(provider, token), { keys: providers }),
    terminal: useAction((name: string): Promise<TerminalOpen> => api.cloud_terminal(name)),
    desktop: useAction((name: string): Promise<NodeDesktop> => api.cloud_desktop(name)),
  }
}
