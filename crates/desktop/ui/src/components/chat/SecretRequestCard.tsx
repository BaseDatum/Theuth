import { memo, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useApi } from '@/lib/rpc'

/**
 * `SecretsEvent.SecretRequested` (37 §6): the model asked for a secret; the user types it
 * into a masked field. Store → `answer_secret(id, value)`, Decline → `answer_secret(id, null)`.
 * `SecretResolved` drops the card (the pending store), and the `secret_request` tool row's
 * result line is the receipt. The value lives only in this component's state.
 */
export const SecretRequestCard = memo(function SecretRequestCard({
  id,
  name,
  note,
  current,
}: {
  id: string
  name: string
  note: string
  current: number | null
}) {
  const api = useApi()
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const answer = async (v: string | null) => {
    setBusy(true)
    try {
      await api.answer_secret(id, v)
      setValue('')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <form
      className="my-2 flex flex-col gap-3 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm"
      onSubmit={(e) => {
        e.preventDefault()
        if (value) void answer(value)
      }}
    >
      <div>
        <div className="font-medium text-info">
          secret requested: <span className="font-mono">{name}</span>
        </div>
        {note && <div className="mt-0.5 text-fg-secondary">{note}</div>}
        {current !== null && (
          <div className="mt-0.5 text-xs text-fg-tertiary">
            adds v{current + 1}; v{current} stays as{' '}
            <span className="font-mono">{`{{secret:${name}:-1}}`}</span>
          </div>
        )}
      </div>
      <Input
        type="password"
        autoComplete="off"
        aria-label={`value of ${name}`}
        placeholder="Paste or type the value"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        disabled={busy}
      />
      <div className="flex gap-2">
        <Button type="submit" size="sm" className="pointer-coarse:h-11 pointer-coarse:flex-1" disabled={busy || !value}>
          Store
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="pointer-coarse:h-11 pointer-coarse:flex-1"
          disabled={busy}
          onClick={() => void answer(null)}
        >
          Decline
        </Button>
        <span className="self-center text-xs text-fg-tertiary">the value is stored encrypted and never shown</span>
      </div>
    </form>
  )
})
