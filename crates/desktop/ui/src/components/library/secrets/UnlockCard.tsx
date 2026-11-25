import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useSecretsApi } from './api'
import type { LoginForm, LoginStep, UnlockRequested } from './types'

const CARD = 'flex flex-col gap-2 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm'

/**
 * §9.4 unlock card: the master password in a masked field, kept in this component's state only and cleared after the
 * answer. `onUnlock` runs `connection_unlock` (the Secrets page, the add stepper, a request card's locked destination)
 * or `answer_unlock` (an `UnlockRequested` from an agent's call); `onCancel` declines.
 */
export function UnlockCard({
  conn,
  account,
  reason,
  onUnlock,
  onCancel,
}: {
  conn: string
  account: string
  reason?: string | null
  onUnlock: (password: string) => Promise<unknown>
  onCancel?: () => void
}) {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      await onUnlock(password)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setPassword('')
      setBusy(false)
    }
  }
  return (
    <form
      className={CARD}
      onSubmit={(e) => {
        e.preventDefault()
        if (password) void submit()
      }}
    >
      <div className="font-medium text-info">
        Unlock bw/{conn} <span className="font-normal text-fg-secondary">({account})</span>
      </div>
      {reason && <div className="text-xs text-fg-secondary">needed for <span className="font-mono">{reason}</span></div>}
      <Input type="password" autoComplete="off" aria-label={`master password for ${conn}`} placeholder="Master password" value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy} />
      {error && <div className="text-xs text-danger">{error}</div>}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={busy || !password}>
          Unlock
        </Button>
        {onCancel && (
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
        )}
        <span className="self-center text-xs text-fg-tertiary">the master password is never stored</span>
      </div>
    </form>
  )
}

/** §9.4: an agent's call needs a locked connection; answered with `answer_unlock(id, password | null)`. */
export function UnlockRequestCard({ request, onDone }: { request: UnlockRequested; onDone?: () => void }) {
  const api = useSecretsApi()
  return (
    <UnlockCard
      conn={request.conn}
      account={request.account}
      reason={request.reason}
      onUnlock={async (pw) => {
        if (!(await api.answer_unlock(request.id, pw))) throw new Error('the unlock request was already answered')
        onDone?.()
      }}
      onCancel={() => void api.answer_unlock(request.id, null).finally(() => onDone?.())}
    />
  )
}

type Method = LoginForm['method']

/**
 * §6.2 / §9.4 login card: API key (recommended; `client_id` / `client_secret`, Account settings › Security › Keys) or
 * email + master password, growing a 2FA or new-device step when `connection_login` answers one. Every value stays in
 * this component's state and is cleared once the login is done.
 */
export function LoginCard({ conn, email: initialEmail, method: initialMethod, onDone }: { conn: string; email: string; method: Method; onDone: () => void }) {
  const api = useSecretsApi()
  const [method, setMethod] = useState<Method>(initialMethod)
  const [f, setF] = useState({ client_id: '', client_secret: '', email: initialEmail, password: '', provider: '', code: '', otp: '' })
  const [step, setStep] = useState<LoginStep | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value })
  const form = (): LoginForm =>
    method === 'api_key'
      ? { method, client_id: f.client_id.trim(), client_secret: f.client_secret }
      : {
          method,
          email: f.email.trim(),
          password: f.password,
          two_factor: step?.step === 'need_two_factor' && f.code ? [f.provider || (step.providers[0] ?? ''), f.code.trim()] : null,
          new_device_otp: step?.step === 'need_new_device_otp' && f.otp ? f.otp.trim() : null,
        }
  const ready = method === 'api_key' ? f.client_id.trim() && f.client_secret : f.email.trim() && f.password
  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const next = await api.connection_login(conn, form())
      if (next.step === 'done') {
        setF({ client_id: '', client_secret: '', email: '', password: '', provider: '', code: '', otp: '' })
        onDone()
      } else setStep(next)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <form
      className={CARD}
      onSubmit={(e) => {
        e.preventDefault()
        if (ready) void submit()
      }}
    >
      <div className="font-medium text-info">Log in bw/{conn}</div>
      <ToggleGroup type="single" variant="outline" size="sm" value={method} aria-label="Login method" onValueChange={(m) => m && setMethod(m as Method)}>
        <ToggleGroupItem value="api_key">API key (recommended)</ToggleGroupItem>
        <ToggleGroupItem value="password">Password</ToggleGroupItem>
      </ToggleGroup>
      {method === 'api_key' ? (
        <>
          <Input autoComplete="off" aria-label="client_id" placeholder="client_id (user.…)" value={f.client_id} onChange={set('client_id')} disabled={busy} spellCheck={false} />
          <Input type="password" autoComplete="off" aria-label="client_secret" placeholder="client_secret" value={f.client_secret} onChange={set('client_secret')} disabled={busy} />
          <div className="text-xs text-fg-tertiary">Bitwarden › Account settings › Security › Keys › View API key. No 2FA prompt applies.</div>
        </>
      ) : (
        <>
          <Input autoComplete="off" aria-label="email" placeholder="Email" value={f.email} onChange={set('email')} disabled={busy} spellCheck={false} />
          <Input type="password" autoComplete="off" aria-label="master password" placeholder="Master password" value={f.password} onChange={set('password')} disabled={busy} />
          {step?.step === 'need_two_factor' && (
            <div className="flex gap-2">
              <select aria-label="two-factor method" className="h-8 rounded-md border border-border-subtle bg-transparent px-1.5 text-xs" value={f.provider || step.providers[0]} onChange={set('provider')}>
                {step.providers.map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
              <Input autoComplete="one-time-code" aria-label="two-factor code" placeholder="Code" value={f.code} onChange={set('code')} disabled={busy} />
            </div>
          )}
          {step?.step === 'need_new_device_otp' && <Input autoComplete="one-time-code" aria-label="new device code" placeholder="Code Bitwarden emailed for this new device" value={f.otp} onChange={set('otp')} disabled={busy} />}
          <div className="text-xs text-fg-tertiary">Duo and WebAuthn need a browser and are not offered; use an API key instead.</div>
        </>
      )}
      {error && <div className="text-xs text-danger">{error}</div>}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={busy || !ready}>
          {step ? 'Continue' : 'Log in'}
        </Button>
      </div>
    </form>
  )
}
