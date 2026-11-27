// 227 piece 3: retries this window's pending "Set on all machines" copies while the app is open, when a machine that was
// offline or locked is back. Client-held: the engines keep no queue (see `set-on-all.ts`).

import { useEffect, useRef, useState } from 'react'
import { readPending, retryPending, runApis } from '@/components/settings/copy/set-on-all'
import { useSettingsMachinesValue } from '@/components/settings/machine-target'
import { notify } from '@/lib/notify'

/** How often the queue is looked at (cheap: localStorage), and how often a machine still away is tried again. */
const LOOK_MS = 30_000

export function PendingSettingsRetry() {
  const [queued, setQueued] = useState(() => readPending().length > 0)
  useEffect(() => {
    const look = () => setQueued(readPending().length > 0)
    const timer = setInterval(look, LOOK_MS)
    window.addEventListener('storage', look)
    return () => {
      clearInterval(timer)
      window.removeEventListener('storage', look)
    }
  }, [])
  // Nothing queued: no mesh or caps queries at all.
  return queued ? <Retry onEmpty={() => setQueued(false)} /> : null
}

function Retry({ onEmpty }: { onEmpty: () => void }) {
  const settings = useSettingsMachinesValue(null, () => {})
  const busy = useRef(false)
  // Back = online and unlocked; the key changes when one comes back, and the timer catches an unlock the list cannot see.
  const back = settings.machines.filter((m) => m.online && !m.sealed).map((m) => m.id).join(',')
  const [tick, setTick] = useState(0)
  useEffect(() => {
    const timer = setInterval(() => setTick((t) => t + 1), LOOK_MS)
    return () => clearInterval(timer)
  }, [])
  useEffect(() => {
    if (busy.current || settings.machines.length === 0) return
    busy.current = true
    void retryPending(runApis(settings), settings.machines)
      .then((ran) => {
        if (ran > 0 && readPending().length === 0) notify.success('Pending settings applied on every machine')
        if (readPending().length === 0) onEmpty()
      })
      .catch(() => undefined)
      .finally(() => (busy.current = false))
    // eslint-disable-next-line react-hooks/exhaustive-deps -- run when a machine comes back or the timer ticks
  }, [back, tick])
  return null
}
