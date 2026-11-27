// 224: the cloud-node queries and mutations, and the `cloudnodes.events` fold. Like `store/sandboxes.ts`: the node list
// lives in the `['cloud', 'list']` cache and is patched in place (`CloudNodeChanged` upserts, `CloudNodeRemoved`
// deletes); `CloudNodeProgress` folds into a per-node progress entry (stage, pct, eta, lines) the wizard's step ④ reads.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useAction } from '@/components/settings/shared'
import { useApi } from '@/lib/rpc'
import { useQuerySupport, useSupported } from '@/rpc/capabilities'
import type { Requirements } from '@/rpc/discovery'
import { on } from '@/store/sessions'
import { applyCloudEvent, cloudKeys, removeNode, subscribeCloudEvents, upsertNode } from './events'
import type { CloudProgress } from './format'
import type { CloudEvent, CloudList, CloudNode, K8sInfo, NodeDesktop, NodeLogs, NodeSpec, ProviderInfo, ProviderKind, SizeList, TerminalOpen } from './types'

export { applyCloudEvent, cloudKeys, removeNode, subscribeCloudEvents, upsertNode }

/** What the cloud section needs of the engine; an engine without `cloudnodes` hides it (the Machines tab stays). */
export const CLOUD_REQUIRED = {
  methods: ['cloud_providers', 'cloud_list', 'cloud_create', 'cloud_sizes', 'cloud_destroy'],
  topics: ['cloudnodes.events@1'],
} as const

/**
 * The sealed vault (design 225) on the connected engine, and a `cloud_create` that takes the master password for the
 * node it makes (`vault_password`). `vault_status` is called by name (`components/vault/api.ts`), so the requirement
 * is typed loosely. An engine without either (legacy included) hides the vault field of Configure and the `sealed`
 * status, and creates nodes as before.
 */
const VAULT_REQUIRED = {
  methods: ['vault_status'],
  fields: [{ method: 'cloud_create', parameter: 'vault_password', path: [] }],
} as unknown as Requirements

export function useVaultSupport(): boolean {
  return useSupported(VAULT_REQUIRED)
}

type Credential = Extract<CloudEvent, { type: 'CloudCredentialChanged' }>

/** Keep the caches following `cloudnodes.events` while any cloud view is mounted (ref-counted with `useInvalidateOn`). */
export function useCloudEvents(): void {
  const qc = useQueryClient()
  useEffect(() => subscribeCloudEvents(qc), [qc])
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
    // `vaultPassword`: the node's master password, only from an engine with `useVaultSupport` (else `null`).
    create: useAction(({ spec, vaultPassword }: { spec: NodeSpec; vaultPassword: string | null }): Promise<CloudNode> => api.cloud_create(spec, vaultPassword), {
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
