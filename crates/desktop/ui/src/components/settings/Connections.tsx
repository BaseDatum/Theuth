import { useQuery } from '@tanstack/react-query'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'

export function Connections() {
  return (
    <>
      <Logins />
      <Gateways />
    </>
  )
}

/** OAuth providers and OAuth MCP servers in one list, as the TUI's `/login` menu. */
function Logins() {
  const api = useApi()
  const logins = useQuery({ queryKey: queryKeys.logins, queryFn: () => api.list_logins() })
  const mcp = useQuery({ queryKey: queryKeys.mcp(null), queryFn: () => api.list_mcp(null) })
  const keys = [queryKeys.logins, queryKeys.mcp(null)]
  const login = useAction((v: { name: string; mcp: boolean }) =>
    (v.mcp ? api.mcp_login(v.name) : api.login(v.name)).then((url) => openUrl(url)),
  )
  const logout = useAction((v: { name: string; mcp: boolean }) => (v.mcp ? api.mcp_logout(v.name) : api.logout(v.name)), {
    keys,
  })

  useEffect(
    () =>
      on('LoginFinished', (e) => {
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
      {logins.isSuccess && mcp.isSuccess && rows.length === 0 && <Empty>Nothing to log in to.</Empty>}
      {rows.map((r) => (
        <Row
          key={`${r.mcp ? 'mcp' : 'provider'}:${r.name}`}
          label={
            <span className="flex items-center gap-2">
              {r.name}
              <Badge variant="outline" className="font-normal">
                {r.mcp ? 'mcp' : 'provider'}
              </Badge>
            </span>
          }
          description={r.status}
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

/** Named OpenAI-compatible gateways; the key is sent once and never echoed. */
function Gateways() {
  const api = useApi()
  const providers = useQuery({ queryKey: queryKeys.providers, queryFn: () => api.list_providers() })
  const [form, setForm] = useState({ name: '', url: '', key: '' })
  const add = useAction((v: typeof form) => api.add_provider(v.name, v.url, v.key), {
    keys: [queryKeys.providers, queryKeys.models],
    onSuccess: (p) => {
      setForm({ name: '', url: '', key: '' })
      toast.success(`Added ${p.name}: ${p.models} models`)
    },
  })
  const remove = useAction((name: string) => api.remove_provider(name), { keys: [queryKeys.providers, queryKeys.models] })
  const ready = form.name.trim() && form.url.trim() && form.key.trim()

  return (
    <Section title="Gateways">
      {providers.data?.length === 0 && <Empty>No gateway configured.</Empty>}
      {providers.data?.map((p) => (
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
          if (ready) add.mutate({ name: form.name.trim(), url: form.url.trim(), key: form.key.trim() })
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
