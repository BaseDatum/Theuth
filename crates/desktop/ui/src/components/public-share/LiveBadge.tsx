// 210 §7.4 item 6 (G4): while a share is active and the main window is not shown, the island shows `● LIVE N · Stop`
// (Stop = the shell's native Stop all, `crates/desktop/src/share.rs`). It runs in the island webview: that one has its
// own connection and is never ordered out (03 §9), whereas a hidden main webview is throttled. A live meeting or the
// voice panel owns the island first (`routes/island.tsx`).

import { useEffect, useState } from 'react'
import { loadShares } from '@/components/public-share/actions'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { islandHide, islandPlace, islandShow, mainWindowVisible, shareStopAll } from '@/lib/tauri'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { activeShares, usePublicShares } from '@/store/public-shares'
import { onTopic } from '@/store/sessions'

/** How often the island asks whether the main window is shown, only while something is active. */
export const MAIN_POLL_MS = 2000
/** The badge strip, logical px (the shell's minimum island width and the strip height). */
const WIDTH = 300
const HEIGHT = 44

/** The badge's count, or `null` for none: something active, the main window not shown, no meeting on the island. */
export function liveBadge(active: number, mainVisible: boolean, meeting: boolean): number | null {
  return active > 0 && !mainVisible && !meeting ? active : null
}

/** The island's own view of the mesh's shares (seed by `share_list` per open connection, follow `share.changed@1`)
 * and of the main window; the badge count per [`liveBadge`]. */
export function useLiveBadge(meeting: boolean): number | null {
  const client = useConnection((s) => s.client)
  const open = useConnection((s) => s.state === 'open')
  const supported = useConnection((s) => supports(s.discovery, { methods: ['share_list'] }))
  useEffect(() => {
    if (!client || !open || !supported) return
    usePublicShares.getState().reset()
    const off = onTopic('share.changed@1', (e) => usePublicShares.getState().changed(e.share))
    void loadShares().catch(() => undefined)
    return off
  }, [client, open, supported])
  const active = usePublicShares((s) => activeShares(s.shares).length)
  const watching = active > 0
  const [visible, setVisible] = useState(true)
  useEffect(() => {
    if (!watching) return setVisible(true)
    let live = true
    const check = () => void mainWindowVisible().then((v) => live && setVisible(v), () => undefined)
    check()
    const timer = setInterval(check, MAIN_POLL_MS)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [watching])
  return liveBadge(active, visible, meeting)
}

/** `● LIVE N · Stop` on the island: placed like the meeting island, hidden again when it goes away. */
export function LiveBadge({ count }: { count: number }) {
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    document.documentElement.dataset.island = ''
    void islandPlace(useMeetingSettings.getState().positions).finally(() => void islandShow(WIDTH, HEIGHT))
    return () => {
      delete document.documentElement.dataset.island
      void islandHide()
    }
  }, [])
  const stop = () => {
    setBusy(true)
    void shareStopAll().finally(() => setBusy(false))
  }
  return (
    <div role="status" data-tauri-drag-region data-live-badge="" className="flex h-dvh items-center gap-2 rounded-2xl border border-danger bg-background/95 px-3 text-[13px] text-foreground shadow-lg backdrop-blur">
      <span aria-hidden className="size-2 shrink-0 rounded-full bg-danger" />
      <span className="font-semibold text-danger">LIVE {count}</span>
      <span data-tauri-drag-region className="min-w-0 flex-1 truncate text-fg-secondary">
        Sharing publicly
      </span>
      <button type="button" disabled={busy} onClick={stop} className="shrink-0 rounded-md bg-danger px-2 py-0.5 font-medium text-white hover:bg-danger/90 disabled:opacity-60">
        Stop
      </button>
    </div>
  )
}
