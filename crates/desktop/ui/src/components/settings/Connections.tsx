import { useQuery } from '@tanstack/react-query'
import { Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { builtinKeyNote, BUILTIN_LABEL, splitProviders } from '@/components/settings/providers'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
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
import { queryKeys, useApi } from '@/lib/rpc'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

export function Connections() {
  return (
    <>
      <Logins />
      <BuiltIns />
      <Gateways />
      <Bedrock />
    </>
  )
}

/** OAuth providers and OAuth MCP servers in one list, as the TUI's `/login` menu. */
function Logins() {
  const api = useApi()
  const active = useEngines((s) => s.active)
  const engineName = useConnection((s) => s.info?.name)
  const logins = useQuery({
    queryKey: queryKeys.logins,
    queryFn: () => api.list_logins(),
  })
  const mcp = useQuery({
    queryKey: queryKeys.mcp(null),
    queryFn: () => api.list_mcp(null),
  })
  // A logout changes which providers `list_models` may offer (52 §3.2); a login refreshes via `LoginFinished`.
  const keys = [queryKeys.logins, queryKeys.mcp(null), queryKeys.models]
  // Over link the daemon opens its own browser on the engine (31 §9): the URL is only shown here, as text.
  const [remote, setRemote] = useState<{ name: string; url: string } | null>(null)
  const login = useAction(async (v: { name: string; mcp: boolean }) => {
    const url = await (v.mcp ? api.mcp_login(v.name) : api.login(v.name))
    if (active !== LOCAL) setRemote({ name: v.name, url })
    else window.open(url, '_blank')
  })
  const logout = useAction(
    (v: { name: string; mcp: boolean }) => (v.mcp ? api.mcp_logout(v.name) : api.logout(v.name)),
    {
      keys,
    },
  )

  useEffect(
    () =>
      on('LoginFinished', (e) => {
        setRemote(null)
        if (e.error) toast.error(`${e.provider}: ${e.error}`)
        else toast.success(`Logged in to ${e.provider}`)
      }),
    [],
  )

  const rows = [
    ...(logins.data ?? []).map((l) => ({
      name: l.name,
      mcp: false,
      on: l.logged_in,
      status: !l.logged_in ? 'not logged in' : l.account ? `logged in (${l.account})` : 'logged in',
    })),
    ...(mcp.data ?? [])
      .filter((s) => s.auth !== 'none')
      .map((s) => ({
        name: s.name,
        mcp: true,
        on: s.auth === 'logged_in',
        status: s.auth === 'logged_in' ? 'logged in' : 'login required',
      })),
  ]

  return (
    <Section title="Accounts">
      {logins.isSuccess && mcp.isSuccess && rows.length === 0 && (
        <Empty>Nothing to log in to.</Empty>
      )}
      {rows.map((r) => (
        <Row
          key={`${r.mcp ? 'mcp' : 'provider'}:${r.name}`}
          label={
            <span className="flex items-center gap-2">
              <Dot on={r.on} />
              {r.name}
              <Badge variant="outline" className="font-normal">
                {r.mcp ? 'mcp' : 'provider'}
              </Badge>
            </span>
          }
          description={
            remote?.name === r.name ? (
              <>
                Sign in from the node ({engineName ?? 'node'}):{' '}
                <code className="select-all break-all font-mono text-xs">{remote.url}</code>
              </>
            ) : (
              r.status
            )
          }
        >
          <Button
            variant="outline"
            size="sm"
            disabled={login.isPending || logout.isPending}
            onClick={() => (r.on ? logout.mutate(r) : login.mutate(r))}
          >
            {r.on ? 'Logout' : 'Login'}
          </Button>
        </Row>
      ))}
    </Section>
  )
}

/**
 * The built-in key providers (85 §0.9): OpenCode Zen and OpenRouter, keyed by the
 * `opencode` / `openrouter` secret through the same masked `set_secret` entry the
 * other Settings key forms use; the value is never read back.
 */
function BuiltIns() {
  const api = useApi()
  const providers = useQuery({
    queryKey: queryKeys.providers,
    queryFn: () => api.list_providers(),
  })
  const [open, setOpen] = useState<string | null>(null)
  const [value, setValue] = useState('')
  const store = useAction((v: { name: string; value: string }) => api.set_secret(v.name, v.value, builtinKeyNote(v.name)), {
    // A new key changes which models `list_models` may offer (85 §2).
    keys: [queryKeys.providers, queryKeys.models, queryKeys.secrets],
    onSuccess: (s) => {
      setValue('')
      setOpen(null)
      toast.success(`Stored ${s.name}; the value is encrypted and never shown`)
    },
  })

  return (
    <Section title="Built-in providers">
      {splitProviders(providers.data).builtins.map((p) => (
        <Row
          key={p.name}
          label={
            <span className="flex items-center gap-2">
              <Dot on={p.configured} />
              {BUILTIN_LABEL[p.name] ?? p.name}
            </span>
          }
          description={
            <>
              <span className="font-mono">{p.name}/…</span> · {p.models} models ·{' '}
              {p.configured ? 'key set' : `no key (secret \`${p.name}\`)`}
            </>
          }
        >
          {open === p.name ? (
            <form
              className="flex items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault()
                if (value.trim()) store.mutate({ name: p.name, value: value.trim() })
              }}
            >
              <Input
                type="password"
                autoComplete="off"
                autoFocus
                className="min-w-48"
                aria-label={`value of ${p.name}`}
                placeholder="Paste the key"
                value={value}
                onChange={(e) => setValue(e.target.value)}
              />
              <Button type="submit" size="sm" disabled={!value.trim() || store.isPending}>
                Store
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => setOpen(null)}>
                Cancel
              </Button>
            </form>
          ) : (
            <Button size="sm" variant="outline" onClick={() => setOpen(p.name)}>
              {p.configured ? 'Replace key…' : 'Set key…'}
            </Button>
          )}
        </Row>
      ))}
    </Section>
  )
}

/** Named OpenAI-compatible gateways; the key is sent once and never echoed. */
function Gateways() {
  const api = useApi()
  const providers = useQuery({
    queryKey: queryKeys.providers,
    queryFn: () => api.list_providers(),
  })
  const gateways = splitProviders(providers.data).gateways
  const [form, setForm] = useState({ name: '', url: '', key: '' })
  const add = useAction((v: typeof form) => api.add_provider(v.name, v.url, v.key), {
    keys: [queryKeys.providers, queryKeys.models],
    onSuccess: (p) => {
      setForm({ name: '', url: '', key: '' })
      toast.success(`Added ${p.name}: ${p.models} models`)
    },
  })
  const remove = useAction((name: string) => api.remove_provider(name), {
    keys: [queryKeys.providers, queryKeys.models],
  })
  const ready = form.name.trim() && form.url.trim() && form.key.trim()

  return (
    <Section title="Gateways">
      {providers.data && gateways.length === 0 && <Empty>No gateway configured.</Empty>}
      {gateways.map((p) => (
        <Row
          key={p.name}
          label={p.name}
          description={
            <>
              <span className="font-mono">{p.base_url}</span> · {p.models} models
            </>
          }
        >
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Remove ${p.name}`}
            disabled={remove.isPending}
            onClick={() => remove.mutate(p.name)}
          >
            <Trash2 />
          </Button>
        </Row>
      ))}
      <form
        className="mt-4 grid grid-cols-[1fr_2fr_2fr_auto] gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (ready)
            add.mutate({
              name: form.name.trim(),
              url: form.url.trim(),
              key: form.key.trim(),
            })
        }}
      >
        <Input
          value={form.name}
          onChange={(e) => setForm({ ...form, name: e.target.value })}
          placeholder="Name"
          aria-label="Gateway name"
          spellCheck={false}
        />
        <Input
          value={form.url}
          onChange={(e) => setForm({ ...form, url: e.target.value })}
          placeholder="https://host/v1/"
          aria-label="Base URL"
          spellCheck={false}
        />
        <Input
          type="password"
          value={form.key}
          onChange={(e) => setForm({ ...form, key: e.target.value })}
          placeholder="API key"
          aria-label="API key"
          autoComplete="off"
        />
        <Button type="submit" variant="outline" disabled={!ready || add.isPending}>
          Add
        </Button>
      </form>
    </Section>
  )
}

/** `Select` value of the default credential chain; profiles are `p:<name>` so no profile name can collide. */
const CHAIN = 'chain'

/** The AWS profile Bedrock signs with, engine-wide (the TUI's `/aws`); credentials never leave the engine. */
function Bedrock() {
  const api = useApi()
  const engineName = useConnection((s) => s.info?.name)
  const aws = useQuery({
    queryKey: queryKeys.awsProfiles,
    queryFn: () => api.list_aws_profiles(),
  })
  const set = useAction((profile: string | null) => api.set_aws_profile(profile), {
    keys: [queryKeys.awsProfiles, queryKeys.models],
    onSuccess: (r) => toast.success(r.selected ? `Bedrock uses profile ${r.selected}` : 'Bedrock uses the AWS default chain'),
  })
  const d = aws.data
  const stale = !!d?.selected && !d.profiles.includes(d.selected)
  // Something to pick, or a stale explicit choice to clear; otherwise the status line says it all.
  const pick = !!d && (d.profiles.length > 0 || !!d.selected)

  return (
    <Section title="AWS Bedrock">
      <Row
        label="Profile"
        description={
          !d
            ? aws.error
              ? `Could not list profiles: ${aws.error.message}`
              : 'Listing profiles…'
            : stale
              ? `Profile ${d.selected} is selected but not found on ${engineName ?? 'the node'}; pick another or the default chain.`
              : d.selected
                ? `Every Bedrock model on ${engineName ?? 'this node'} signs with profile ${d.selected}.`
                : `Bedrock on ${engineName ?? 'this node'} uses a configured API key, otherwise the AWS default credential chain (${d.default_profile}). Select a profile to use its credentials from the next request.`
        }
      >
        {pick && (
          <Select
            value={d.selected ? `p:${d.selected}` : CHAIN}
            disabled={set.isPending}
            onValueChange={(v) => set.mutate(v === CHAIN ? null : v.slice(2))}
          >
            <SelectTrigger aria-label="AWS inference profile" className="max-w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={CHAIN}>AWS default chain ({d.default_profile})</SelectItem>
              {d.profiles.map((p) => (
                <SelectItem key={p} value={`p:${p}`}>
                  {p}
                </SelectItem>
              ))}
              {stale && <SelectItem value={`p:${d.selected}`}>{d.selected} (not found)</SelectItem>}
            </SelectContent>
          </Select>
        )}
      </Row>
    </Section>
  )
}
