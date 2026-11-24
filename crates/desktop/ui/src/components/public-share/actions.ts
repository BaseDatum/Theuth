// 210 §7.4: Stop and Stop all are never gated (no approval, no confirmation). The store follows `share.changed@1`;
// a Stop all's `edge_pending` goes to the store so the strip and the cards say the edge stop is still pending.

import { notify as toast } from '@/lib/notify'
import { type Stopped, shareApi } from '@/components/public-share/wire'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { type Machines, usePublicShares } from '@/store/public-shares'

export async function loadShares(): Promise<void> {
  usePublicShares.getState().replace(await shareApi().share_list())
}

/** The mesh's machine names for shares on other machines (§7.4 item 1): `mesh_status`, when the engine has it. */
export async function loadMachines(): Promise<void> {
  if (!supports(useConnection.getState().discovery, { methods: ['mesh_status'] })) return usePublicShares.getState().setMachines(null)
  const m = await shareApi().mesh_status()
  const names: Machines['names'] = { [m.self_id]: m.self_name }
  for (const p of m.peers) names[p.id] = p.name
  usePublicShares.getState().setMachines({ self: m.self_id, hub: m.lease.holder, names })
}

/** The toast for a `share_stop` result: a pending edge stop is not "stopped" (FIX-A M7). */
export function stoppedText(id: string, r: Stopped): string {
  const n = r.stopped === 1 ? '1 public share' : `${r.stopped} public shares`
  if (r.edge_pending) return `Stopped ${n} here; the edge stop is pending and will be retried (${r.edge_pending})`
  return id === 'all' ? `Stopped ${n}` : 'Stopped'
}

/** A `share_stop("all")` result, from this webview or the tray's (`lib/tauri.ts` `onShareStoppedAll`). */
export function applyStopped(id: string, r: Stopped): void {
  if (id === 'all') usePublicShares.getState().setEdgePending(r.edge_pending ?? null)
  if (r.edge_pending) toast.warning(stoppedText(id, r))
  else if (id === 'all') toast.success(stoppedText(id, r))
}

/** Stop one share, or `all` (the mesh, at the edge). */
export async function stopShare(id: string): Promise<void> {
  try {
    applyStopped(id, await shareApi().share_stop(id))
  } catch (e) {
    toast.error(`Stop failed: ${e instanceof Error ? e.message : String(e)}`)
  }
}

export const stopAllShares = () => stopShare('all')

/** Share ids whose password was already shown (ids only, never the password): the owner sees it once (FIX-A M9), in
 * this page and, where storage works, across reloads of this webview. */
const REVEALED_KEY = 'theuth.public-share.revealed'
const revealed = new Set<string>()

/** `share_password {id}` the first time it is asked for `id` here; `null` after that, for a non-owner, or once the
 * engine restarted (it keeps the password in memory only). `known` = the RPC `Published.password` already in hand. */
export async function takePassword(id: string, known?: string | null): Promise<string | null> {
  try {
    for (const s of JSON.parse(localStorage.getItem(REVEALED_KEY) ?? '[]') as string[]) revealed.add(s)
  } catch {
    // No storage: this page's set only.
  }
  if (revealed.has(id)) return null
  revealed.add(id)
  try {
    localStorage.setItem(REVEALED_KEY, JSON.stringify([...revealed].slice(-200)))
  } catch {
    // No storage: this page's set only.
  }
  try {
    return (await shareApi().share_password(id)) ?? known ?? null
  } catch {
    return known ?? null
  }
}
