import { useQuery } from '@tanstack/react-query'
import { Copy } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { type FormEvent, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { AllowList, useChannelEvents } from '@/components/settings/Channels'
import { Dot, Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PairingLink } from '@/rpc/bindings/PairingLink'
import type { TelegramEvent } from '@/rpc/bindings/TelegramEvent'
import type { TelegramStatus } from '@/rpc/bindings/TelegramStatus'
import { on } from '@/store/sessions'

/** One phrase per `TelegramEvent` variant (98 §10.5); a bot that is not polling is idle because another engine holds the hub lease. */
export function telegramState(s: TelegramStatus): string {
  const idle = s.bot && !s.polling ? ' · idle (hub elsewhere)' : ''
  return telegramPhrase(s) + idle
}

function telegramPhrase(s: TelegramStatus): string {
  const e = s.state
  if (!e) return 'unknown'
  switch (e.type) {
    case 'Unconfigured':
      return 'no bot token'
    case 'Identity':
      return `connected as @${e.bot} · topics ${e.topics ? 'on' : 'off'}`
    case 'Connected':
      return `connected as @${e.bot}${s.bot ? ` · topics ${s.bot.topics ? 'on' : 'off'}` : ''}`
    case 'Disconnected':
      return `disconnected: ${e.reason}`
    case 'Conflict':
      return 'another poller holds this token'
    case 'RateLimited':
      return `rate limited on ${e.method}, retry in ${e.retry_after_secs}s`
    case 'StreamFallback':
      return 'connected'
    case 'Unreachable':
      return `chat ${e.chat} unreachable: ${e.reason}`
    case 'PairingLink':
      return 'waiting for the owner to tap Start'
    case 'Paired':
      return `paired with ${e.name}`
    case 'DaemonOnly':
      return 'daemon only'
    case 'Starting':
      return 'starting…'
  }
}

/** A Telegram id typed by hand: digits only (a leading `-` for group chat ids), else `null`. */
export function telegramId(value: string, kind: 'user' | 'chat'): number | null {
  const v = value.trim()
  return (kind === 'chat' ? /^-?\d+$/ : /^\d+$/).test(v) ? Number(v) : null
}

/** Whole seconds until `untilMs` (unix ms), never negative. */
export function secondsLeft(untilMs: number, nowMs = Date.now()): number {
  return Math.max(0, Math.ceil((untilMs - nowMs) / 1000))
}

/** `m:ss`. */
export function mmss(secs: number): string {
  return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`
}

/** Seconds left until `untilMs`, re-read every second; 0 when there is nothing to count. */
function useCountdown(untilMs: number | null): number {
  const [secs, setSecs] = useState(() => (untilMs === null ? 0 : secondsLeft(untilMs)))
  useEffect(() => {
    if (untilMs === null) {
      setSecs(0)
      return
    }
    setSecs(secondsLeft(untilMs))
    const id = setInterval(() => setSecs(secondsLeft(untilMs)), 1000)
    return () => clearInterval(id)
  }, [untilMs])
  return secs
}

const TELEGRAM_LIVE: readonly TelegramEvent['type'][] = ['Connected', 'Identity', 'StreamFallback', 'Paired', 'PairingLink']

/** Rejects before the call when `value` is not an id; the rejection surfaces through `useAction`'s toast. */
function withId(kind: 'user' | 'chat', call: (id: number) => Promise<null>): (value: string) => Promise<null> {
  return (value) => {
    const id = telegramId(value, kind)
    return id === null ? Promise.reject(new Error(`${kind === 'user' ? 'User' : 'Chat'} ids are numeric: ${value}`)) : call(id)
  }
}

/** 188: Channels › Telegram: the bot, its token, the owner and pairing, allowed users and groups. */
export function Telegram() {
  const api = useApi()
  useChannelEvents(queryKeys.telegram)
  const status = useQuery({ queryKey: queryKeys.telegram, queryFn: () => api.telegram_status() })
  const keys = [queryKeys.telegram]
  const [token, setToken] = useState('')
  const [replacing, setReplacing] = useState(false)
  const [ownerId, setOwnerId] = useState('')
  const s = status.data
  const starting = s?.state?.type === 'Starting'
  const unconfigured = !starting && (!s?.state || s.state.type === 'Unconfigured')

  const saveToken = useAction((v: string) => api.set_secret('telegram_bot', v, 'Telegram bot token (@BotFather /newbot)'), {
    keys,
    onSuccess: () => {
      toast.success(unconfigured ? 'Token saved — starting the bot' : 'Token saved — applies at the next restart')
      setToken('')
      setReplacing(false)
    },
  })
  const link = useAction((owner: number | null) => api.telegram_link(owner), { keys, onSuccess: () => setOwnerId('') })
  const allow = useAction(withId('user', (id) => api.telegram_allow(id)), { keys })
  const remove = useAction((v: string) => api.telegram_remove_allowed(Number(v)), { keys })
  const allowGroup = useAction(withId('chat', (id) => api.telegram_allow_group(id)), { keys })
  const removeGroup = useAction((v: string) => api.telegram_remove_group(Number(v)), { keys })
  useEffect(() => on('Paired', () => link.reset()), [link.reset])

  const pairing: PairingLink | null = link.data ?? (s?.state?.type === 'PairingLink' ? s.state : null)
  const left = useCountdown(pairing ? Number(pairing.expires_at_ms) : null)
  const showToken = (unconfigured || replacing) && !starting

  const submitToken = (e: FormEvent) => {
    e.preventDefault()
    const v = token.trim()
    if (v) saveToken.mutate(v)
  }
  const submitOwner = (e: FormEvent) => {
    e.preventDefault()
    const id = telegramId(ownerId, 'user')
    if (id === null) toast.error(`User ids are numeric: ${ownerId}`)
    else link.mutate(id)
  }

  return (
    <Section title="Telegram">
      {status.isPending && <Row label="Bot" description="loading…" />}
      {status.isError && (
        <Row label="Bot" description={`status unavailable: ${status.error instanceof Error ? status.error.message : String(status.error)}`}>
          <Button variant="outline" size="sm" onClick={() => void status.refetch()}>
            Retry
          </Button>
        </Row>
      )}
      {s && (
        <>
          <Row
            label="Bot"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.polling && !!s.state && TELEGRAM_LIVE.includes(s.state.type)} />
                {telegramState(s)}
              </span>
            }
          />
          <Row
            label="Bot token"
            description={
              <>
                {starting ? 'checking the vault…' : unconfigured ? 'not set' : replacing ? 'set · the new token applies at the next restart' : 'set'}
                {showToken && (
                  <form className="mt-2 flex gap-2" onSubmit={submitToken}>
                    <Input
                      type="password"
                      value={token}
                      onChange={(e) => setToken(e.target.value)}
                      placeholder="123456:ABC-DEF…"
                      className="max-w-64 font-mono"
                      autoComplete="off"
                      spellCheck={false}
                    />
                    <Button type="submit" variant="outline" disabled={token.trim().length === 0 || saveToken.isPending}>
                      Save
                    </Button>
                  </form>
                )}
              </>
            }
          >
            {!unconfigured &&
              !starting &&
              (replacing ? (
                <Button variant="ghost" size="sm" onClick={() => setReplacing(false)}>
                  Cancel
                </Button>
              ) : (
                <Button variant="ghost" size="sm" onClick={() => setReplacing(true)}>
                  Replace…
                </Button>
              ))}
          </Row>
          <Row
            label="Owner"
            description={
              <>
                <span className="font-mono">{s.owner === null ? 'not paired' : String(s.owner)}</span>
                <form className="mt-2 flex gap-2" onSubmit={submitOwner}>
                  <Input
                    value={ownerId}
                    onChange={(e) => setOwnerId(e.target.value)}
                    placeholder="or owner id 123456789"
                    className="max-w-64 font-mono"
                    spellCheck={false}
                  />
                  <Button type="submit" variant="outline" disabled={ownerId.trim().length === 0 || link.isPending}>
                    Set
                  </Button>
                </form>
              </>
            }
          >
            <Button variant="outline" size="sm" disabled={link.isPending || unconfigured || starting} onClick={() => link.mutate(null)}>
              {s.owner === null ? 'Pair' : 'Re-pair'}
            </Button>
          </Row>
          {pairing && left > 0 && (
            <Row
              label="Pair the owner"
              description={
                <>
                  Open this link from the owner's Telegram and tap Start · expires in {mmss(left)}
                  <div className="mt-3 inline-block rounded-lg bg-white p-3">
                    <QRCodeSVG value={pairing.url} size={192} marginSize={0} />
                  </div>
                  <code className="mt-2 block break-all font-mono text-xs text-fg-tertiary">{pairing.url}</code>
                </>
              }
            >
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Copy link"
                onClick={() => navigator.clipboard.writeText(pairing.url).then(() => toast.message('Copied'))}
              >
                <Copy />
              </Button>
            </Row>
          )}
          <AllowList label="Allowed users" items={s.allowed.map(String)} placeholder="123456789" allow={allow} remove={remove} />
          <AllowList
            label="Groups"
            items={s.groups.map(String)}
            placeholder="-1001234567890"
            allow={allowGroup}
            remove={removeGroup}
          />
          <details className="py-4 text-[13px] text-fg-secondary">
            <summary className="cursor-pointer text-sm text-fg">Setup notes</summary>
            <ul className="mt-2 list-disc space-y-1 pl-5">
              <li>@BotFather → Bot Settings → Topics: on gives every session its own thread; off keeps one flat chat.</li>
              <li>
                Groups: add the bot as an admin with <em>Manage topics</em>, or <code>/setprivacy</code> → Disable and re-add it; then
                allow the chat id above (<code>/status</code> in the group shows it).
              </li>
              <li>
                In the chat: <code>/telegram link</code>, <code>/telegram allow &lt;id&gt;</code>, <code>/telegram rm &lt;id&gt;</code>,{' '}
                <code>/telegram group add &lt;chat id&gt;</code>.
              </li>
            </ul>
          </details>
        </>
      )}
    </Section>
  )
}
