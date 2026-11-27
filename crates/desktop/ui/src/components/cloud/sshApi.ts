// SSH provider RPC helpers (`cloud_ssh_hosts`, `cloud_ssh_probe`), usable over a raw `RpcClient`
// (`client.call`, as `components/vault/api.ts`) or the generated typed `Api`.

import { useMutation, useQuery } from '@tanstack/react-query'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { RpcClient } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import type { SshHosts, SshOptions, SshProbe } from './types'

export interface SshApi {
  hosts(): Promise<SshHosts>
  probe(opts: SshOptions): Promise<SshProbe>
}

export const sshKeys = {
  hosts: ['cloud', 'ssh', 'hosts'] as const,
}

/** Call `cloud_ssh_hosts` / `cloud_ssh_probe` directly on an `RpcClient`. */
export function sshApi(client: Pick<RpcClient, 'call'>): SshApi {
  return {
    hosts: () => client.call<SshHosts>('cloud_ssh_hosts', []),
    probe: (opts: SshOptions) => client.call<SshProbe>('cloud_ssh_probe', [opts]),
  }
}

export function useSshHosts(enabled = true) {
  const can = useQuerySupport(['cloud_ssh_hosts'])
  const api = useApi()
  const client = useConnection((s) => s.client)
  return useQuery<SshHosts>({
    enabled: can && enabled,
    queryKey: sshKeys.hosts,
    queryFn: () => {
      if (typeof api.cloud_ssh_hosts === 'function') return api.cloud_ssh_hosts()
      if (client && typeof client.call === 'function') return sshApi(client).hosts()
      return Promise.resolve({ ssh_available: false, hosts: [] })
    },
    staleTime: 30_000,
  })
}

export function useSshProbe() {
  const api = useApi()
  const client = useConnection((s) => s.client)
  return useMutation<SshProbe, Error, SshOptions>({
    mutationFn: (opts) => {
      if (typeof api.cloud_ssh_probe === 'function') return api.cloud_ssh_probe(opts)
      if (client && typeof client.call === 'function') return sshApi(client).probe(opts)
      return Promise.reject(new Error('SSH probe is not available on this engine'))
    },
  })
}
