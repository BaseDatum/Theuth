// 221 D20 `lock_on_sleep`: the desktop shell reports sleep and screen lock (`crates/desktop/src/power.rs`) as
// `power-lock { kind }`; this window forwards each to the engine it is connected to, which locks every external-secrets
// connection with `lock_on_sleep` on. Off Tauri, and on an engine without the method, it does nothing.

import { listen } from '@tauri-apps/api/event'
import { useEffect } from 'react'
import { useApi } from './rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { isTauri } from './tauri'

/** What the desktop shell reports (`crates/desktop/src/power.rs`). */
type PowerEvent = 'sleep' | 'screen_lock'

/** Forward the shell's `power-lock` events to `secrets_power_event` while mounted. */
export function usePowerLock(): void {
  const api = useApi()
  const can = useQuerySupport(['secrets_power_event'])
  useEffect(() => {
    if (!can || !isTauri()) return
    let gone = false
    let off: (() => void) | undefined
    void listen<{ kind: PowerEvent }>('power-lock', (e) => void api.secrets_power_event(e.payload.kind).catch(() => undefined)).then((u) => {
      if (gone) u()
      else off = u
    })
    return () => {
      gone = true
      off?.()
    }
  }, [api, can])
}
