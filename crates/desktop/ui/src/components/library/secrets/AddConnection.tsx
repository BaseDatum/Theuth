import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { queryKeys } from '@/lib/rpc'
import { statusDetail, statusLabel } from './labels'
import { type LoginMethod, LoginCard, SSO_HINT, UnlockCard } from './UnlockCard'
import type { BwServer, ConnConfig } from '@/rpc/bindings/method.secrets.input'
import type { ConnInfo, ConnStatus } from '@/rpc/bindings/method.secrets.output'
import type { Backend } from './types'

/** Secrets Manager regions offered by the region select (§9.1). */
export const AWS_REGIONS = [
  'us-east-1', 'us-east-2', 'us-west-1', 'us-west-2', 'ca-central-1', 'sa-east-1',
  'eu-west-1', 'eu-west-2', 'eu-west-3', 'eu-central-1', 'eu-central-2', 'eu-north-1', 'eu-south-1', 'eu-south-2',
  'ap-south-1', 'ap-south-2', 'ap-northeast-1', 'ap-northeast-2', 'ap-northeast-3', 'ap-southeast-1', 'ap-southeast-2',
  'ap-southeast-3', 'ap-southeast-4', 'ap-east-1', 'me-south-1', 'me-central-1', 'af-south-1', 'il-central-1',
] as const

/** `CONN` = `[a-z0-9-]{1,32}` (§3.1). */
export const CONN_NAME = /^[a-z0-9-]{1,32}$/

const SERVER_LABEL: Record<BwServer, string> = { us: 'US cloud', eu: 'EU cloud', self_hosted: 'Self-hosted' }
const SELECT = 'h-8 rounded-md border border-border-subtle bg-transparent px-1.5 text-sm'

type Stage = 'form' | 'login' | 'unlock'

/**
 * §9.1 Add connection. AWS: name → profile (`list_aws_profiles`, `(default chain)` first) → region → optional KMS key and
 * prefix → Test (`probe_secret_connection`) → Save. Bitwarden: name → server (US / EU / self-hosted base URL, Advanced
 * for API, identity and web vault URLs) → email (prelogin runs on blur, so a bad server or account fails here) → login
 * method → Save, then the login card and the unlock card. The connection is saved before the login because
 * `connection_login` needs it; Save seeds its always-ask entries first (§6.3).
 */
export function AddConnection({ existing, onDone }: { existing: readonly string[]; onDone: () => void }) {
  const api = useApi()
  const qc = useQueryClient()
  const canProfiles = useQuerySupport(['list_aws_profiles'])
  const [backend, setBackend] = useState<Backend>('bw')
  const [conn, setConn] = useState('')
  // No region is preselected: `list_aws_profiles` carries no profile region (§9.1's "default from the profile"), and a
  // wrong guess lists an empty account.
  const [aws, setAws] = useState({ profile: '', region: '', kms: '', prefix: '' })
  const [bw, setBw] = useState({ server: 'us' as BwServer, base: '', api: '', identity: '', web: '', email: '', login: 'api_key' as LoginMethod, org: '' })
  const [advanced, setAdvanced] = useState(false)
  const [probe, setProbe] = useState<ConnStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [stage, setStage] = useState<Stage>('form')
  const [saved, setSaved] = useState<ConnInfo | null>(null)
  const profiles = useQuery({ enabled: canProfiles && backend === 'aws', queryKey: ['aws-profiles'], queryFn: () => api.list_aws_profiles() })
  const name = conn.trim()
  const nameError = !name ? null : !CONN_NAME.test(name) ? 'a–z, 0–9 and -, at most 32' : existing.includes(name) ? 'already a connection' : null
  const cfg = (): ConnConfig =>
    backend === 'aws'
      ? { backend, profile: aws.profile || null, region: aws.region, kms_key_id: aws.kms.trim() || null, prefix: aws.prefix.trim() || null }
      : {
          backend,
          server: bw.server,
          base_url: bw.server === 'self_hosted' ? bw.base.trim() || null : null,
          api_url: bw.api.trim() || null,
          identity_url: bw.identity.trim() || null,
          web_vault_url: bw.web.trim() || null,
          email: bw.email.trim(),
          login: bw.login,
          org_identifier: bw.login === 'sso' ? bw.org.trim() || null : null,
        }
  const ready = name && !nameError && (backend === 'aws' ? aws.region : bw.email.trim() && (bw.server !== 'self_hosted' || bw.base.trim().startsWith('https://')))
  const run = async <T,>(f: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true)
    setError(null)
    try {
      return await f()
    } catch (e) {
      setError(errorText(e))
      return undefined
    } finally {
      setBusy(false)
    }
  }
  const test = () => void run(async () => setProbe(await api.probe_secret_connection(name, cfg())))
  const save = () =>
    void run(async () => {
      const info = await api.add_secret_connection(name, cfg())
      void qc.invalidateQueries({ queryKey: queryKeys.secretConnections })
      setSaved(info)
      if (info.backend === 'aws') onDone()
      else setStage(info.status.state === 'logged_out' ? 'login' : 'unlock')
    })
  if (saved && stage === 'login')
    return <LoginCard conn={saved.conn} email={bw.email.trim()} method={bw.login} org={bw.org.trim() || null} onDone={() => setStage('unlock')} onCancel={onDone} />
  if (saved && stage === 'unlock')
    return (
      <UnlockCard
        conn={saved.conn}
        account={bw.email.trim()}
        onUnlock={async (pw) => {
          await api.connection_unlock(saved.conn, pw)
          void qc.invalidateQueries({ queryKey: queryKeys.secretConnections })
          onDone()
        }}
        onCancel={onDone}
      />
    )
  return (
    <form
      className="flex flex-col gap-2 rounded-xl border border-border-subtle px-4 py-3 text-sm"
      onSubmit={(e) => {
        e.preventDefault()
        if (ready) save()
      }}
    >
      <ToggleGroup type="single" variant="outline" size="sm" aria-label="Backend" value={backend} onValueChange={(b) => b && (setBackend(b as Backend), setProbe(null))}>
        <ToggleGroupItem value="bw">Bitwarden</ToggleGroupItem>
        <ToggleGroupItem value="aws">AWS Secrets Manager</ToggleGroupItem>
      </ToggleGroup>
      <Input
        value={conn}
        onChange={(e) => setConn(e.target.value)}
        placeholder="Name (personal, prod)"
        aria-label="Connection name"
        spellCheck={false}
      />
      {nameError && <div className="text-xs text-danger">{nameError}</div>}
      {backend === 'aws' ? (
        <>
          <select aria-label="AWS profile" className={SELECT} value={aws.profile} onChange={(e) => setAws({ ...aws, profile: e.target.value })}>
            <option value="">(default chain)</option>
            {profiles.data?.profiles.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <select aria-label="AWS region" className={SELECT} value={aws.region} onChange={(e) => setAws({ ...aws, region: e.target.value })}>
            <option value="" disabled>
              Region…
            </option>
            {AWS_REGIONS.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
          <Input value={aws.kms} onChange={(e) => setAws({ ...aws, kms: e.target.value })} placeholder="KMS key for new secrets (optional, alias/app)" aria-label="KMS key" spellCheck={false} />
          <Input
            value={aws.prefix}
            onChange={(e) => setAws({ ...aws, prefix: e.target.value })}
            placeholder="Name prefix to browse (optional, app/)"
            aria-label="Prefix"
            spellCheck={false}
          />
          <div className="text-xs text-fg-tertiary">Credentials stay in ~/.aws; an expired SSO session needs `aws sso login` outside theuth.</div>
        </>
      ) : (
        <>
          <ToggleGroup type="single" variant="outline" size="sm" aria-label="Server" value={bw.server} onValueChange={(s) => s && setBw({ ...bw, server: s as BwServer })}>
            {(Object.keys(SERVER_LABEL) as BwServer[]).map((s) => (
              <ToggleGroupItem key={s} value={s}>
                {SERVER_LABEL[s]}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          {bw.server === 'self_hosted' && (
            <>
              <Input
                value={bw.base}
                onChange={(e) => setBw({ ...bw, base: e.target.value })}
                placeholder="https://vault.example.org"
                aria-label="Server URL"
                spellCheck={false}
              />
              {bw.base.trim() && !bw.base.trim().startsWith('https://') && <div className="text-xs text-danger">Bitwarden's client talks HTTPS only.</div>}
              <div className="text-xs text-fg-tertiary">A private CA must be in this Mac's trust store (Keychain Access › System); there is no per-connection CA file.</div>
            </>
          )}
          <button type="button" className="self-start text-xs text-fg-secondary underline" onClick={() => setAdvanced(!advanced)}>
            {advanced ? 'Hide advanced' : 'Advanced'}
          </button>
          {advanced && (
            <>
              <Input value={bw.api} onChange={(e) => setBw({ ...bw, api: e.target.value })} placeholder="API URL (default <server>/api)" aria-label="API URL" spellCheck={false} />
              <Input value={bw.identity} onChange={(e) => setBw({ ...bw, identity: e.target.value })} placeholder="Identity URL (default <server>/identity)" aria-label="Identity URL" spellCheck={false} />
              <Input value={bw.web} onChange={(e) => setBw({ ...bw, web: e.target.value })} placeholder="Web vault URL recipients reach (Send links)" aria-label="Web vault URL" spellCheck={false} />
            </>
          )}
          <Input
            value={bw.email}
            onChange={(e) => setBw({ ...bw, email: e.target.value })}
            onBlur={() => name && !nameError && bw.email.trim() && test()}
            placeholder="Account email"
            aria-label="Account email"
            spellCheck={false}
          />
          <ToggleGroup type="single" variant="outline" size="sm" aria-label="Login method" value={bw.login} onValueChange={(l) => l && setBw({ ...bw, login: l as LoginMethod })}>
            <ToggleGroupItem value="api_key">API key (recommended)</ToggleGroupItem>
            <ToggleGroupItem value="password">Password</ToggleGroupItem>
            <ToggleGroupItem value="sso">Log in with SSO</ToggleGroupItem>
          </ToggleGroup>
          {bw.login === 'password' && <div className="text-xs text-fg-tertiary">{SSO_HINT}</div>}
          {bw.login === 'sso' && (
            <Input
              value={bw.org}
              onChange={(e) => setBw({ ...bw, org: e.target.value })}
              placeholder="Organisation SSO identifier (optional on Vaultwarden)"
              aria-label="SSO organisation identifier"
              spellCheck={false}
            />
          )}
        </>
      )}
      {probe && (
        <div className={`text-xs ${probe.state === 'ready' || probe.state === 'locked' || probe.state === 'logged_out' ? 'text-fg-secondary' : 'text-danger'}`}>
          Test: {statusLabel(probe)}
          {statusDetail(probe) && ` · ${statusDetail(probe)}`}
        </div>
      )}
      {error && <div className="text-xs text-danger">{error}</div>}
      <div className="flex gap-2">
        <Button type="button" size="sm" variant="outline" disabled={busy || !ready} onClick={test}>
          Test
        </Button>
        <Button type="submit" size="sm" disabled={busy || !ready}>
          Save
        </Button>
        <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
