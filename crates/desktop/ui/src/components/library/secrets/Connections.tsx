import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Cloud, KeyRound, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Dot, Empty, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { notify } from '@/lib/notify'
import { AddConnection } from './AddConnection'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { Browse } from './Browse'
import { AskSwitches, ConnSettingsPanel } from './ConnSettings'
import { backendLabel, statusDetail, statusLabel } from './labels'
import { Sends } from './Sends'
import { type LoginMethod, LoginCard, UnlockCard } from './UnlockCard'
import type { ConnInfo } from '@/rpc/bindings/method.secrets.output'

/** Methods the Connections section needs before it renders. */
const CONNECTION_METHODS = ['list_secret_connections', 'add_secret_connection', 'remove_secret_connection', 'connection_unlock', 'connection_lock', 'list_external_secrets', 'set_connection_setting'] as const

type Panel = 'browse' | 'sends' | 'settings' | 'unlock' | 'login'

/** The account part of a Bitwarden summary (`me@… · vault.bitwarden.eu` → `me@…`). */
const accountOf = (info: ConnInfo) => info.summary.split(' · ')[0] ?? info.conn

/** §9.6: the connection's login kind (`ConnInfo.login`, absent from an older engine); the login card starts there. */
const loginOf = (info: ConnInfo): LoginMethod => (info as ConnInfo & { login?: LoginMethod | null }).login ?? 'api_key'

/** One connection: glyph, name, summary, status dot, Lock / Unlock / Log in, the four ask switches, Browse, Sends (Bitwarden), Settings, Remove. */
function ConnectionRow({ info }: { info: ConnInfo }) {
  const api = useApi()
  const qc = useQueryClient()
  const [panel, setPanel] = useState<Panel | null>(null)
  const toggle = (p: Panel) => setPanel(panel === p ? null : p)
  const lock = useAction(() => api.connection_lock(info.conn), { keys: [queryKeys.secretConnections] })
  const remove = useAction(() => api.remove_secret_connection(info.conn), {
    keys: [queryKeys.secretConnections, ['always-ask']],
    onSuccess: () => notify.success(`Removed ${info.conn}`),
  })
  const state = info.status.state
  const detail = statusDetail(info.status)
  const Glyph = info.backend === 'aws' ? Cloud : KeyRound
  const refresh = () => void qc.invalidateQueries({ queryKey: queryKeys.secretConnections })
  return (
    <div className="border-b border-border-subtle py-3 last:border-0">
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-2">
          <Glyph className="size-4 shrink-0 text-fg-secondary" aria-label={backendLabel(info.backend)} />
          <div className="min-w-0">
            <div className="text-sm">
              <span className="font-mono">{info.conn}</span> <span className="text-fg-tertiary">· {backendLabel(info.backend)}</span>
            </div>
            <div className="truncate text-[13px] text-fg-secondary">{info.summary}</div>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <span className="flex items-center gap-1.5 text-[13px] text-fg-secondary" title={detail ?? undefined}>
            <Dot on={state === 'ready'} />
            {statusLabel(info.status)}
          </span>
          {info.backend === 'bw' && state === 'ready' && (
            <Button variant="outline" size="sm" disabled={lock.isPending} onClick={() => lock.mutate(undefined)}>
              Lock
            </Button>
          )}
          {state === 'locked' && (
            <Button variant="outline" size="sm" onClick={() => toggle('unlock')}>
              Unlock
            </Button>
          )}
          {state === 'logged_out' && info.backend === 'bw' && (
            <Button variant="outline" size="sm" onClick={() => toggle('login')}>
              Log in
            </Button>
          )}
          {state === 'ready' && (
            <Button variant="ghost" size="sm" onClick={() => toggle('browse')}>
              Browse
            </Button>
          )}
          {info.backend === 'bw' && (
            <Button variant="ghost" size="sm" onClick={() => toggle('sends')}>
              Sends
            </Button>
          )}
          <Button variant="ghost" size="sm" aria-expanded={panel === 'settings'} onClick={() => toggle('settings')}>
            Settings
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Remove ${info.conn}`}
            disabled={remove.isPending}
            onClick={() => confirm(`Remove ${info.conn}? Its stored credentials, its config and its always-ask entries go; the secrets in ${backendLabel(info.backend)} stay.`) && remove.mutate(undefined)}
          >
            <Trash2 />
          </Button>
        </div>
      </div>
      {detail && state !== 'ready' && <div className="mt-1 text-xs text-fg-tertiary">{detail}</div>}
      <div className="mt-2">
        <AskSwitches info={info} />
      </div>
      {panel === 'unlock' && (
        <UnlockCard
          conn={info.conn}
          account={accountOf(info)}
          onUnlock={async (pw) => {
            await api.connection_unlock(info.conn, pw)
            setPanel(null)
            refresh()
          }}
          onCancel={() => setPanel(null)}
        />
      )}
      {panel === 'login' && (
        <LoginCard
          conn={info.conn}
          email={accountOf(info)}
          method={loginOf(info)}
          onCancel={() => setPanel(null)}
          onDone={() => {
            setPanel('unlock')
            refresh()
          }}
        />
      )}
      {panel === 'browse' && <Browse info={info} />}
      {panel === 'sends' && <Sends info={info} />}
      {panel === 'settings' && <ConnSettingsPanel info={info} />}
    </div>
  )
}

/** §9.1 Connections, above the local list: one row per external manager, and Add connection. */
export function Connections() {
  const api = useApi()
  const can = useQuerySupport(CONNECTION_METHODS)
  const list = useQuery({ enabled: can, queryKey: queryKeys.secretConnections, queryFn: () => api.list_secret_connections() })
  const [adding, setAdding] = useState(false)
  if (!can) return null
  return (
    <Section
      title="Connections"
      actions={
        !adding && (
          <Button variant="outline" size="sm" onClick={() => setAdding(true)}>
            Add connection
          </Button>
        )
      }
    >
      {adding && <AddConnection existing={(list.data ?? []).map((c) => c.conn)} onDone={() => setAdding(false)} />}
      {list.data?.length === 0 && !adding && (
        <Empty>No external manager. Connect AWS Secrets Manager or Bitwarden to use their secrets by reference; the agent never sees a value.</Empty>
      )}
      {list.data?.map((c) => <ConnectionRow key={c.conn} info={c} />)}
    </Section>
  )
}
