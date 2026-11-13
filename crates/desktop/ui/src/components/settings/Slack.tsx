import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Copy } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { AllowList, ChannelStatusRow, useChannelEvents } from '@/components/settings/Channels'
import { Dot, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { openExternal } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SlackStatus } from "@/rpc/bindings/method.slack.output"

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

/** 188: Channels › Slack: the app manifest, the bot, the owner, allowed users and channels. */
export function Slack() {
  const canQuery1 = useQuerySupport(["slack_status"])
  const api = useApi()
  useChannelEvents('slack.events@1', queryKeys.slack)
  const status = useQuery({ enabled: canQuery1, queryKey: queryKeys.slack, queryFn: () => api.slack_status() })
  const keys = [queryKeys.slack]
  const allowUser = useAction((u: string) => api.slack_allow(u), { keys })
  const removeUser = useAction((u: string) => api.slack_remove_allowed(u), { keys })
  const allowChannel = useAction((c: string) => api.slack_allow_channel(c), { keys })
  const removeChannel = useAction((c: string) => api.slack_remove_channel(c), { keys })
  const s = status.data

  return (
    <Section title="Slack">
      <SlackManifest />
      <ChannelStatusRow label="Bot" status={status} />
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
  const canQuery2 = useQuerySupport(["slack_manifest"])
  const api = useApi()
  const manifest = useQuery({ enabled: canQuery2, queryKey: queryKeys.slackManifest, queryFn: () => api.slack_manifest(), staleTime: Infinity })
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
        wide
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
