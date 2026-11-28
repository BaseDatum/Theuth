// Wizard configure step for "Install via SSH": pick a host from ~/.ssh/config or enter one manually,
// probe OS/arch without installing, optionally set a vault master password, and install natively.

import { Check, Loader2, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import { createLabel, formatSshOsArch, vaultError } from './format'
import { Card, DesktopFooter, VaultField, type VaultInput } from './steps'
import type { SshHostEntry, SshHosts, SshOptions, SshProbe } from './types'

export type SshInputMode = 'config' | 'manual'

export interface SshConfigureProps {
  hosts: SshHosts | null
  loading?: boolean
  mode: SshInputMode
  onMode: (mode: SshInputMode) => void
  ssh: SshOptions
  onChange: (next: SshOptions) => void
  onPickHost: (entry: SshHostEntry) => void
  portText: string
  onPortText: (v: string) => void
  portError: string | null
  name: string
  nameError: string | null
  onName: (name: string) => void
  probe: {
    result: SshProbe | null
    error: string | null
    pending: boolean
    onCheck: () => void
  }
  vault?: VaultInput | null
  busy: boolean
  onCreate: () => void
  onBack?: () => void
}

/** One-line connection preview for a `~/.ssh/config` host entry. */
export function hostSubtitle(entry: SshHostEntry): string {
  const parts: string[] = []
  const target = entry.hostname ?? entry.alias
  const userHost = entry.user ? `${entry.user}@${target}` : target
  parts.push(entry.port ? `${userHost}:${entry.port}` : userHost)
  if (entry.proxy_jump) parts.push(`via ${entry.proxy_jump}`)
  return parts.join(' · ')
}

/** Validate an optional port string; empty means default (22 / ~/.ssh/config). */
export function parseSshPort(text: string): { port: number | null; error: string | null } {
  const trimmed = text.trim()
  if (!trimmed) return { port: null, error: null }
  if (!/^\d+$/.test(trimmed)) return { port: null, error: 'Port must be a number between 1 and 65535' }
  const n = Number(trimmed)
  if (!Number.isInteger(n) || n < 1 || n > 65535) return { port: null, error: 'Port must be between 1 and 65535' }
  return { port: n, error: null }
}

/** Suggest a valid node name (`checkName`) from an SSH alias or hostname. */
export function suggestSshNodeName(host: string, taken: readonly string[] = []): string {
  const raw = host
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 36)
    .replace(/-+$/, '')
  const base = raw && /^[a-z]/.test(raw) ? raw : raw ? `n-${raw}`.slice(0, 36).replace(/-+$/, '') : 'ssh-node'
  if (!taken.includes(base)) return base
  for (let i = 2; ; i++) {
    const candidate = `${base}-${i}`
    if (!taken.includes(candidate)) return candidate
  }
}

export function SshConfigure({
  hosts,
  loading,
  mode,
  onMode,
  ssh,
  onChange,
  onPickHost,
  portText,
  onPortText,
  portError,
  name,
  nameError,
  onName,
  probe,
  vault,
  busy,
  onCreate,
  onBack,
}: SshConfigureProps) {
  const configHosts = hosts?.hosts ?? []
  const effectiveMode: SshInputMode = configHosts.length > 0 ? mode : 'manual'
  const hostReady = ssh.host.trim().length > 0 && !portError
  const canCreate = hostReady && !nameError && (!vault || vaultError(vault.password, vault.confirm) === null) && hosts?.ssh_available !== false
  const probeLabel = probe.result?.ok ? formatSshOsArch(probe.result.os, probe.result.arch) : null
  const probeError = probe.error?.trim()
    ? probe.error
    : probe.result && !probe.result.ok
      ? probe.result.error?.trim()
        ? probe.result.error
        : 'Could not reach server'
      : null

  return (
    <>
      {hosts && !hosts.ssh_available && (
        <p role="alert" className="rounded-lg border border-danger-border/50 bg-danger-bg px-3 py-2 text-xs text-danger">
          The system ssh command was not found on PATH. Install an OpenSSH client to use this provider.
        </p>
      )}

      <Card className="flex flex-col gap-2.5">
        <div className="flex items-center justify-between gap-2">
          <div>
            <div className="text-sm font-semibold">Target server</div>
            <div className="text-xs leading-snug text-fg-secondary">Uses your existing SSH access. Keys are never read by theuth.</div>
          </div>
          {configHosts.length > 0 && (
            <div className="flex shrink-0 rounded-lg border border-border bg-layer-2 p-0.5 text-xs">
              <button
                type="button"
                aria-pressed={effectiveMode === 'config'}
                onClick={() => onMode('config')}
                className={cn('rounded-md px-2.5 py-1 font-medium transition-colors', effectiveMode === 'config' ? 'bg-card text-foreground shadow-xs' : 'text-fg-secondary hover:text-foreground')}
              >
                ~/.ssh/config ({configHosts.length})
              </button>
              <button
                type="button"
                aria-pressed={effectiveMode === 'manual'}
                onClick={() => onMode('manual')}
                className={cn('rounded-md px-2.5 py-1 font-medium transition-colors', effectiveMode === 'manual' ? 'bg-card text-foreground shadow-xs' : 'text-fg-secondary hover:text-foreground')}
              >
                Manual
              </button>
            </div>
          )}
        </div>

        {loading && configHosts.length === 0 ? (
          <div className="text-xs text-fg-secondary">Reading ~/.ssh/config…</div>
        ) : effectiveMode === 'config' ? (
          <div role="radiogroup" aria-label="SSH host" className="grid max-h-44 grid-cols-2 gap-2 overflow-y-auto pr-0.5">
            {configHosts.map((entry) => {
              const selected = ssh.host === entry.alias
              return (
                <button
                  key={entry.alias}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  data-ssh-host={entry.alias}
                  onClick={() => onPickHost(entry)}
                  className={cn(
                    'flex items-center gap-2 rounded-lg border border-border bg-background px-2.5 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50',
                    selected && 'border-primary ring-1 ring-primary',
                  )}
                >
                  <Server className="size-3.5 shrink-0 text-fg-secondary" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-semibold">{entry.alias}</div>
                    <div className="truncate text-[11px] text-fg-secondary">{hostSubtitle(entry)}</div>
                  </div>
                </button>
              )
            })}
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-2.5">
            <div>
              <label htmlFor="ssh-host" className="text-xs text-fg-secondary">
                Host
              </label>
              <Input
                id="ssh-host"
                aria-label="Host"
                className="mt-1 h-8"
                placeholder="server.example.com or 10.0.0.5"
                spellCheck={false}
                value={ssh.host}
                onChange={(e) => onChange({ ...ssh, host: e.currentTarget.value })}
              />
            </div>
            <div>
              <label htmlFor="ssh-user" className="text-xs text-fg-secondary">
                User (optional)
              </label>
              <Input
                id="ssh-user"
                aria-label="User"
                className="mt-1 h-8"
                placeholder="Uses your SSH default"
                spellCheck={false}
                value={ssh.user ?? ''}
                onChange={(e) => onChange({ ...ssh, user: e.currentTarget.value.trim() || null })}
              />
            </div>
            <div>
              <label htmlFor="ssh-port" className="text-xs text-fg-secondary">
                Port
              </label>
              <Input
                id="ssh-port"
                aria-label="Port"
                className="mt-1 h-8"
                inputMode="numeric"
                placeholder="22"
                aria-invalid={!!portError || undefined}
                value={portText}
                onChange={(e) => onPortText(e.currentTarget.value)}
              />
              {portError && <p className="mt-1 text-xs text-danger">{portError}</p>}
            </div>
            <div>
              <label htmlFor="ssh-identity" className="text-xs text-fg-secondary">
                Identity file (optional)
              </label>
              <Input
                id="ssh-identity"
                aria-label="Identity file"
                className="mt-1 h-8"
                placeholder="~/.ssh/id_ed25519"
                spellCheck={false}
                value={ssh.identity ?? ''}
                onChange={(e) => onChange({ ...ssh, identity: e.currentTarget.value.trim() || null })}
              />
            </div>
          </div>
        )}

        <div className="flex flex-wrap items-center gap-2 pt-0.5">
          <Button type="button" size="sm" variant="outline" disabled={!hostReady || probe.pending || hosts?.ssh_available === false} onClick={probe.onCheck}>
            {probe.pending && <Loader2 className="animate-spin" />}
            Check server
          </Button>
          {probeLabel && (
            <span data-ssh-probe="ok" className="inline-flex items-center gap-1 text-xs font-medium text-success">
              <Check className="size-3.5" />
              {probeLabel}
            </span>
          )}
        </div>
        {probeError && (
          <p
            data-ssh-probe="error"
            role="alert"
            className="w-full rounded-lg border border-danger-border/50 bg-danger-bg px-3 py-2 font-mono text-xs whitespace-pre-wrap break-words text-danger"
          >
            {probeError}
          </p>
        )}
      </Card>

      <Card>
        <div className="text-xs leading-snug text-fg-secondary">Node name</div>
        <Input
          className="mt-1 h-8"
          aria-label="Name"
          aria-invalid={!!nameError || undefined}
          spellCheck={false}
          value={name}
          onChange={(e) => onName(e.currentTarget.value)}
        />
        {nameError && <p className="mt-1 text-xs text-danger">{nameError}</p>}
      </Card>

      {vault && <VaultField row vault={vault} />}

      <DesktopFooter
        left="Installs natively as your SSH user · no sudo · no open ports"
        onBack={onBack}
        action={createLabel({ provider: 'ssh', k8s: null }, null)}
        disabled={!canCreate}
        busy={busy}
        onAction={onCreate}
      />
    </>
  )
}
