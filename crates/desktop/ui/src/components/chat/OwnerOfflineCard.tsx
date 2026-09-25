import { Unplug } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { lastSeenText, ownerOfflineText, useEngineLabel } from '@/lib/owner'
import { useApi } from '@/lib/rpc'

/** What a refused send left behind (68 §5): the unreachable owner and the input to resend after a take. */
export type Offline = { owner: string; lastSeen: string | number | null; body: string; files: string[] }

/**
 * Above the composer after `OwnerUnreachable` (68 §5): "Owned by <engine>, last seen <ago>. Continue here now?"
 * Continue = `session_take(id, "user")` then the input is sent again here; Wait dismisses and keeps the draft.
 */
export function OwnerOfflineCard({ session, offline, onDone }: { session: string; offline: Offline; onDone(): void }) {
  const api = useApi()
  const engine = useEngineLabel(offline.owner)
  const [busy, setBusy] = useState(false)
  const take = async () => {
    setBusy(true)
    try {
      await api.session_take(session, 'user')
      await api.send_input(session, offline.body, offline.files)
      onDone()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div
      role="alert"
      className="mb-2 flex items-start gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-2 text-sm"
    >
      <Unplug className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden="true" />
      <span className="min-w-0 flex-1 text-fg-secondary">{ownerOfflineText(engine, lastSeenText(offline.lastSeen))}</span>
      <Button size="sm" variant="outline" disabled={busy} onClick={onDone}>
        Wait
      </Button>
      <Button size="sm" disabled={busy} onClick={() => void take()}>
        Continue here
      </Button>
    </div>
  )
}
