// 39 §4.2: the pending app update. Rust emits `update-ready { version }` once the bundle is
// verified; the sidebar dot and Settings › General › App read it. Nothing here pops anything.

import { create } from 'zustand'
import { onUpdateReady } from '@/lib/tauri'

type UpdateStore = {
  /** Version that installs at quit; `null` until `update-ready` fires. */
  pending: string | null
}

export const useUpdate = create<UpdateStore>()(() => ({ pending: null }))

let started = false

/** Subscribe to `update-ready` once for the app's lifetime (39 §4.2). */
export function startUpdateListener(): void {
  if (started) return
  started = true
  void onUpdateReady((p) => useUpdate.setState({ pending: p.version }))
}
