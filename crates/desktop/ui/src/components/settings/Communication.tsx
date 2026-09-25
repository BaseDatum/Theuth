import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Copy, RefreshCw } from 'lucide-react'
import { toast } from 'sonner'
import { Peers } from '@/components/chat/Peers'
import { Dot, Empty, Row, Section, errorText, refreshInfo, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import type { A2aStatus } from '@/rpc/bindings/A2aStatus'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { PeerScope } from '@/rpc/bindings/PeerScope'
import { useSessions } from '@/store/sessions'

const SCOPES: readonly [PeerScope, string][] = [
  ['isolated', 'Isolated — reachable only inside its own tree'],
  ['owner', 'Owner — may exchange with other owner roots'],
]

/** Touch target for a text button beside a row (44px under a coarse pointer). */
const TOUCH = 'pointer-coarse:min-h-11'

/** Human text for `CommunicationState.source`. */
function sourceText(s: CommunicationState): string {
  switch (s.source) {
    case 'env':
      return 'Set by the environment (THEUTH_AGENT__COMMUNICATION); `always` writes config.yaml, but the environment wins again on restart.'
    case 'config':
      return 'From config.yaml.'
    case 'command':
      return 'Changed by a command; saved to config.yaml.'
    default:
      return 'Not configured: off.'
  }
}

/**
 * 50 §2.1/§10 Settings › Communication: the authoritative `get_communication`
 * snapshot for the global switch and the open chat, the root's scope and
 * alias, the A2A network status with its reload, and the peer picker with
 * pending requests. Nothing here is guessed: every row renders the server's
 * state and every control is one RPC whose reply replaces the snapshot.
 */
export function Communication({ session, focus }: { session: string | null; focus: string | null }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const state = useQuery({
    queryKey: queryKeys.communication(session),
    queryFn: () => api.get_communication(session),
  })
  // Root identity only from the loaded session itself: a top-level session is its own root.
  const slice = useSessions((s) => (session ? s.sessions[session] : undefined))
  const isRoot = !!session && !!slice?.loaded && slice.parent === null

  /**
   * Install a setter's reply when it is still the snapshot this pane shows: the
   * same session and not older than what an event-driven refetch already landed.
   */
  const install = (next: CommunicationState) => {
    if (next.session !== session) return
    const cur = qc.getQueryData<CommunicationState>(queryKeys.communication(session))
    if (cur && BigInt(next.revision) < BigInt(cur.revision)) return
    qc.setQueryData(queryKeys.communication(session), next)
  }
  const setGlobal = useAction(
    async (on: boolean) => {
      const next = await api.set_communication(null, on)
      await refreshInfo(api)
      return next
    },
    { keys: [['communication'], ['peers']], onSuccess: install },
  )
  const setSession = useAction((on: boolean) => api.set_communication(session, on), {
    keys: [['communication'], ['peers']],
    onSuccess: install,
  })
  const setScope = useAction(
    (v: { scope: PeerScope; alias: string | null }) => api.set_peer_scope(session as string, v.scope, v.alias),
    { keys: [['communication'], ['peers']], onSuccess: install },
  )

  const s = state.data
  const busy = !s || setGlobal.isPending || setSession.isPending
  const openSession = (id: string) => void navigate({ to: '/c/$session', params: { session: id } })

  return (
    <>
      <Section
        title="Communication"
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
              Node-wide ceiling for discovery, sends, delegation and child results; off blocks every session. Saved
              to config.yaml. {s && sourceText(s)}
            </>
          }
        >
          <Switch
            aria-label="Let sessions message each other, node-wide"
            aria-busy={setGlobal.isPending || undefined}
            checked={s?.global ?? false}
            disabled={busy}
            onCheckedChange={(v) => setGlobal.mutate(v)}
          />
        </Row>
        {session && (
          <Row
            label="This chat may communicate"
            description={
              <>
                Turning it off opts this chat and its subagents out. Kept in memory: it survives compaction and
                switching frontends, not a restart of the node.
                {s && !s.global && ' Globally off right now, so this switch alone cannot enable it.'}
              </>
            }
          >
            <Switch
              aria-label="This chat may communicate"
              aria-busy={setSession.isPending || undefined}
              checked={s ? !s.opted_out : false}
              disabled={busy}
              onCheckedChange={(v) => setSession.mutate(v)}
            />
          </Row>
        )}
        {s && (
          <Row
            label={
              <span className="flex items-center gap-2" aria-live="polite">
                <Dot on={s.effective} />
                Effective: {s.effective ? 'on' : 'off'}
              </span>
            }
            description={
              s.effective
                ? 'A route still needs the peer in scope, and network peers an explicit grant.'
                : !s.global
                  ? 'Blocked: the node-wide switch is off.'
                  : s.inherited_from && s.inherited_from !== s.session
                    ? `Blocked: an ancestor session opted out (${s.inherited_from}).`
                    : s.opted_out
                      ? 'Blocked: this chat opted out.'
                      : 'Blocked.'
            }
          >
            {!s.effective && s.global && s.inherited_from && s.inherited_from !== s.session && (
              <Button variant="outline" size="sm" className={TOUCH} onClick={() => openSession(s.inherited_from as string)}>
                Open ancestor
              </Button>
            )}
          </Row>
        )}
      </Section>

      {session && s && (
        <Section title="Root scope">
          {isRoot ? (
            <>
              <Row
                label="Who may discover this tree"
                description="Owner roots can message one another; isolated trees stay private even to a raw id. Owner-only, never a model tool."
              >
                <Select
                  value={s.scope}
                  disabled={setScope.isPending}
                  onValueChange={(v) => setScope.mutate({ scope: v as PeerScope, alias: null })}
                >
                  <SelectTrigger className={`max-w-72 ${TOUCH}`} aria-label="Root scope">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {SCOPES.map(([id, label]) => (
                      <SelectItem key={id} value={id}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Row>
              <Row label="Alias" description="Short name other owner roots see instead of the id. Empty clears it.">
                <Input
                  key={s.alias ?? ''}
                  aria-label="Root alias"
                  defaultValue={s.alias ?? ''}
                  disabled={setScope.isPending}
                  maxLength={64}
                  className={`w-48 ${TOUCH}`}
                  onBlur={(e) => {
                    const alias = e.currentTarget.value.trim()
                    if (alias !== (s.alias ?? '')) setScope.mutate({ scope: s.scope, alias })
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') e.currentTarget.blur()
                  }}
                />
              </Row>
            </>
          ) : (
            <Row
              label={
                <span className="flex items-center gap-2">
                  Scope <Badge variant="outline">{s.scope}</Badge>
                  {s.alias && <span className="text-fg-secondary">alias {s.alias}</span>}
                </span>
              }
              description={
                slice?.loaded
                  ? 'Inherited from the root of this tree; change it from the root session.'
                  : 'Open the chat once to know whether it is a root before editing its scope.'
              }
            >
              {slice?.parent && (
                <Button variant="outline" size="sm" className={TOUCH} onClick={() => openSession(slice.parent as string)}>
                  Open parent
                </Button>
              )}
            </Row>
          )}
        </Section>
      )}

      <Network state={s} />

      {session && <Peers session={session} focus={focus} />}
    </>
  )
}

/** 50 §6.1/§10: `a2a_status` with peers, exports, the error and `reload_a2a`. */
function Network({ state }: { state: CommunicationState | undefined }) {
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
  return (
    <Section
      title="Network (A2A)"
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
      {status.isPending && <Empty>Loading…</Empty>}
      {status.isError && <p className="py-2 text-sm text-destructive">{errorText(status.error)}</p>}
      {a && (
        <>
          <Row
            label={
              <span className="flex items-center gap-2">
                <Dot on={a.listening} />
                {!a.enabled ? 'Disabled' : a.listening ? 'Listening' : 'Not listening'}
              </span>
            }
            description={
              !a.enabled
                ? 'a2a.enabled is false in config.yaml: no listener, client or credential is loaded. Enabling it needs a restart.'
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
          {a.enabled && a.peers.length === 0 && a.exports.length === 0 && (
            <Empty>No peers or exports in a2a/peers.yaml. Edit the file, then reload.</Empty>
          )}
          {a.peers.map((p) => (
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
            <Row
              key={x.name}
              label={
                <span className="flex flex-wrap items-center gap-2">
                  <Dot on={x.peer !== null} />
                  {x.name}
                  <Badge variant="outline">export</Badge>
                  <span className="text-fg-secondary">{x.peer ? `granted to ${x.peer}` : 'no peer'}</span>
                </span>
              }
              description={<span className="break-all font-mono text-xs">{x.url}</span>}
            >
              <Button
                variant="outline"
                size="sm"
                className={TOUCH}
                onClick={() => void navigate({ to: '/c/$session', params: { session: x.session } })}
              >
                Open session
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Copy URL of ${x.name}`}
                onClick={() => navigator.clipboard.writeText(x.url).then(() => toast.message('Copied'), (e: unknown) => toast.error(errorText(e)))}
              >
                <Copy />
              </Button>
            </Row>
          ))}
        </>
      )}
    </Section>
  )
}
