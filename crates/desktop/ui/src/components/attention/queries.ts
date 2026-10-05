import { useQuery } from '@tanstack/react-query'
import { PACKET_KEY } from '@/components/hud/queries'
import { PACKET_SIM, simFlag } from '@/components/settings/sandbox-queries'
import { useApi } from '@/lib/rpc'

/** `packet_get(key)` — the agent's summary, pointers, options and checks behind an inbox item (101 §3); off when the item has none. */
export function usePacket(key: string | null) {
  const api = useApi()
  const sim = simFlag()
  return useQuery({
    queryKey: [...PACKET_KEY, key],
    queryFn: () => (sim ? (PACKET_SIM[key as string] ?? null) : api.packet_get(key as string)),
    enabled: key !== null,
  })
}
