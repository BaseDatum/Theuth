import { Unplug } from 'lucide-react'
import { useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { type OwnerUnreachable, lastSeenText, ownerMidTurnText, ownerOfflineText, useEngineLabel } from '@/lib/owner'
import { useApi } from '@/lib/rpc'
import { useSessionField } from '@/store/sessions'

/**
 * Above the composer after `OwnerUnreachable` (68 §5): "Owned by <engine>, last seen <ago>. Continue here now?"
 * Continue = `session_take(id, "user")` then `onResend` (the composer's retry: the input is still in its box, flagged
 * unsent, so a second failure keeps it there); Wait dismisses and keeps the draft. 139 §9.3: when the owner was
 * mid-turn the body says the take resumes it here and what that risks; the engine does the resume (D5).
 */
export function OwnerOfflineCard({
  session,
  offline,
  onDone,
  onResend,
}: {
  session: string
  offline: OwnerUnreachable
  onDone(): void
  onResend(): Promise<void>
}) {
  const api = useApi()
  const engine = useEngineLabel(offline.owner)
  const resume = useSessionField(session, 'resume')
  const [busy, setBusy] = useState(false)
  const take = async () => {
    setBusy(true)
    try {
      await api.session_take(session, 'user')
      onDone()
      await onResend()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  // 139 §9.3 (D5): `Loaded.resume` on a replica names the owner's open turn; the `user` take resumes it here.
  const midTurn = !!resume && !resume.here
  return (
    <div
      role="alert"
      className="mb-2 flex items-start gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-2 text-sm"
    >
      <Unplug className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden="true" />
      <span className="min-w-0 flex-1 text-fg-secondary">
        {ownerOfflineText(engine, lastSeenText(offline.lastSeen))}
        {midTurn && ` ${ownerMidTurnText(engine)}`}
      </span>
      <Button size="sm" variant="outline" disabled={busy} onClick={onDone}>
        Wait
      </Button>
      <Button size="sm" disabled={busy} onClick={() => void take()}>
        {midTurn ? 'Continue here and resume' : 'Continue here'}
      </Button>
    </div>
  )
}
