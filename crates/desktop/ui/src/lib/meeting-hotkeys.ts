// 53 decisions 9–10 / 03 §4: the two global shortcuts of a meeting — push-to-talk (held → `meeting_ptt`)
// and the eye (toggle hidden-from-capture). Registered from the main webview only, on a desktop shell
// (the daemon records on every desktop OS, 58 §6.1), and only while a meeting records or is paused:
// a global ⌥` outside a meeting is a stolen key.

import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useEffect } from 'react'
import { notify as toast } from '@/lib/notify'
import { useCompanionAlive } from '@/lib/companion'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { isDesktop } from '@/lib/platform'
import { isIsland, setHiddenFromCapture } from '@/lib/tauri'
import { live, useMeeting } from '@/store/meeting'
import { useConnection } from '@/store/connection'

/** Mount once in the main webview beside `useTauri()`. */
export function useMeetingHotkeys(): void {
  const active = useMeeting((s) => live(s.state))
  const ptt = useMeetingSettings((s) => s.ptt)
  const eye = useMeetingSettings((s) => s.eye)
  // 149 D15: the companion's island registers PTT/eye while it is alive.
  const companion = useCompanionAlive()
  useEffect(() => {
    if (!active || !isDesktop() || isIsland() || companion) return
    let held = false
    const api = () => useConnection.getState().api
    void register(ptt, (e) => {
      const down = e.state === 'Pressed'
      if (down === held) return
      held = down
      void api()?.meeting_ptt(down).catch(() => undefined)
    }).catch(() => toast.error(`Could not register ${ptt}; another app holds it`))
    void register(eye, (e) => {
      if (e.state === 'Pressed') void setHiddenFromCapture(!useMeetingSettings.getState().hidden)
    }).catch(() => toast.error(`Could not register ${eye}; another app holds it`))
    return () => {
      void unregister([ptt, eye]).catch(() => undefined)
      // The key may still be down across a state change or rebind: the span must close.
      if (held) void api()?.meeting_ptt(false).catch(() => undefined)
    }
  }, [active, ptt, eye, companion])
}
