// 210 §7.4: Stop and Stop all are never gated (no approval, no confirmation); the store reloads afterwards so a
// client without the `share.changed@1` binding still sees the result.

import { toast } from 'sonner'
import { shareCall } from '@/components/public-share/wire'
import { usePublicShares } from '@/store/public-shares'

export async function loadShares(): Promise<void> {
  usePublicShares.getState().replace(await shareCall('share_list'))
}

/** Stop one share, or `all` (the mesh, at the edge). */
export async function stopShare(id: string): Promise<void> {
  try {
    const { stopped } = await shareCall('share_stop', id)
    if (id === 'all') toast.success(stopped === 1 ? 'Stopped 1 public share' : `Stopped ${stopped} public shares`)
  } catch (e) {
    toast.error(`Stop failed: ${e instanceof Error ? e.message : String(e)}`)
  } finally {
    await loadShares().catch(() => undefined)
  }
}

export const stopAllShares = () => stopShare('all')
