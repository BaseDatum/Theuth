import { useNavigate } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { INITIAL, type LinkFailure, failure } from '@/lib/link-status'
import { cn } from '@/lib/utils'
import { LOCAL, useEngines } from '@/link/engines'

/** The named failure of one link engine's socket, re-evaluated every second while it is not connected (parked/backoff counters). */
export function useLinkFailure(id: string): LinkFailure | null {
  const state = useEngines((s) => s.link[id]) ?? INITIAL
  const engine = useEngines((s) => s.engines.find((h) => h.id === id))
  const [now, setNow] = useState(() => Date.now())
  const idle = state.phase === 'ready' || state.phase === 'idle'
  useEffect(() => {
    if (idle) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [idle])
  if (!engine || idle) return null
  return failure(state, now, { name: (engine.kind === 'mesh' && engine.meshName) || engine.name, relayHost: engine.relayHost })
}

/** `title` / `detail` / Re-pair for a failure; `compact` is the one-line form for list rows. */
export function FailureText({ failure: f, onRepair, compact, className }: { failure: LinkFailure; onRepair: () => void; compact?: boolean; className?: string }) {
  const repair = f.action === 'repair' && (
    <Button variant={compact ? 'link' : 'secondary'} size="sm" className={compact ? 'h-auto p-0 text-[13px]' : 'min-h-11'} onClick={onRepair}>
      Re-pair
    </Button>
  )
  if (compact)
    return (
      <span className={cn('inline-flex flex-wrap items-baseline gap-x-2', className)}>
        <span className="text-warning">{f.title}.</span> {f.detail} {repair}
      </span>
    )
  return (
    <div className={cn('flex flex-col gap-2', className)}>
      <p className="text-fg-secondary">{f.title}</p>
      <p className="text-sm text-fg-secondary">{f.detail}</p>
      {repair && <div className="flex justify-end">{repair}</div>}
    </div>
  )
}

/** Strip under the chat header while the active link engine's socket is failing (the startup overlay only covers a first connect). */
export function LinkBanner() {
  const active = useEngines((s) => s.active)
  const navigate = useNavigate()
  const f = useLinkFailure(active)
  if (active === LOCAL || !f) return null
  return (
    <div role="status" className="mx-auto w-full shrink-0 px-4 py-1 text-xs md:max-w-[768px]">
      <FailureText compact failure={f} onRepair={() => void navigate({ to: '/link', search: { repair: active } })} />
    </div>
  )
}
