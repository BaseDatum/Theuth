import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Copy, RefreshCw } from 'lucide-react'
import { Fragment } from 'react'
import { toast } from 'sonner'
import { ConfigFlagRow } from '@/components/settings/ConfigFlagRow'
import { Dot, Empty, Row, Section, errorText, refreshInfo, useAction } from '@/components/settings/shared'
import { WaitingRow } from '@/components/settings/Contacts'
import { MODE_LABEL, useContacts, useFirstWaiting } from '@/lib/contacts'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import type { A2aStatus } from '@/rpc/bindings/A2aStatus'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'

/** Touch target for a text button beside a row (44px under a coarse pointer). */
const TOUCH = 'pointer-coarse:min-h-11'

/** Human text for `CommunicationState.source`; `null` when the saved setting is all there is to say. */
function sourceText(s: CommunicationState): string | null {
  switch (s.source) {
    case 'env':
      return 'Set by an environment variable (THEUTH_AGENT__COMMUNICATION), which wins again when theuth restarts.'
    case 'config':
      return null
    case 'command':
      return 'Changed with /communication.'
    case 'mesh':
      return 'On by default because this machine has paired machines.'
    default:
      return 'Not configured: off.'
  }
}

/**
 * 188 Settings › Network › Messaging (50 §2.1/§10): the node-wide switch from the authoritative `get_communication`
 * snapshot, then A2A over HTTP. Every row renders the server's state and every control is one RPC whose reply
 * replaces the snapshot. What one chat may do lives in that chat's menu.
 */
export function MessagingTab() {
  const api = useApi()
  const qc = useQueryClient()
  const state = useQuery({
    queryKey: queryKeys.communication(null),
    queryFn: () => api.get_communication(null),
  })

  /** Install the setter's reply unless an event-driven refetch already landed a newer snapshot. */
  const install = (next: CommunicationState) => {
    if (next.session !== null) return
    const cur = qc.getQueryData<CommunicationState>(queryKeys.communication(null))
    if (cur && BigInt(next.revision) < BigInt(cur.revision)) return
    qc.setQueryData(queryKeys.communication(null), next)
  }
  const setGlobal = useAction(
    async (on: boolean) => {
      const next = await api.set_communication(null, on)
      await refreshInfo(api)
      return next
    },
    { keys: [['communication'], ['peers']], onSuccess: install },
  )

  const s = state.data
  return (
    <>
      <Section
        title="Messaging"
        actions={
          state.isError && (
            <Button variant="outline" size="sm" className={TOUCH} onClick={() => void state.refetch()}>
              <RefreshCw /> Retry
            </Button>
          )
        }
      >
        {state.isError && <p className="py-2 text-sm text-destructive">{errorText(state.error)}</p>}
        {state.isPending && <Empty>Loading…</Empty>}
        <Row
          label="Let sessions message each other"
          description={
            <>
              Node-wide ceiling for discovery, sends, delegation and child results; off blocks every session. {s && sourceText(s)}
            </>
          }
        >
          <Switch
            aria-label="Let sessions message each other, node-wide"
            aria-busy={setGlobal.isPending || undefined}
            checked={s?.global ?? false}
            disabled={!s || setGlobal.isPending}
            onCheckedChange={(v) => setGlobal.mutate(v)}
          />
        </Row>
      </Section>

      <A2aSection state={s} />
    </>
  )
}

/**
 * 50 §6.1/§10: A2A over HTTP — its switch, `a2a_status` with the HTTP peers, the exports, the error and `reload_a2a`.
 * 183 §7: a contact is listed under Network › Contacts, never here as an HTTP peer with roots.
 */
export function A2aSection({ state }: { state: CommunicationState | undefined }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const status = useQuery({ queryKey: queryKeys.a2a, queryFn: () => api.a2a_status() })
  const reload = useAction(() => api.reload_a2a(), {
    keys: [['communication'], ['peers']],
    onSuccess: (next: A2aStatus) => {
      qc.setQueryData(queryKeys.a2a, next)
      toast(next.error ? `Reload failed, previous configuration kept: ${next.error}` : 'A2A configuration reloaded')
    },
  })
  const a = status.data
  const contacts = useContacts().data ?? []
  const waiting = useFirstWaiting().data ?? []
  const contactNames = new Set(contacts.map((c) => c.name))
  return (
    <Section
      title="A2A"
      actions={
        <>
          {status.isError && (
            <Button variant="outline" size="sm" className={TOUCH} onClick={() => void status.refetch()}>
              <RefreshCw /> Retry
            </Button>
          )}
          {a?.enabled && (
            <Button
              variant="outline"
              size="sm"
              className={TOUCH}
              disabled={reload.isPending}
              aria-busy={reload.isPending || undefined}
              onClick={() => reload.mutate(undefined)}
            >
              <RefreshCw className={reload.isPending ? 'animate-spin' : undefined} /> Reload peers and exports
            </Button>
          )}
        </>
      }
    >
      <ConfigFlagRow
        flag="a2a.enabled"
        name="A2A over HTTP"
        label="A2A over HTTP"
        description="An HTTP listener, client and credential for agents outside theuth. Contacts don't need it: they reach this machine over theuth/a2a/1."
      />
      {status.isPending && <Empty>Loading…</Empty>}
      {status.isError && <p className="py-2 text-sm text-destructive">{errorText(status.error)}</p>}
      {a && (
        <>
          <Row
            label={
              <span className="flex items-center gap-2">
                <Dot on={a.listening} />
                {!a.enabled ? 'Off' : a.listening ? 'Listening' : 'Not listening'}
              </span>
            }
            description={
              !a.enabled
                ? undefined
                : a.listening
                  ? `Bound to ${a.address ?? 'the configured address'}${a.public_url ? `, published at ${a.public_url}` : ''}.`
                  : state && !state.global
                    ? 'Configured, but stopped while the node-wide communication switch is off.'
                    : 'Configured, but the listener is not bound.'
            }
          />
          {a.error && (
            <Row label="Problem" description={<span className="text-destructive">{a.error}</span>} />
          )}
          {a.peers
            .filter((p) => !contactNames.has(p.name))
            .map((p) => (
              <Row
                key={p.name}
                label={
                  <span className="flex flex-wrap items-center gap-2">
                    <Dot on={p.binding !== null} />
                    {p.name}
                    {p.card && <span className="text-fg-secondary">({p.card})</span>}
                    {p.outbound && <Badge variant="outline">outbound</Badge>}
                    {p.inbound && <Badge variant="outline">inbound</Badge>}
                  </span>
                }
                description={
                  <>
                    {p.sessions.length} local root{p.sessions.length === 1 ? '' : 's'} may send
                    {p.exports.length > 0 && ` · may call ${p.exports.join(', ')}`}
                    {p.error && <span className="block text-destructive">{p.error}</span>}
                  </>
                }
              />
            ))}
          {a.exports.map((x) => (
            <Fragment key={x.name}>
            <Row
              label={
                <span className="flex flex-wrap items-center gap-2">
                  <Dot on={x.peer !== null} />
                  {x.name}
                  <Badge variant="outline">export</Badge>
                  {x.mode && <Badge variant="outline">{MODE_LABEL[x.mode]}</Badge>}
                  <span className="text-fg-secondary">{x.peer ? `granted to ${x.peer}` : 'no peer'}</span>
                </span>
              }
              description={
                x.peer !== null && contactNames.has(x.peer) ? (
                  x.mode === 'full'
                    ? 'reached by the contact over theuth/a2a/1, full collaboration: its agent and this chat’s agent talk freely, like mesh peers'
                    : 'reached by the contact over theuth/a2a/1, cautious: one capped turn per message'
                ) : (
                  <span className="break-all font-mono text-xs">{x.url}</span>
                )
              }
            >
              <Button
                variant="outline"
                size="sm"
                className={TOUCH}
                onClick={() => void navigate({ to: '/c/$session', params: { session: x.session } })}
              >
                Open session
              </Button>
              {!(x.peer !== null && contactNames.has(x.peer)) && (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`Copy URL of ${x.name}`}
                  onClick={() => navigator.clipboard.writeText(x.url).then(() => toast.message('Copied'), (e: unknown) => toast.error(errorText(e)))}
                >
                  <Copy />
                </Button>
              )}
            </Row>
            {waiting
              .filter((w) => w.export === x.name)
              .map((w) => (
                <WaitingRow key={w.contact} w={w} />
              ))}
            </Fragment>
          ))}
        </>
      )}
    </Section>
  )
}
