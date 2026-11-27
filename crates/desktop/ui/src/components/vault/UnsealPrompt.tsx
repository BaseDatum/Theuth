import { LockKeyholeOpen } from 'lucide-react'
import { useId, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { notify } from '@/lib/notify'
import { type UnsealRequestView, useVaultApi } from './api'

const CARD = 'flex flex-col gap-2 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm shadow-lg'

/** Runs before an approval goes out; `false` cancels it (a later biometric check wraps Unlock / Allow here). */
export type BeforeApprove = () => Promise<boolean>

export function unsealTitle(r: UnsealRequestView): string {
  return r.purpose === 'enroll' ? `${r.node_name} wants this device to unlock its vault` : `${r.node_name} restarted — unlock its vault?`
}

/**
 * 225: one sealed node asking this (owner) device to release its vault key, or (`enroll`) to become one of its unlock
 * devices. A human press only: `vault_answer_unseal(id, approve, always)`; `always` lets this device answer that
 * node's later asks by itself. `onDone` drops the card (answered, or found already answered or expired).
 */
export function UnsealPrompt({ request, beforeApprove, onDone }: { request: UnsealRequestView; beforeApprove?: BeforeApprove; onDone?: (id: string) => void }) {
  const api = useVaultApi()
  const [always, setAlways] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const switchId = useId()
  const enroll = request.purpose === 'enroll'

  const answer = async (approve: boolean) => {
    setBusy(true)
    setError(null)
    try {
      if (approve && beforeApprove && !(await beforeApprove())) return
      const ok = await api.answerUnseal(request.id, approve, approve && always)
      if (!ok) notify.warning(`${request.node_name}'s request was already answered or expired`)
      onDone?.(request.id)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className={CARD} role="group" aria-label={unsealTitle(request)}>
      <div className="flex items-start gap-2 font-medium text-info">
        <LockKeyholeOpen className="mt-0.5 size-4 shrink-0" aria-hidden />
        <span>{unsealTitle(request)}</span>
      </div>
      <div className="text-xs text-fg-secondary">
        {enroll
          ? `Allow it, and after a restart ${request.node_name} can ask this device for its vault key.`
          : `Its secrets stay locked until a device you own sends its vault key, end-to-end encrypted.`}
      </div>
      <label htmlFor={switchId} className="flex items-center gap-2 text-xs">
        <Switch id={switchId} size="sm" checked={always} disabled={busy} onCheckedChange={setAlways} />
        Always unlock automatically from this device
      </label>
      {error && <div className="text-xs text-danger">{error}</div>}
      <div className="flex gap-2">
        <Button size="sm" disabled={busy} onClick={() => void answer(true)}>
          {enroll ? 'Allow' : 'Unlock'}
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer(false)}>
          Not now
        </Button>
      </div>
    </div>
  )
}
