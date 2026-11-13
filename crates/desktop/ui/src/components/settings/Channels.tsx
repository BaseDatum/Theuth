import { type QueryKey, type UseQueryResult, useQueryClient } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'
import { type Action, Empty, Row, errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { SignalEvent } from "@/rpc/bindings/method.signal.output"
import type { SlackEvent } from "@/rpc/bindings/method.slack.output"
import type { TelegramEvent } from "@/rpc/bindings/method.telegram.output"
import { on } from '@/store/sessions'

// 188: what Channels › Signal, Slack and Telegram share; each tab is its own component (`Signal.tsx`, `Slack.tsx`, `Telegram.tsx`).

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

/** Refetch a channel's status on any channel bus event. */
export function useChannelEvents(key: QueryKey) {
  const qc = useQueryClient()
  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: key })
    const offs = CHANNEL_EVENTS.map((t) => on(t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc, key])
}

/** A channel's first row while its status call loads or after it fails (with Retry); nothing once it answers. */
export function ChannelStatusRow({ label, status }: { label: string; status: UseQueryResult<unknown> }) {
  if (status.isPending) return <Row label={label} description="loading…" />
  if (!status.isError) return null
  return (
    <Row label={label} description={`status unavailable: ${errorText(status.error)}`}>
      <Button variant="outline" size="sm" onClick={() => void status.refetch()}>
        Retry
      </Button>
    </Row>
  )
}

/** An id list with a remove `×` per item and an input that adds one. */
export function AllowList({
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
