import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { notify } from '@/lib/notify'
import { isDesktop } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import type { LoginForm } from '@/rpc/bindings/method.secrets.input'
import type { LoginStep, UnlockRequestView } from '@/rpc/bindings/method.secrets.output'
import { type SsoDeps, useSsoLogin } from './sso'

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
      <Input
        type="password"
        autoComplete="off"
        aria-label={`master password for ${conn}`}
        placeholder="Master password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        disabled={busy}
      />
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

/**
 * §9.4: an agent's call needs a locked connection; answered with `answer_unlock(id, password | null)`. The card goes
 * once answered, declined, or found already answered (`false`: answered elsewhere, timed out, its `Resolved` missed).
 */
export function UnlockRequestCard({ request, onDone }: { request: UnlockRequestView; onDone?: () => void }) {
  const api = useApi()
  return (
    <UnlockCard
      conn={request.conn}
      account={request.account}
      reason={request.reason}
      onUnlock={async (pw) => {
        const ok = await api.answer_unlock(request.id, pw)
        onDone?.()
        if (!ok) notify.warning(`The unlock request for bw/${request.conn} was already answered`)
      }}
      onCancel={() =>
        void api
          .answer_unlock(request.id, null)
          .catch((e: unknown) => notify.error(errorText(e)))
          .finally(() => onDone?.())
      }
    />
  )
}

/** `connection_login`'s two methods and §9.6's SSO, which runs its own flow (`sso.ts`). */
export type LoginMethod = LoginForm['method'] | 'sso'

const CLEARED = { client_id: '', client_secret: '', email: '', password: '', provider: '', code: '', otp: '', org: '' }

/**
 * §6.2 / §9.4 / §9.6 login card: API key (recommended; `client_id` / `client_secret`, Account settings › Security ›
 * Keys), email + master password, or SSO (desktop: the system browser and the shell's loopback listener), growing a 2FA
 * or new-device step when the engine answers one. Every value stays in this component's state and is cleared once the
 * login is done; engine errors show verbatim.
 */
export function LoginCard({
  conn,
  email: initialEmail,
  method: initialMethod,
  org: initialOrg = null,
  onDone,
  onCancel,
  ssoDeps,
}: {
  conn: string
  email: string
  method: LoginMethod
  /** The connection's SSO org identifier, when known. */
  org?: string | null
  onDone: () => void
  onCancel?: () => void
  /** Tests: the shell and engine calls of the SSO flow. */
  ssoDeps?: SsoDeps
}) {
  const api = useApi()
  const [method, setMethod] = useState<LoginMethod>(initialMethod)
  const [f, setF] = useState({ ...CLEARED, email: initialEmail, org: initialOrg ?? '' })
  const [step, setStep] = useState<LoginStep | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const finish = () => {
    setF(CLEARED)
    onDone()
  }
  const sso = useSsoLogin(conn, finish, ssoDeps)
  // SSO needs the shell's loopback listener, so a desktop app (ssoDeps: tests).
  const ssoHere = isDesktop() || ssoDeps !== undefined
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value })
  const ssoRunning = sso.state.phase === 'browser' || sso.state.phase === 'completing'
  const ssoTwoFactor = sso.state.phase === 'two_factor' ? sso.state.providers : null
  // The 2FA step of either flow: its providers.
  const providers = method === 'sso' ? ssoTwoFactor : step?.step === 'need_two_factor' ? step.providers : null
  const provider = f.provider || (providers?.[0] ?? '')
  const form = (): LoginForm =>
    method === 'api_key'
      ? { method, client_id: f.client_id.trim(), client_secret: f.client_secret }
      : {
          method: 'password',
          email: f.email.trim(),
          password: f.password,
          two_factor: providers && f.code ? [provider, f.code.trim()] : null,
          new_device_otp: step?.step === 'need_new_device_otp' && f.otp ? f.otp.trim() : null,
        }
  // A 2FA or new-device step needs its code before Continue.
  const stepReady = providers ? f.code.trim() : step?.step === 'need_new_device_otp' ? f.otp.trim() : true
  const ready =
    method === 'sso' ? !ssoRunning && ssoHere && (!ssoTwoFactor || f.code.trim()) : method === 'api_key' ? f.client_id.trim() && f.client_secret : f.email.trim() && f.password && stepReady
  const submit = async () => {
    if (method === 'sso') {
      if (ssoTwoFactor) await sso.twoFactor(provider, f.code.trim())
      else await sso.start(f.org.trim() || null)
      return
    }
    setBusy(true)
    setError(null)
    try {
      const next = await api.connection_login(conn, form())
      if (next.step === 'done') finish()
      else setStep(next)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }
  const shownError = method === 'sso' ? sso.error : error
  return (
    <form
      className={CARD}
      onSubmit={(e) => {
        e.preventDefault()
        if (ready) void submit()
      }}
    >
      <div className="font-medium text-info">Log in bw/{conn}</div>
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={method}
        aria-label="Login method"
        disabled={busy || ssoRunning}
        onValueChange={(m) => {
          if (!m) return
          if (method === 'sso') sso.cancel()
          setMethod(m as LoginMethod)
          setStep(null)
        }}
      >
        <ToggleGroupItem value="api_key">API key (recommended)</ToggleGroupItem>
        <ToggleGroupItem value="password">Password</ToggleGroupItem>
        <ToggleGroupItem value="sso">Log in with SSO</ToggleGroupItem>
      </ToggleGroup>
      {method === 'password' && <div className="text-xs text-fg-tertiary">{SSO_HINT}</div>}
      {method === 'api_key' && (
        <>
          <Input autoComplete="off" aria-label="client_id" placeholder="client_id (user.…)" value={f.client_id} onChange={set('client_id')} disabled={busy} spellCheck={false} />
          <Input
            type="password"
            autoComplete="off"
            aria-label="client_secret"
            placeholder="client_secret"
            value={f.client_secret}
            onChange={set('client_secret')}
            disabled={busy}
          />
          <div className="text-xs text-fg-tertiary">Bitwarden › Account settings › Security › Keys › View API key. No 2FA prompt applies.</div>
        </>
      )}
      {method === 'password' && (
        <>
          <Input autoComplete="off" aria-label="email" placeholder="Email" value={f.email} onChange={set('email')} disabled={busy} spellCheck={false} />
          <Input type="password" autoComplete="off" aria-label="master password" placeholder="Master password" value={f.password} onChange={set('password')} disabled={busy} />
        </>
      )}
      {method === 'sso' &&
        (!ssoHere ? (
          <div className="text-xs text-fg-secondary">SSO login runs in the desktop app or the TUI, where the browser can hand the answer back.</div>
        ) : sso.state.phase === 'idle' ? (
          <>
            <Input
              autoComplete="off"
              aria-label="SSO organisation identifier"
              placeholder="Organisation SSO identifier (optional on Vaultwarden)"
              value={f.org}
              onChange={set('org')}
              spellCheck={false}
            />
            <div className="text-xs text-fg-tertiary">Your browser opens your organisation's sign-in; the master password is asked here afterwards.</div>
          </>
        ) : sso.state.phase === 'browser' ? (
          <div role="status" className="text-fg-secondary">
            Finish signing in in your browser…
          </div>
        ) : sso.state.phase === 'completing' ? (
          <div role="status" className="text-fg-secondary">
            Signing in…
          </div>
        ) : null)}
      {providers && (
        <div className="flex gap-2">
          <select aria-label="two-factor method" className="h-8 rounded-md border border-border-subtle bg-transparent px-1.5 text-xs" value={provider} onChange={set('provider')}>
            {providers.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <Input autoComplete="one-time-code" aria-label="two-factor code" placeholder="Code" value={f.code} onChange={set('code')} disabled={busy} />
        </div>
      )}
      {method === 'password' && step?.step === 'need_new_device_otp' && (
        <Input
          autoComplete="one-time-code"
          aria-label="new device code"
          placeholder="Code Bitwarden emailed for this new device"
          value={f.otp}
          onChange={set('otp')}
          disabled={busy}
        />
      )}
      {method === 'password' && <div className="text-xs text-fg-tertiary">Duo and WebAuthn need a browser and are not offered; use an API key instead.</div>}
      {shownError && <div className="text-xs text-danger">{shownError}</div>}
      <div className="flex gap-2">
        {!ssoRunning && (
          <Button type="submit" size="sm" disabled={busy || !ready}>
            {providers || step ? 'Continue' : method === 'sso' ? 'Log in with SSO' : 'Log in'}
          </Button>
        )}
        {(ssoRunning || ssoTwoFactor) && (
          <Button type="button" size="sm" variant="outline" onClick={sso.cancel}>
            Cancel
          </Button>
        )}
        {!ssoRunning && !ssoTwoFactor && onCancel && (
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </form>
  )
}

/** §9.6 "Both surfaces": under the password option of every Bitwarden login chooser. */
export const SSO_HINT = 'Your organisation requires SSO? Use Log in with SSO or API key.'
