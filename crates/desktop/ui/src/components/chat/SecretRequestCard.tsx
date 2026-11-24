import { memo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useApi } from '@/lib/rpc'
import type { SecretRequestField } from "@/rpc/bindings/method.secrets.output"
import { useSecretRequests } from '@/store/approvals'
import { choiceFromSuggestion, type ExpiryChoice, type ExpiryUnit, formatExpiry, msToLocalInput, resolveExpiry } from './secret-expiry'

/**
 * The positional `answer_secret(id, value, values, expires_ms)` tail: `[value, null, at]` for one field,
 * `[null, { name: value }, at]` for several, `[null, null, null]` to decline. `expiresMs` (absolute Unix ms) applies
 * to every value stored from the card; `null` = never.
 */
export function answerPayload(
  fields: readonly SecretRequestField[],
  values: Record<string, string>,
  decline: boolean,
  expiresMs: number | null = null,
): [string | null, Record<string, string> | null, number | null] {
  if (decline) return [null, null, null]
  const one = fields.length === 1 ? fields[0] : null
  if (one) return [values[one.name] ?? '', null, expiresMs]
  return [null, Object.fromEntries(fields.map((f) => [f.name, values[f.name] ?? ''])), expiresMs]
}

/** A local `SecretRequested`'s fields: every field, or — from an engine older than forms — the one `name`. */
export function eventFields(p: { name: string; current: number | null; fields?: SecretRequestField[] | null }): SecretRequestField[] {
  return p.fields?.length ? p.fields : [{ name: p.name, label: p.name, masked: true, current: p.current }]
}

const UNITS: ExpiryUnit[] = ['minutes', 'hours', 'days']

/** Never | In [n] [unit] | On [local date-time], and the absolute time it resolves to (or why it does not). */
function ExpiryControl({ choice, onChange, disabled, now }: { choice: ExpiryChoice; onChange: (c: ExpiryChoice) => void; disabled: boolean; now: number }) {
  const resolved = resolveExpiry(choice, now)
  const mode = (m: string) => {
    if (m === 'never') onChange({ mode: 'never' })
    else if (m === 'in') onChange({ mode: 'in', amount: '1', unit: 'hours' })
    else if (m === 'on') onChange({ mode: 'on', local: msToLocalInput(now + 86_400_000) })
  }
  return (
    <fieldset className="flex flex-col gap-1.5" disabled={disabled}>
      <legend className="mb-1 text-xs text-fg-secondary">Expires</legend>
      <div className="flex flex-wrap items-center gap-2">
        <ToggleGroup type="single" variant="outline" size="sm" value={choice.mode} aria-label="Expiry" onValueChange={(m) => m && m !== choice.mode && mode(m)}>
          <ToggleGroupItem value="never">Never</ToggleGroupItem>
          <ToggleGroupItem value="in">In</ToggleGroupItem>
          <ToggleGroupItem value="on">On</ToggleGroupItem>
        </ToggleGroup>
        {choice.mode === 'in' && (
          <>
            <Input
              type="number"
              min={1}
              step={1}
              aria-label="expires in"
              className="h-7 w-20"
              value={choice.amount}
              onChange={(e) => onChange({ ...choice, amount: e.target.value })}
            />
            <select
              aria-label="unit"
              className="h-7 rounded-md border border-border-subtle bg-transparent px-1.5 text-xs"
              value={choice.unit}
              onChange={(e) => onChange({ ...choice, unit: e.target.value as ExpiryUnit })}
            >
              {UNITS.map((u) => (
                <option key={u} value={u}>
                  {u}
                </option>
              ))}
            </select>
          </>
        )}
        {choice.mode === 'on' && (
          <Input
            type="datetime-local"
            aria-label="expires on"
            className="h-7 w-auto"
            value={choice.local}
            onChange={(e) => onChange({ mode: 'on', local: e.target.value })}
          />
        )}
      </div>
      <div className={`text-xs ${resolved.ok ? 'text-fg-tertiary' : 'text-danger'}`} aria-live="polite">
        {resolved.ok
          ? resolved.at === null
            ? 'Kept until you delete it'
            : `Expires ${formatExpiry(resolved.at, now)} — then deleted`
          : resolved.error}
      </div>
    </fieldset>
  )
}

/**
 * `SecretsEvent.SecretRequested` (37 §6) and the replicated `secret_requests` row (79 §6.6, contract §2.7): the model
 * asked for one or several secrets; the user types them into masked fields and may set one expiry for all of them
 * (`expires`, the agent's suggestion, only pre-fills it). Store → `answer_secret(id, value, null, at)` for a single
 * field (37's wire), `answer_secret(id, null, { name: value }, at)` for several (79 H's `values`); Decline →
 * `answer_secret(id, null, null, null)`. `SecretResolved` drops the card; the tool row's result line is the receipt.
 * Values live only in this component's state. `origin` names where the request was raised (a VM's name, 79 §6.6);
 * `onAnswered` fires after a successful answer (the inbox refetches its rows). `onError` (the inbox, which never
 * toasts) takes a failure, or "already answered", instead of the toast.
 */
export const SecretRequestCard = memo(function SecretRequestCard({
  id,
  title,
  note,
  fields,
  expires,
  origin,
  onAnswered,
  onError,
}: {
  id: string
  title?: string | null
  note?: string
  fields: SecretRequestField[]
  expires?: string | null
  origin?: string | null
  onAnswered?: () => void
  onError?: (error: string) => void
}) {
  const api = useApi()
  const [values, setValues] = useState<Record<string, string>>({})
  const [expiry, setExpiry] = useState<ExpiryChoice>(() => choiceFromSuggestion(expires))
  const [busy, setBusy] = useState(false)
  const now = Date.now()
  const resolved = resolveExpiry(expiry, now)
  const complete = fields.every((f) => values[f.name]) && resolved.ok
  const one = fields.length === 1 ? fields[0] : null
  const answer = async (decline: boolean) => {
    // "In 2 hours" counts from the moment the user stores.
    const at = resolveExpiry(expiry, Date.now())
    if (!decline && !at.ok) return
    setBusy(true)
    try {
      const [value, form, expiresMs] = answerPayload(fields, values, decline, at.ok ? at.at : null)
      if (await api.answer_secret(id, value, form, expiresMs)) {
        setValues({})
        onAnswered?.()
      } else {
        // Answered elsewhere and the `SecretResolved` was missed: the tool row's result is the receipt.
        useSecretRequests.getState().resolve(id)
        ;(onError ?? toast.error)('the secret request was already answered')
      }
    } catch (e) {
      ;(onError ?? toast.error)(e instanceof Error ? e.message : String(e))
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
              {f.note && f.note !== f.label && <span className="text-fg-tertiary"> · {f.note}</span>}
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
      <ExpiryControl choice={expiry} onChange={setExpiry} disabled={busy} now={now} />
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
