import { memo, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useApi } from '@/lib/rpc'
import type { SecretRequestField } from '@/rpc/bindings/SecretRequestField'
import { useSecretRequests } from '@/store/approvals'

/**
 * The positional `answer_secret(id, value, values)` tail: `[value, null]` for one field, `[null, { name: value }]` for a
 * form, `[null, null]` to decline.
 */
export function answerPayload(fields: readonly SecretRequestField[], values: Record<string, string>, decline: boolean): [string | null, Record<string, string> | null] {
  if (decline) return [null, null]
  const one = fields.length === 1 ? fields[0] : null
  if (one) return [values[one.name] ?? '', null]
  return [null, Object.fromEntries(fields.map((f) => [f.name, values[f.name] ?? '']))]
}

/**
 * `SecretsEvent.SecretRequested` (37 §6) and the replicated `secret_requests` row (79 §6.6, contract §2.7): the model
 * asked for one or several secrets; the user types them into masked fields. Store → `answer_secret(id, value, null)` for a
 * single field (37's wire), `answer_secret(id, null, { name: value })` for a form (79 H's `values`); Decline →
 * `answer_secret(id, null, null)`. `SecretResolved` drops the card; the tool row's result line is the receipt. Values live
 * only in this component's state. `origin` names where the request was raised (a VM's name, 79 §6.6); `onAnswered`
 * fires after a successful answer (the inbox refetches its rows).
 */
export const SecretRequestCard = memo(function SecretRequestCard({ id, title, note, fields, origin, onAnswered }: { id: string; title?: string | null; note?: string; fields: SecretRequestField[]; origin?: string | null; onAnswered?: () => void }) {
  const api = useApi()
  const [values, setValues] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const complete = fields.every((f) => values[f.name])
  const one = fields.length === 1 ? fields[0] : null
  const answer = async (decline: boolean) => {
    setBusy(true)
    try {
      const [value, form] = answerPayload(fields, values, decline)
      if (await api.answer_secret(id, value, form)) {
        setValues({})
        onAnswered?.()
      } else {
        // Answered elsewhere and the `SecretResolved` was missed: the tool row's result is the receipt.
        useSecretRequests.getState().resolve(id)
        toast.error('the secret request was already answered')
      }
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
        if (complete) void answer(false)
      }}
    >
      <div>
        <div className="font-medium text-info">
          {title ?? 'secret requested'}
          {one && (
            <>
              : <span className="font-mono">{one.name}</span>
            </>
          )}
          {origin && <span className="ml-1 text-xs font-normal text-fg-tertiary">· on {origin}</span>}
        </div>
        {note && <div className="mt-0.5 text-fg-secondary">{note}</div>}
        {one && one.current !== null && (
          <div className="mt-0.5 text-xs text-fg-tertiary">
            adds v{one.current + 1}; v{one.current} stays as <span className="font-mono">{`{{secret:${one.name}:-1}}`}</span>
          </div>
        )}
      </div>
      {fields.map((f) => (
        <label key={f.name} className="flex flex-col gap-1">
          {!one && (
            <span className="text-xs text-fg-secondary">
              {f.label} <span className="font-mono text-fg-tertiary">{f.name}</span>
              {f.current !== null && ` · adds v${f.current + 1}`}
            </span>
          )}
          <Input
            type={f.masked ? 'password' : 'text'}
            autoComplete="off"
            aria-label={`value of ${f.name}`}
            placeholder={one ? 'Paste or type the value' : f.label}
            value={values[f.name] ?? ''}
            onChange={(e) => setValues((v) => ({ ...v, [f.name]: e.target.value }))}
            disabled={busy}
          />
        </label>
      ))}
      <div className="flex gap-2">
        <Button type="submit" size="sm" className="pointer-coarse:h-11 pointer-coarse:flex-1" disabled={busy || !complete}>
          Store
        </Button>
        <Button type="button" size="sm" variant="outline" className="pointer-coarse:h-11 pointer-coarse:flex-1" disabled={busy} onClick={() => void answer(true)}>
          Decline
        </Button>
        <span className="self-center text-xs text-fg-tertiary">{fields.length === 1 ? 'the value is stored encrypted and never shown' : 'the values are stored encrypted and never shown'}</span>
      </div>
    </form>
  )
})
