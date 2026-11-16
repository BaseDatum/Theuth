import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { routed } from '@/lib/owner'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CommunicationState } from "@/rpc/bindings/method.agent.output"
import type { PeerScope } from "@/rpc/bindings/method.agent.output"
import { useSessions } from '@/store/sessions'
import { effectiveLine, scopeView, useMessagingSheet } from './messaging-sheet'
import { Peers } from './Peers'

const SCOPES: readonly [PeerScope, string][] = [
  ['isolated', 'Isolated — reachable only inside its own tree'],
  ['owner', 'Owner — may exchange with other owner roots'],
]

/** Touch target for a text button beside a row (44px under a coarse pointer). */
const TOUCH = 'pointer-coarse:min-h-11'

/**
 * 188 (D1): one chat's messaging, opened from its ⋯ menu (Messaging…), the composer's notice, a peer card or
 * `?messaging=1`: why it can or cannot reach peers, its tree's root scope and alias, and the peer requests on it.
 * Its on/off is the menu's checkbox, and the node-wide switch lives in Settings › Network › Messaging.
 */
export function MessagingSheet({ session }: { session: string }) {
  const open = useMessagingSheet((s) => s.session === session)
  const peer = useMessagingSheet((s) => s.peer)
  const path = useRouterState({ select: (s) => s.location.pathname })
  // Leaving the page (another chat, Settings, a peer's Open) closes it: coming back must not reopen it.
  useEffect(
    () => () => {
      const sheet = useMessagingSheet.getState()
      if (sheet.session === session) sheet.hide()
    },
    [session, path],
  )
  return (
    <Sheet open={open} onOpenChange={(o) => !o && useMessagingSheet.getState().hide()}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>Messaging</SheetTitle>
          <SheetDescription>Whether this chat can reach other sessions and peers, who may discover its tree, and the requests on it.</SheetDescription>
        </SheetHeader>
        <div className="mx-4 mb-4 min-h-0 flex-1 overflow-y-auto">
          <Messaging session={session} peer={peer} />
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** The authoritative `get_communication` snapshot for this chat; every control is one RPC whose reply replaces it. */
function Messaging({ session, peer }: { session: string; peer: string | null }) {
  const canQuery1 = useQuerySupport(["get_communication"])
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const state = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.communication(session || null),
    queryFn: () => api.get_communication(session || null),
  })

  /**
   * Install a setter's reply when it is still the snapshot this sheet shows: the
   * same session and not older than what an event-driven refetch already landed.
   */
  const install = (next: CommunicationState | undefined) => {
    // 183 §4.4: `routed` resolves undefined when the owner is unreachable and the call is parked behind the chat's card.
    if (!next || next.session !== session) return
    const cur = qc.getQueryData<CommunicationState>(queryKeys.communication(session))
    if (cur && next.revision < cur.revision) return
    qc.setQueryData(queryKeys.communication(session), next)
  }
  const setScope = useAction(
    async (v: { scope: PeerScope; alias: string | null }) => {
      if (!session) {
        useDrafts.getState().setSettings(v)
        return undefined
      }
      return routed(session, () => api.set_peer_scope(session, v.scope, v.alias))
    },
    { keys: [['communication'], ['peers']], onSuccess: install },
  )

  const pending = useDrafts((s) => s.settings)
  const s = state.data && (!session ? { ...state.data, scope: pending.scope ?? state.data.scope, alias: pending.alias ?? null } : state.data)
  const openSession = (id: string) => void navigate({ to: '/c/$session', params: { session: id } })

  return (
    <>
      <Section
        title="Status"
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
        {s && (session ? <Effective state={s} onOpen={openSession} /> : <p className="py-2 text-sm">Agent messaging: {pending.communication === undefined ? 'machine default' : pending.communication ? 'on' : 'off'}. Applied before the first message; the machine-wide switch still applies.</p>)}
      </Section>
      {s && <RootScope session={session} state={s} busy={setScope.isPending} onScope={setScope.mutate} onOpen={openSession} />}
      {session && <Peers session={session} focus={peer} />}
    </>
  )
}

function Effective({ state: s, onOpen }: { state: CommunicationState; onOpen(id: string): void }) {
  const { reason, ancestor, nodeOff } = effectiveLine(s)
  return (
    <Row
      label={
        <span className="flex items-center gap-2" aria-live="polite">
          <Dot on={s.effective} />
          Effective: {s.effective ? 'on' : 'off'}
        </span>
      }
      description={
        <>
          {reason}
          {nodeOff && (
            <>
              {' '}
              <Button asChild variant="link" size="xs" className={`h-auto px-0 ${TOUCH}`}>
                <Link to="/settings/$tab" params={{ tab: 'messaging' }}>
                  Open Network › Messaging
                </Link>
              </Button>
            </>
          )}
        </>
      }
    >
      {ancestor && (
        <Button variant="outline" size="sm" className={TOUCH} onClick={() => onOpen(ancestor)}>
          Open ancestor
        </Button>
      )}
    </Row>
  )
}

/** Who may discover this tree: chosen on its root, shown (with the way up) on every other chat in it. */
function RootScope({
  session,
  state: s,
  busy,
  onScope,
  onOpen,
}: {
  session: string
  state: CommunicationState
  busy: boolean
  onScope(v: { scope: PeerScope; alias: string | null }): void
  onOpen(id: string): void
}) {
  const slice = useSessions((st) => st.sessions[session])
  const view = scopeView(session ? slice : { loaded: true, parent: null })
  const parent = view.kind === 'inherited' ? view.parent : null
  return (
    <Section title="Root scope">
      {view.kind === 'root' ? (
        <>
          <Row
            label="Who may discover this tree"
            description="Owner roots can message one another; isolated trees stay private even to a raw id. Owner-only, never a model tool."
          >
            <Select value={s.scope} disabled={busy} onValueChange={(v) => onScope({ scope: v as PeerScope, alias: s.alias })}>
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
            <AliasInput key={s.alias ?? ''} saved={s.alias ?? ''} disabled={busy} onSave={(alias) => onScope({ scope: s.scope, alias })} />
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
          description={view.note}
        >
          {parent && (
            <Button variant="outline" size="sm" className={TOUCH} onClick={() => onOpen(parent)}>
              Open parent
            </Button>
          )}
        </Row>
      )}
    </Section>
  )
}

/** The root's alias: saved on blur or Enter when changed; a new snapshot (`key`) resets the draft. */
function AliasInput({ saved, disabled, onSave }: { saved: string; disabled: boolean; onSave(alias: string): void }) {
  const [draft, setDraft] = useState(saved)
  return (
    <Input
      aria-label="Root alias"
      value={draft}
      disabled={disabled}
      maxLength={64}
      className={`w-48 ${TOUCH}`}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        const alias = draft.trim()
        if (alias !== saved) onSave(alias)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
      }}
    />
  )
}
