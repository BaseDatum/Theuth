import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, X } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { type FormEvent, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { type Action, Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
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
import { on } from '@/store/sessions'

/** Every Signal and Slack bus event; any of them means a status snapshot is stale. */
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
] as const satisfies readonly (SignalEvent | SlackEvent)['type'][]

export function Channels() {
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => {
      void qc.invalidateQueries({ queryKey: queryKeys.signal })
      void qc.invalidateQueries({ queryKey: queryKeys.slack })
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
