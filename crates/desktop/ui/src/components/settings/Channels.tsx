import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, X } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { type FormEvent, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { type Action, Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { openExternal } from '@/components/attention/Pointers'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SignalEvent } from '@/rpc/bindings/SignalEvent'
import type { SignalStatus } from '@/rpc/bindings/SignalStatus'
import type { SlackEvent } from '@/rpc/bindings/SlackEvent'
import type { SlackStatus } from '@/rpc/bindings/SlackStatus'
import type { PairingLink } from '@/rpc/bindings/PairingLink'
import type { TelegramEvent } from '@/rpc/bindings/TelegramEvent'
import type { TelegramStatus } from '@/rpc/bindings/TelegramStatus'
import { on } from '@/store/sessions'

/** Every Signal, Slack and Telegram bus event; any of them means a status snapshot is stale. */
const CHANNEL_EVENTS = [
  'LinkUrl',
  'Linked',
  'Connected',
  'Disconnected',
  'Unlinked',
  'DaemonOnly',
  'RateLimited',
  'StreamFallback',
  'Unconfigured',
  'Identity',
  'Conflict',
  'Unreachable',
  'PairingLink',
  'Paired',
] as const satisfies readonly (SignalEvent | SlackEvent | TelegramEvent)['type'][]

export function Channels() {
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => {
      void qc.invalidateQueries({ queryKey: queryKeys.signal })
      void qc.invalidateQueries({ queryKey: queryKeys.slack })
      void qc.invalidateQueries({ queryKey: queryKeys.telegram })
    }
    const offs = CHANNEL_EVENTS.map((t) => on(t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
  return (
    <>
      <Signal />
      <Slack />
      <Telegram />
    </>
  )
}

function signalState(s: SignalStatus): string {
  const e = s.state
  if (!e) return s.registered ? 'registered' : 'not linked'
  switch (e.type) {
    case 'LinkUrl':
      return 'waiting for the link'
    case 'Linked':
      return `linked as ${e.number}`
    case 'Connected':
      return 'connected'
    case 'Disconnected':
      return `disconnected, retrying in ${e.retry_in_secs}s`
    case 'Unlinked':
      return 'unlinked'
    case 'DaemonOnly':
      return 'daemon only'
  }
}

function Signal() {
  const api = useApi()
  const status = useQuery({ queryKey: queryKeys.signal, queryFn: () => api.signal_status() })
  const keys = [queryKeys.signal]
  const link = useAction((_: void) => api.signal_link())
  const unlink = useAction((_: void) => api.signal_unlink(), { keys, onSuccess: () => link.reset() })
  const allow = useAction((n: string) => api.signal_allow(n), { keys })
  const remove = useAction((n: string) => api.signal_remove_allowed(n), { keys })
  const s = status.data
  const linkUrl = link.data ?? (s?.state?.type === 'LinkUrl' ? s.state.url : null)

  return (
    <Section title="Signal">
      {s && (
        <>
          <Row
            label="Account"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.state?.type === 'Connected' || s.state?.type === 'Linked'} />
                {s.number ? `${s.number} · ${signalState(s)}` : signalState(s)}
              </span>
            }
          >
            {s.registered ? (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="outline" size="sm">
                    Unlink
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>Unlink Signal?</AlertDialogTitle>
                    <AlertDialogDescription>
                      This device is removed from the account; linking again needs a new QR scan.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction onClick={() => unlink.mutate(undefined)}>Unlink</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            ) : (
              <Button variant="outline" size="sm" disabled={link.isPending} onClick={() => link.mutate(undefined)}>
                Link
              </Button>
            )}
          </Row>
          {linkUrl && !s.registered && (
            <Row
              label="Link this device"
              description={
                <>
                  Open Signal › Linked devices › Link new device, and scan this code:
                  <div className="mt-3 inline-block rounded-lg bg-white p-3">
                    <QRCodeSVG value={linkUrl} size={192} marginSize={0} />
                  </div>
                  <code className="mt-2 block break-all font-mono text-xs text-fg-tertiary">{linkUrl}</code>
                </>
              }
            >
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Copy link"
                onClick={() => navigator.clipboard.writeText(linkUrl).then(() => toast.message('Copied'))}
              >
                <Copy />
              </Button>
            </Row>
          )}
          <Row label="Owner" description="Messages from this number always reach the agent.">
            <span className="font-mono text-sm">{s.owner}</span>
          </Row>
          <AllowList
            label="Allowed numbers"
            items={s.allowed}
            placeholder="+15551234567"
            allow={allow}
            remove={remove}
          />
        </>
      )}
    </Section>
  )
}

function slackState(s: SlackStatus): string {
  const e = s.state
  if (!e) return 'unknown'
  switch (e.type) {
    case 'Connected':
      return `connected (${e.connections} sockets)`
    case 'Disconnected':
      return `disconnected: ${e.reason}`
    case 'RateLimited':
      return `rate limited on ${e.method}, retry in ${e.retry_after_secs}s`
    case 'StreamFallback':
      return 'connected'
    case 'Unconfigured':
      return 'not configured'
    case 'DaemonOnly':
      return 'daemon only'
  }
}

function Slack() {
  const api = useApi()
  const status = useQuery({ queryKey: queryKeys.slack, queryFn: () => api.slack_status() })
  const keys = [queryKeys.slack]
  const allowUser = useAction((u: string) => api.slack_allow(u), { keys })
  const removeUser = useAction((u: string) => api.slack_remove_allowed(u), { keys })
  const allowChannel = useAction((c: string) => api.slack_allow_channel(c), { keys })
  const removeChannel = useAction((c: string) => api.slack_remove_channel(c), { keys })
  const s = status.data

  return (
    <Section title="Slack">
      <SlackManifest />
      {s && (
        <>
          <Row
            label="Bot"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.state?.type === 'Connected' || s.state?.type === 'StreamFallback'} />
                {[slackState(s), s.bot_user && `user ${s.bot_user}`, s.team && `team ${s.team}`].filter(Boolean).join(' · ')}
              </span>
            }
          />
          <Row label="Owner" description="Messages from this user always reach the agent.">
            <span className="font-mono text-sm">{s.owner}</span>
          </Row>
          <AllowList label="Allowed users" items={s.allowed} placeholder="U0123456789" allow={allowUser} remove={removeUser} />
          <AllowList
            label="Channels"
            items={s.channels}
            placeholder="C0123456789"
            allow={allowChannel}
            remove={removeChannel}
          />
        </>
      )}
    </Section>
  )
}

/** 170 D2: the app manifest (`slack_manifest`), first in the Slack section and independent of `slack_status` — it is needed before Slack is configured. */
function SlackManifest() {
  const api = useApi()
  const manifest = useQuery({ queryKey: queryKeys.slackManifest, queryFn: () => api.slack_manifest(), staleTime: Infinity })
  const [shown, setShown] = useState(false)
  const m = manifest.data
  const copy = () => {
    if (m === undefined) return
    navigator.clipboard.writeText(m).then(
      () => toast.message('Slack manifest copied'),
      (e: unknown) => toast.error(errorText(e)),
    )
  }
  return (
    <>
      <Row
        stack
        label="App manifest"
        description={
          <>
            Create the theuth Slack app from this manifest (api.slack.com/apps → Create New App → From a manifest), or paste it under App Manifest to update an existing app, then reinstall it.
            {manifest.isPending && <span className="block">loading…</span>}
            {manifest.isError && <span className="block text-destructive">manifest unavailable: {errorText(manifest.error)}</span>}
          </>
        }
      >
        <Button variant="outline" size="sm" disabled={m === undefined} onClick={copy}>
          <Copy />
          Copy manifest
        </Button>
        <Button variant="outline" size="sm" disabled={m === undefined} onClick={() => setShown(!shown)}>
          {shown ? 'Hide manifest' : 'Show manifest'}
        </Button>
        <Button variant="outline" size="sm" onClick={() => openExternal('https://api.slack.com/apps')}>
          Open Slack apps
        </Button>
      </Row>
      {shown && m !== undefined && (
        <pre aria-label="Slack app manifest" className="mb-4 max-h-80 overflow-auto rounded-md bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre select-text text-fg-secondary">
          {m}
        </pre>
      )}
    </>
  )
}

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

function Telegram() {
  const api = useApi()
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

/** An id list with a remove `×` per item and an input that adds one. */
function AllowList({
  label,
  items,
  placeholder,
  allow,
  remove,
}: {
  label: string
  items: string[]
  placeholder: string
  allow: Action<string, null>
  remove: Action<string, null>
}) {
  const [value, setValue] = useState('')
  const submit = (e: FormEvent) => {
    e.preventDefault()
    const v = value.trim()
    if (v) allow.mutate(v, { onSuccess: () => setValue('') })
  }
  return (
    <div className="border-b border-border-subtle py-4 last:border-0">
      <div className="text-sm">{label}</div>
      {items.length === 0 && <Empty>None.</Empty>}
      <ul className="my-2 space-y-1">
        {items.map((id) => (
          <li key={id} className="flex items-center justify-between gap-2 font-mono text-sm">
            {id}
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={`Remove ${id}`}
              disabled={remove.isPending}
              onClick={() => remove.mutate(id)}
            >
              <X />
            </Button>
          </li>
        ))}
      </ul>
      <form className="flex gap-2" onSubmit={submit}>
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={placeholder}
          className="max-w-64 font-mono"
          spellCheck={false}
        />
        <Button type="submit" variant="outline" disabled={value.trim().length === 0 || allow.isPending}>
          Allow
        </Button>
      </form>
    </div>
  )
}
