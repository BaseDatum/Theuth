// 210 §7.4: the side effects behind `store/public-shares` — seed by `share_list` on every connection, follow the
// `share.*@1` topics (or poll `share_list` while the generated bindings do not route them yet), keep the tray item in
// step, answer the tray's Stop, and post "Live: <url>" / "First visitor on <host>" when the window is hidden (item 7).
// Mounted once, in the main window's shell.

import { useEffect } from 'react'
import { loadShares, stopAllShares } from '@/components/public-share/actions'
import { SHARE_TOPICS, type ShareChanged, type ShareStatus, type ShareTraffic, onShareTopic } from '@/components/public-share/wire'
import { notifyIfHidden } from '@/lib/notify'
import { isDesktop } from '@/lib/platform'
import { onShareStopAll, shareTray } from '@/lib/tauri'
import { useConnection } from '@/store/connection'
import { firstVisitors, newlyLive, summarize, usePublicShares } from '@/store/public-shares'

/** While `share.changed@1` is not routed by the bindings: how often `share_list` refreshes the store. */
export const POLL_MS = 10_000

export function PublicShareHost() {
  const client = useConnection((s) => s.client)
  const open = useConnection((s) => s.state === 'open')

  useEffect(() => {
    if (!client || !open) return
    const st = usePublicShares.getState()
    st.reset()
    const load = () => void loadShares().catch(() => undefined)
    load()
    const offs = [
      onShareTopic<ShareChanged>(SHARE_TOPICS.changed, (e) => usePublicShares.getState().changed(e.share)),
      onShareTopic<ShareTraffic>(SHARE_TOPICS.traffic, (e) => usePublicShares.getState().setTraffic(e)),
      onShareTopic<ShareStatus>(SHARE_TOPICS.status, (e) => usePublicShares.getState().setStatus(e)),
    ]
    // TODO(210-M1, WIRE1): drop the poll once `share.changed@1` is in the generated `TOPICS`.
    const timer = offs[0] === null ? window.setInterval(load, POLL_MS) : 0
    return () => {
      for (const off of offs) off?.()
      if (timer) window.clearInterval(timer)
    }
  }, [client, open])

  // Tray item and hidden-window notifications follow the store.
  useEffect(() => {
    let last = -1
    const offStore = usePublicShares.subscribe((next, prev) => {
      const sum = summarize(next.shares, next.traffic)
      const active = sum.live + sum.pending
      if (isDesktop() && active !== last) {
        last = active
        void shareTray(active).catch(() => undefined)
      }
      if (!prev.loaded) return // the first `share_list` is the baseline, not news
      for (const id of newlyLive(prev.shares, next.shares)) {
        const s = next.shares[id]
        if (s) void notifyIfHidden('Live on the public internet', s.url)
      }
      for (const id of firstVisitors(prev.traffic, next.traffic)) {
        const s = next.shares[id]
        if (s) void notifyIfHidden(`First visitor on ${s.host}`, s.url)
      }
    })
    const offTray = onShareStopAll(() => void stopAllShares())
    return () => {
      offStore()
      void offTray.then((f) => f())
    }
  }, [])

  return null
}
