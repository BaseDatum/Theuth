// 221 D20 `lock_on_sleep` (macOS only): the desktop shell reports sleep, display sleep and screen lock
// (`crates/desktop/src/power.rs`) as `power-lock { kind }`; this window forwards each to the engine it is connected to,
// which locks every external-secrets connection with `lock_on_sleep` on. Off Tauri, and on an engine without the method,
// it does nothing.

import { listen } from '@tauri-apps/api/event'
import { useEffect } from 'react'
import { useApi } from './rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { isTauri } from './tauri'

/** What the desktop shell reports (`crates/desktop/src/power.rs`); nothing else is forwarded. */
const KINDS = ['sleep', 'screen_lock'] as const
export type PowerEvent = (typeof KINDS)[number]

/** The payload's kind when it is one the shell sends, else `null`. */
export function powerKind(payload: unknown): PowerEvent | null {
  const kind = (payload as { kind?: unknown } | null)?.kind
  return KINDS.find((k) => k === kind) ?? null
}

/** Forward the shell's `power-lock` events to `secrets_power_event` while mounted. */
export function usePowerLock(): void {
  const api = useApi()
  const can = useQuerySupport(['secrets_power_event'])
  useEffect(() => {
    if (!can || !isTauri()) return
    let gone = false
    let off: (() => void) | undefined
    void listen<unknown>('power-lock', (e) => {
      const kind = powerKind(e.payload)
      if (kind) void api.secrets_power_event(kind).catch(() => undefined)
    }).then((u) => {
      if (gone) u()
      else off = u
    })
    return () => {
      gone = true
      off?.()
    }
  }, [api, can])
}
