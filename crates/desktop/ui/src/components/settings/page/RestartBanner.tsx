import { useQuery } from '@tanstack/react-query'
import { LoaderCircle } from 'lucide-react'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { LOCAL, useEngines } from '@/link/engines'
import { useRestartPending } from '@/lib/restart-pending'
import { connection, isTauri, relaunch } from '@/lib/tauri'

/**
 * 188 S6: whether Restart theuth can work here. Only the desktop app on its own engine, and only when it started that
 * daemon (`Connection.owned`): `relaunch` stops just a daemon it owns, so on a reused one (a service install, a
 * terminal `t --serve`) it would reconnect to the same process and apply nothing.
 */
function useCanRestart(): boolean {
  const active = useEngines((s) => s.active)
  const local = isTauri() && active === LOCAL
  // The shell caches the connection after boot, so this reads it back without probing or spawning.
  const owned = useQuery({ queryKey: ['shell-connection'], queryFn: connection, enabled: local, staleTime: Number.POSITIVE_INFINITY, select: (c) => c.owned })
  return local && owned.data === true
}

/** 188 S6: one line for every saved change that waits for a restart, with the restart where the app can do it. */
export function RestartBanner() {
  const pending = useRestartPending((s) => s.pending)
  const canRestart = useCanRestart()
  // The new daemon listens on a new port that this window's client does not know: reload, and the app boots on the
  // shell's fresh connection, re-reading every row and starting with nothing pending.
  const restart = useAction(async () => {
    await relaunch()
    location.reload()
  })
  if (pending.length === 0) return null
  const restarting = restart.isPending || restart.isSuccess
  return (
    <div role="status" className="mb-4 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-[10px] border border-border-subtle bg-layer-2 px-3 py-2 text-sm">
      <p className="min-w-0 flex-1">
        {pending.length === 1 ? '1 change applies' : `${pending.length} changes apply`} after theuth restarts: {pending.map((p) => p.label).join(', ')}.
        {!canRestart && <span className="block text-fg-secondary">Restart theuth where it runs to apply them.</span>}
      </p>
      {canRestart && (
        <Button variant="outline" size="sm" disabled={restarting} onClick={() => restart.mutate(undefined)}>
          {restarting && <LoaderCircle className="animate-spin" />} {restarting ? 'Restarting…' : 'Restart theuth'}
        </Button>
      )}
    </div>
  )
}
