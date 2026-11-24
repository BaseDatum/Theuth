// 210 §7.4: the side effects behind `store/public-shares` — seed by `share_list` on every open connection, follow the
// `share.*@1` topics, keep the tray item in step, take the tray's Stop all result (the shell runs it,
// `crates/desktop/src/share.rs`), and post "Live: <url>" / "First visitor on <host>" when the window is hidden (item 7).
// Mounted once, in the main window's shell.

import { useEffect } from 'react'
import { applyStopped, loadMachines, loadShares } from '@/components/public-share/actions'
import { notifyIfHidden } from '@/lib/os-notify'
import { isDesktop } from '@/lib/platform'
import { onShareStoppedAll, shareTray } from '@/lib/tauri'
import { useConnection } from '@/store/connection'
import { firstVisitors, newlyLive, summarize, usePublicShares } from '@/store/public-shares'
import { onTopic } from '@/store/sessions'

export function PublicShareHost() {
  const client = useConnection((s) => s.client)
  const open = useConnection((s) => s.state === 'open')

  useEffect(() => {
    if (!client || !open) return
    usePublicShares.getState().reset()
    // Listen first, then seed: a change between the two lands after the baseline or in it, never lost.
    const offs = [
      onTopic('share.changed@1', (e) => {
        const st = usePublicShares.getState()
        st.changed(e.share)
        // A share from a machine this webview has no name for (paired since): read the mesh again.
        if (st.machines && !(e.share.engine in st.machines.names)) void loadMachines().catch(() => undefined)
      }),
      onTopic('share.traffic@1', (e) => usePublicShares.getState().setTraffic(e)),
      onTopic('share.status@1', (e) => usePublicShares.getState().setStatus(e)),
    ]
    void loadShares().catch(() => undefined)
    void loadMachines().catch(() => undefined)
    return () => {
      for (const off of offs) off()
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
    const offTray = onShareStoppedAll((r) => applyStopped('all', r))
    return () => {
      offStore()
      void offTray.then((f) => f())
    }
  }, [])

  return null
}
