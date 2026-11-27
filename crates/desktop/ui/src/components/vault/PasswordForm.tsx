import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import { MIN_PASSWORD, strength } from './strength'

const BAR = ['bg-danger', 'bg-danger', 'bg-warning', 'bg-success', 'bg-success'] as const

/** Four bars, the label and a hint for the password being typed. */
export function StrengthMeter({ password }: { password: string }) {
  const s = strength(password)
  return (
    <div className="flex flex-col gap-1" aria-live="polite">
      <div className="flex gap-1" aria-hidden>
        {[1, 2, 3, 4].map((n) => (
          <div key={n} className={cn('h-1 flex-1 rounded-full', password && n <= Math.max(s.score, 1) ? BAR[s.score] : 'bg-border-subtle')} />
        ))}
      </div>
      {password && (
        <div className="text-xs text-fg-secondary">
          <span className="font-medium">{s.label}</span> · {s.hint}
        </div>
      )}
    </div>
  )
}

/**
 * 225: Add password (new + confirmation) or Change password (current + new + confirmation). Refuses a short or
 * mismatched new password inline without calling `onSubmit`; the fields are cleared once `onSubmit` settles.
 * `onSubmit` reports its own failures (settings actions notify); `onDone` runs after a success.
 */
export function PasswordForm({
  mode,
  name,
  onSubmit,
  onDone,
  onCancel,
}: {
  mode: 'add' | 'change'
  /** The machine, for the field labels. */
  name: string
  onSubmit: (next: string, current: string) => Promise<unknown>
  onDone?: () => void
  onCancel?: () => void
}) {
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    if (mode === 'change' && !current) return setError('Enter the current password')
    if ([...next].length < MIN_PASSWORD) return setError(`Use at least ${MIN_PASSWORD} characters`)
    if (next !== confirm) {
      setConfirm('')
      return setError('The passwords do not match')
    }
    setError(null)
    setBusy(true)
    try {
      await onSubmit(next, current)
      onDone?.()
    } catch {
      // Reported by `onSubmit`'s action; the form stays open for another try.
    } finally {
      setCurrent('')
      setNext('')
      setConfirm('')
      setBusy(false)
    }
  }

  return (
    <form
      className="flex w-full max-w-md flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      {mode === 'change' && (
        <Input type="password" autoComplete="current-password" aria-label={`current password for ${name}`} placeholder="Current password" value={current} disabled={busy} onChange={(e) => setCurrent(e.target.value)} />
      )}
      <Input type="password" autoComplete="new-password" aria-label={`new password for ${name}`} placeholder={`New password (at least ${MIN_PASSWORD} characters)`} value={next} disabled={busy} onChange={(e) => setNext(e.target.value)} />
      <StrengthMeter password={next} />
      <Input type="password" autoComplete="new-password" aria-label={`confirm new password for ${name}`} placeholder="Confirm new password" value={confirm} disabled={busy} onChange={(e) => setConfirm(e.target.value)} />
      {error && (
        <div role="alert" className="text-xs text-danger">
          {error}
        </div>
      )}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={busy || !next || !confirm}>
          {mode === 'add' ? 'Add password' : 'Change password'}
        </Button>
        {onCancel && (
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </form>
  )
}
