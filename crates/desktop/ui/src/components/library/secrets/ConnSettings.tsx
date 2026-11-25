import { useQuery } from '@tanstack/react-query'
import { Row, useAction } from '@/components/settings/shared'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { ConnInfo, ConnSettings, ExternalSettings } from '@/rpc/bindings/method.secrets.output'
import { type Direction, alwaysAskKey } from './labels'
import { EXTERNAL_DEFAULTS } from './types'

type Flag = { [K in keyof ConnSettings]: ConnSettings[K] extends boolean ? K : never }[keyof ConnSettings]
type Count = { [K in keyof ConnSettings]: ConnSettings[K] extends number ? K : never }[keyof ConnSettings]
/** One `{ key, value }` write of a settings record, key and value in step. */
type Write<T> = { [K in keyof T]: { key: K; value: T[K] } }[keyof T]

/** D20's per-connection switches, each with its one-sentence cost (§8). `bw`: Bitwarden only. */
const FLAGS: { key: Flag; label: string; cost: string; bw?: boolean }[] = [
  { key: 'allow_reveal', label: 'Reveal', cost: 'Your Reveal button on this page shows values on this screen; each reveal is logged.' },
  { key: 'allow_permanent_delete', label: 'Allow permanent delete', cost: 'A delete may skip the recovery window or the trash; a mistake cannot be undone.' },
  { key: 'remember_unlock', label: 'Remember unlock', cost: "Bitwarden stays unlocked across restarts; this machine's key then protects your vault instead of your master password.", bw: true },
  { key: 'lock_on_sleep', label: 'Lock on sleep', cost: 'Locks when this Mac sleeps or its screen locks; the next use asks for the master password.', bw: true },
  { key: 'agent_may_request_login', label: 'Agent may ask you to log in', cost: 'A logged-out connection shows you a login card for the agent’s call instead of failing it.', bw: true },
  { key: 'expose_password_history', label: 'Expose password history', cost: 'References may reach a login’s old passwords (#password:-1 … -4).', bw: true },
  { key: 'expose_totp_seed', label: 'Expose TOTP seed', cost: 'References may expand the TOTP seed itself, not only the current code.', bw: true },
  { key: 'mcp_config_refs', label: 'References in MCP config', cost: 'MCP servers expand this connection’s references at connect time; only while reads do not always ask.' },
  { key: 'send_owner_only', label: 'Only you answer Send cards', cost: 'Off: the double, an app press or a chat user may approve a Send.', bw: true },
  { key: 'send_link_to_agent', label: 'Send link to the agent', cost: 'The agent receives the Send link itself; it enters the transcript.', bw: true },
  { key: 'manage_all_sends', label: 'Manage all Sends', cost: 'List and revoke Sends theuth did not create.', bw: true },
]

const COUNTS: { key: Count; label: string; cost: string; min: number; max: number; aws?: boolean; bw?: boolean }[] = [
  { key: 'delete_recovery_days', label: 'Recovery window (days)', cost: 'How long AWS keeps a deleted secret before it purges it (7–30).', min: 7, max: 30, aws: true },
  { key: 'lock_after_idle_secs', label: 'Lock after idle (seconds)', cost: '0 locks only on quit; the unlock card reappears on the next use.', min: 0, max: 604_800, bw: true },
]

/** §8: "Ask before every …" — a second view of the connection's `secret:` entries in `tools.always_ask`. */
export function AskSwitches({ info }: { info: ConnInfo }) {
  const api = useApi()
  const set = useAction((v: { dir: Direction; on: boolean }) => api.set_always_ask(null, alwaysAskKey(info.backend, info.conn, v.dir), v.on), {
    keys: [queryKeys.secretConnections, ['always-ask']],
  })
  const dirs: Direction[] = ['read', 'write', 'delete', ...(info.backend === 'bw' ? (['send'] as const) : [])]
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-secondary">
      Ask before every
      {dirs.map((dir) => (
        <label key={dir} className="flex items-center gap-1.5">
          <Switch
            size="sm"
            aria-label={`Ask before every ${dir} on ${info.conn}`}
            checked={(dir === 'send' ? info.ask.send : info.ask[dir]) ?? false}
            disabled={set.isPending}
            onCheckedChange={(on) => set.mutate({ dir, on })}
          />
          {dir === 'send' ? 'Send' : dir}
        </label>
      ))}
    </span>
  )
}

/** §8's Settings disclosure on a connection row: every per-connection D20 key, written by `set_connection_setting`. */
export function ConnSettingsPanel({ info }: { info: ConnInfo }) {
  const api = useApi()
  const set = useAction((v: Write<ConnSettings>) => api.set_connection_setting(info.conn, v.key, v.value), {
    keys: [queryKeys.secretConnections],
  })
  const s = info.settings
  return (
    <div className="ml-4 border-l border-border-subtle pl-4">
      {FLAGS.filter((f) => !f.bw || info.backend === 'bw').map((f) => (
        <Row key={f.key} label={f.label} description={f.cost}>
          <Switch aria-label={`${f.label} on ${info.conn}`} checked={s[f.key]} disabled={set.isPending} onCheckedChange={(value) => set.mutate({ key: f.key, value })} />
        </Row>
      ))}
      {COUNTS.filter((c) => (!c.aws || info.backend === 'aws') && (!c.bw || info.backend === 'bw')).map((c) => (
        <Row key={c.key} label={c.label} description={c.cost}>
          <Input
            type="number"
            min={c.min}
            max={c.max}
            defaultValue={s[c.key]}
            aria-label={`${c.label} on ${info.conn}`}
            className="h-8 w-24"
            onBlur={(e) => {
              const n = Number(e.currentTarget.value)
              if (Number.isInteger(n) && n >= c.min && n <= c.max && n !== s[c.key]) set.mutate({ key: c.key, value: n })
            }}
          />
        </Row>
      ))}
      <Row label="Writes may reach organisations" description="Creates, updates and deletes may target organisation collections, not only your personal vault.">
        <Switch
          aria-label={`Writes may reach organisations on ${info.conn}`}
          checked={s.write_scope === 'personal_and_org'}
          disabled={set.isPending}
          onCheckedChange={(on) => set.mutate({ key: 'write_scope', value: on ? 'personal_and_org' : 'personal' })}
        />
      </Row>
    </div>
  )
}

/** The harness-wide D20 keys (Settings › Secrets). */
export function ExternalSettingsPanel() {
  const api = useApi()
  const can = useQuerySupport(['get_external_settings', 'set_external_setting'])
  const q = useQuery({ enabled: can, queryKey: queryKeys.secretExternalSettings, queryFn: () => api.get_external_settings() })
  const set = useAction((v: Write<ExternalSettings>) => api.set_external_setting(v.key, v.value), {
    keys: [queryKeys.secretExternalSettings],
  })
  const s = q.data ?? EXTERNAL_DEFAULTS
  return (
    <>
      <Row label="Send file size (MB)" description="The largest file a Send may carry; the server's own limit still applies (500 MB on bitwarden.com).">
        <Input
          key={s.send_max_file_mb}
          type="number"
          min={1}
          max={525}
          defaultValue={s.send_max_file_mb}
          aria-label="Send file size limit in MB"
          className="h-8 w-24"
          disabled={!q.data}
          onBlur={(e) => {
            const n = Number(e.currentTarget.value)
            if (Number.isInteger(n) && n >= 1 && n <= 525 && n !== s.send_max_file_mb) set.mutate({ key: 'send_max_file_mb', value: n })
          }}
        />
      </Row>
      <Row label="Copying a Send link needs your press" description="Off: any owner connection (a script, the TUI) may copy a link without a card.">
        <Switch aria-label="Copying a Send link needs your press" checked={s.copy_link_needs_press} disabled={!q.data || set.isPending} onCheckedChange={(value) => set.mutate({ key: 'copy_link_needs_press', value })} />
      </Row>
    </>
  )
}
