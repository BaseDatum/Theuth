import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronRight, Radar, RefreshCw, Trash2 } from 'lucide-react'
import { Fragment, type KeyboardEvent, type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import {
  builtinKeyNote,
  BUILTIN_LABEL,
  contextPlaceholder,
  freeName,
  isLocalUrl,
  LIMIT_SOURCE,
  type Limits,
  limitsArgs,
  limitsSave,
  localAddArgs,
  type LocalForm,
  loginText,
  orderProviders,
  pairLogins,
  presetForm,
  probeForm,
  probeState,
  refreshedText,
  RUNTIMES,
  runtimeLabel,
} from '@/components/settings/providers'
import { type Action, Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
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
import type { LocalServer } from "@/rpc/bindings/method.agent.output"
import type { ProviderInfo } from "@/rpc/bindings/method.agent.output"
import type { ProviderModel } from "@/rpc/bindings/method.agent.output"
import type { LoginInfo } from "@/rpc/bindings/method.agent.output"
import { on } from '@/store/sessions'

/** 188 A6: Settings › Models › Providers — every model provider with its key or login, then the AWS profile Bedrock signs with. */
export function ModelProviders() {
  return (
    <>
      <ProviderList />
      <Bedrock />
    </>
  )
}

/** A provider's OAuth login (the TUI's `/login`): Log in opens the browser, Log out forgets the tokens. */
function useLogins() {
  const api = useApi()
  const active = useEngines((s) => s.active)
  const logins = useQuery({ queryKey: queryKeys.logins, queryFn: () => api.list_logins() })
  // Over link the daemon opens its own browser on the engine (31 §9): the URL is only shown here, as text.
  const [remote, setRemote] = useState<{ name: string; url: string } | null>(null)
  const login = useAction(async (name: string) => {
    const url = await api.login(name)
    if (active !== LOCAL) setRemote({ name, url })
    else window.open(url, '_blank')
  })
  // A logout changes which providers `list_models` may offer (52 §3.2); a login refreshes via `LoginFinished`.
  const logout = useAction((name: string) => api.logout(name), { keys: [queryKeys.logins, queryKeys.models] })
  useEffect(
    () =>
      on('LoginFinished', (e) => {
        setRemote(null)
        if (e.error) toast.error(`${e.provider}: ${e.error}`)
        else toast.success(`Logged in to ${e.provider}`)
      }),
    [],
  )
  return { list: logins.data, remote, login, logout }
}

type Logins = ReturnType<typeof useLogins>

/** A login's state, or while a remote sign-in waits, the URL to open on the node. */
function LoginState({ l, logins }: { l: LoginInfo; logins: Logins }) {
  const engineName = useConnection((s) => s.info?.name)
  if (logins.remote?.name !== l.name) return <>{loginText(l)}</>
  return (
    <span className="block">
      Sign in from the node ({engineName ?? 'node'}): <code className="select-all break-all font-mono text-xs">{logins.remote.url}</code>
    </span>
  )
}

/** A provider row's login after its key or address; nothing for a provider without one. */
function LoginSuffix({ l, logins }: { l: LoginInfo | undefined; logins: Logins }) {
  if (!l) return null
  return (
    <>
      {' · '}
      <LoginState l={l} logins={logins} />
    </>
  )
}

function LoginButton({ l, logins }: { l: LoginInfo | undefined; logins: Logins }) {
  if (!l) return null
  return (
    <Button variant="outline" size="sm" disabled={logins.login.isPending || logins.logout.isPending} onClick={() => (l.logged_in ? logins.logout.mutate(l.name) : logins.login.mutate(l.name))}>
      {l.logged_in ? 'Log out' : 'Log in'}
    </Button>
  )
}

/** A sub-heading inside the Providers section (177: Local servers, Custom providers). */
const SUB = 'mt-6 text-[13px] font-semibold'

/**
 * One list of model providers (162 D2/D9): the built-in key providers (85 §0.9: OpenCode Zen, OpenRouter, keyed by the
 * `opencode` / `openrouter` secret through the masked `set_secret` entry; the value is never read back) and the OAuth
 * providers, then the local model servers (177 U1), then the other custom OpenAI-compatible providers (key sent once
 * with `add_provider`, never echoed). A custom or local row expands to its models' limits (177 U2). A provider with a
 * login carries it on its row (188 A6); a login with no provider row gets a row of its own after the built-ins.
 */
function ProviderList() {
  const api = useApi()
  const providers = useQuery({
    queryKey: queryKeys.providers,
    queryFn: () => api.list_providers(),
  })
  const logins = useLogins()
  const rows = orderProviders(providers.data)
  // Until the providers answer, every login would look unpaired.
  const paired = pairLogins(rows, providers.data ? (logins.list ?? []) : [])
  const custom = rows.filter((p) => !p.builtin && !p.local)
  const local = rows.filter((p) => !p.builtin && p.local)
  // A provider change changes which models `list_models` may offer (85 §2).
  const keys = [queryKeys.providers, queryKeys.models]
  const [open, setOpen] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<string | null>(null)
  const [value, setValue] = useState('')
  const store = useAction((v: { name: string; value: string }) => api.set_secret(v.name, v.value, builtinKeyNote(v.name)), {
    keys: [...keys, queryKeys.secrets],
    onSuccess: (s) => {
      setValue('')
      setOpen(null)
      toast.success(`Stored ${s.name}; the value is encrypted and never shown`)
    },
  })
  const refresh = useAction((name: string) => api.refresh_provider_models(name), {
    keys,
    onSuccess: (r) => toast.success(refreshedText(r)),
  })
  const [form, setForm] = useState<CustomForm>({ name: '', url: '', key: '' })
  const add = useAction((v: CustomForm) => api.add_provider(v.name.trim(), v.url.trim(), v.key.trim() || null), {
    keys,
    onSuccess: (p) => {
      setForm({ name: '', url: '', key: '' })
      toast.success(`Added ${p.name}: ${p.models} models`)
    },
  })
  const remove = useAction((name: string) => api.remove_provider(name), { keys })
  const names = rows.map((p) => p.name)

  const row = (p: ProviderInfo) => (
    <Fragment key={p.name}>
      <Row
        label={
          <span className="flex items-center gap-2">
            {/* A keyless local server is as ready as a keyed one, a logged-in provider too. */}
            <Dot on={p.configured || p.local || !!paired.byProvider.get(p.name)?.logged_in} />
            {BUILTIN_LABEL[p.name] ?? p.name}
            <Badge variant="outline" className="font-normal">
              {p.builtin ? 'built-in' : p.local ? 'local' : 'custom'}
            </Badge>
          </span>
        }
        description={
          <>
            {p.runtime && `${runtimeLabel(p.runtime)} · `}
            <span className="font-mono">{p.name}/…</span> · {p.models} models ·{' '}
            {!p.builtin ? (
              <span className="font-mono">{p.base_url}</span>
            ) : p.configured ? (
              'key set'
            ) : (
              `no key (secret \`${p.name}\`)`
            )}
            <LoginSuffix l={paired.byProvider.get(p.name)} logins={logins} />
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
          <span className="flex items-center gap-2">
            {!p.builtin && (
              <Button size="sm" variant="ghost" aria-expanded={expanded === p.name} aria-label={`Model limits of ${p.name}`} onClick={() => setExpanded(expanded === p.name ? null : p.name)}>
                <ChevronRight className={expanded === p.name ? 'rotate-90 transition-transform' : 'transition-transform'} /> Limits
              </Button>
            )}
            {p.refreshable && (
              <Button size="sm" variant="outline" disabled={refresh.isPending} onClick={() => refresh.mutate(p.name)}>
                <RefreshCw className={refresh.isPending && refresh.variables === p.name ? 'animate-spin' : undefined} /> Refresh models
              </Button>
            )}
            <LoginButton l={paired.byProvider.get(p.name)} logins={logins} />
            {p.builtin ? (
              <Button size="sm" variant="outline" onClick={() => setOpen(p.name)}>
                {p.configured ? 'Replace key…' : 'Set key…'}
              </Button>
            ) : (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="ghost" size="icon-sm" aria-label={`Remove ${p.name}`} disabled={remove.isPending}>
                    <Trash2 />
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>{`Remove ${p.name}?`}</AlertDialogTitle>
                    <AlertDialogDescription>Its model list and any limits you set are deleted.</AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction onClick={() => remove.mutate(p.name)}>Remove</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            )}
          </span>
        )}
      </Row>
      {expanded === p.name && <ModelLimits provider={p.name} />}
    </Fragment>
  )

  return (
    <Section title="Providers">
      {rows.filter((p) => p.builtin).map(row)}
      {paired.alone.map((l) => (
        <Row
          key={l.name}
          label={
            <span className="flex items-center gap-2">
              <Dot on={l.logged_in} />
              {BUILTIN_LABEL[l.name] ?? l.name}
              <Badge variant="outline" className="font-normal">
                built-in
              </Badge>
            </span>
          }
          description={<LoginState l={l} logins={logins} />}
        >
          <LoginButton l={l} logins={logins} />
        </Row>
      ))}
      <LocalServers names={names} empty={!!providers.data && local.length === 0}>
        {local.map(row)}
      </LocalServers>
      <h4 className={SUB}>Custom providers</h4>
      {custom.map(row)}
      {providers.data && custom.length === 0 && <Empty>No custom providers yet.</Empty>}
      <CustomProviderForm form={form} names={names} onChange={setForm} pending={add.isPending} onSubmit={() => add.mutate(form)} />
    </Section>
  )
}

/** 177 U1: Find running servers (`local_servers_probe`: the engine probes its own 127.0.0.1), the local provider rows, and Add local server. */
function LocalServers({ names, empty, children }: { names: string[]; empty: boolean; children: ReactNode }) {
  const api = useApi()
  const engine = useConnection((s) => s.info?.name) ?? 'this node'
  // A query, not a mutation: the rows stay through a re-probe and a failed one (R1-1). Fetched only on demand.
  const probe = useQuery({ queryKey: queryKeys.localServers, queryFn: () => api.local_servers_probe(), enabled: false, retry: false })
  const [form, setForm] = useState<LocalForm>(() => presetForm('ollama'))
  const add = useAction((f: LocalForm) => api.add_provider(...localAddArgs(f)), {
    keys: [queryKeys.providers, queryKeys.models],
    onSuccess: (p, f) => {
      setForm({ ...presetForm(f.runtime), name: '' })
      toast.success(`Added ${p.name}: ${p.models} models`)
      // The probe's `added_as` follows the new provider.
      if (probe.data) void probe.refetch()
    },
  })
  return (
    <>
      <div className="mt-6 flex items-center justify-between gap-4">
        <h4 className="text-[13px] font-semibold">Local servers</h4>
        <Button size="sm" variant="outline" disabled={probe.isFetching} onClick={() => void probe.refetch()}>
          <Radar className={probe.isFetching ? 'animate-pulse' : undefined} /> Find running servers
        </Button>
      </div>
      <ProbeStatus engine={engine} fetching={probe.isFetching} error={probe.error?.message ?? null} />
      {probe.data && (
        <ProbeRows
          servers={probe.data}
          names={names}
          engine={engine}
          onAdd={(s) => {
            setForm({ ...probeForm(s), name: freeName(probeForm(s).name, names) })
            document.getElementById(LOCAL_NAME_ID)?.focus()
          }}
        />
      )}
      {children}
      {empty && !probe.data && <Empty>{`No local servers yet. Find running servers looks on ${engine}.`}</Empty>}
      <LocalServerForm form={form} names={names} onChange={setForm} pending={add.isPending} onSubmit={() => add.mutate(form)} />
    </>
  )
}

/** The probe's live line: "Looking for servers on <engine>…" while it runs, the error of a failed one (the last rows stay). */
export function ProbeStatus({ engine, fetching, error }: { engine: string; fetching: boolean; error: string | null }) {
  const text = fetching ? `Looking for servers on ${engine}…` : error ? `Could not look for servers: ${error}` : ''
  return (
    <p role="status" className="text-[13px] text-fg-secondary empty:hidden">
      {text}
    </p>
  )
}

/** The probe's answer: one row per default port, Add (prefills the form) or "Added as <name>" while that provider exists. */
export function ProbeRows({ servers, names, engine, onAdd }: { servers: LocalServer[]; names: readonly string[]; engine: string; onAdd(s: LocalServer): void }) {
  if (servers.length === 0) return <Empty>{`Nothing answered on ${engine}.`}</Empty>
  return (
    <div role="group" className="mt-2 rounded-md border border-border-subtle px-3" aria-label="Local servers found">
      {servers.map((s) => (
        <Row
          key={s.base_url}
          label={
            <span className="flex items-center gap-2">
              <Dot on={s.reachable} />
              {runtimeLabel(s.runtime)}
            </span>
          }
          description={
            <>
              <span className="font-mono">{s.base_url}</span> · {probeState(s)}
            </>
          }
        >
          {/* The probe's `added_as` is as old as the probe; a provider removed since gets Add back. */}
          {s.added_as && names.includes(s.added_as) ? (
            <span className="text-sm text-fg-secondary">
              Added as <span className="font-mono">{s.added_as}</span>
            </span>
          ) : (
            <Button size="sm" variant="outline" aria-label={`Add ${runtimeLabel(s.runtime)} at ${s.base_url}`} onClick={() => onAdd(s)}>
              Add
            </Button>
          )}
        </Row>
      ))}
    </div>
  )
}

/** R1-3: adding under an existing name replaces that provider, and an empty key deletes its stored key. */
function ReplaceWarning({ name, names }: { name: string; names: readonly string[] }) {
  const n = name.trim()
  if (!names.includes(n)) return null
  return <p className="col-span-full text-[13px] text-warning">{`Replaces ${n}; an empty key removes its key.`}</p>
}

const LOCAL_NAME_ID = 'local-server-name'

/** Add local server: the runtime fills the name (a free one) and the default address, both editable; the key is optional. */
export function LocalServerForm({ form, names, onChange, onSubmit, pending }: { form: LocalForm; names: readonly string[]; onChange(f: LocalForm): void; onSubmit(): void; pending: boolean }) {
  const ready = !!(form.name.trim() && form.url.trim())
  return (
    <form
      className="mt-3 grid grid-cols-[12rem_1fr_auto] gap-2"
      aria-label="Add local server"
      onSubmit={(e) => {
        e.preventDefault()
        if (ready) onSubmit()
      }}
    >
      <Select
        value={form.runtime}
        onValueChange={(r) => {
          const next = presetForm(r as LocalForm['runtime'])
          onChange({ ...next, name: freeName(next.name, names), key: form.key })
        }}
      >
        <SelectTrigger aria-label="Local server runtime" className="w-full">
          <SelectValue>{runtimeLabel(form.runtime)}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          {RUNTIMES.map((r) => (
            <SelectItem key={r.runtime} value={r.runtime}>
              {r.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Input id={LOCAL_NAME_ID} className="col-span-2" value={form.name} onChange={(e) => onChange({ ...form, name: e.target.value })} placeholder="Name" aria-label="Local server name" spellCheck={false} />
      <Input className="col-span-3 font-mono" value={form.url} onChange={(e) => onChange({ ...form, url: e.target.value })} placeholder="http://127.0.0.1:8080/v1/" aria-label="Local server address" spellCheck={false} />
      <Input
        type="password"
        className="col-span-2"
        value={form.key}
        onChange={(e) => onChange({ ...form, key: e.target.value })}
        placeholder="API key (optional)"
        aria-label="Local server API key"
        aria-describedby="local-server-key-help"
        autoComplete="off"
      />
      <Button type="submit" variant="outline" disabled={!ready || pending}>
        Add local server
      </Button>
      <p id="local-server-key-help" className="col-span-3 text-[13px] text-fg-secondary">
        Only if your server asks for one (omlx with key verification on does)
      </p>
      <ReplaceWarning name={form.name} names={names} />
    </form>
  )
}

export type CustomForm = { name: string; url: string; key: string }

/** Add custom provider (162): the key is required unless the address is local (177 U3, the engine's rule). */
export function CustomProviderForm({ form, names, onChange, onSubmit, pending }: { form: CustomForm; names: readonly string[]; onChange(f: CustomForm): void; onSubmit(): void; pending: boolean }) {
  const local = isLocalUrl(form.url)
  const ready = !!(form.name.trim() && form.url.trim() && (local || form.key.trim()))
  return (
    <form
      className="mt-4 grid grid-cols-[1fr_2fr_2fr_auto] gap-2"
      aria-label="Add custom provider"
      onSubmit={(e) => {
        e.preventDefault()
        if (ready) onSubmit()
      }}
    >
      <Input value={form.name} onChange={(e) => onChange({ ...form, name: e.target.value })} placeholder="Name" aria-label="Custom provider name" spellCheck={false} />
      <Input value={form.url} onChange={(e) => onChange({ ...form, url: e.target.value })} placeholder="https://host/v1/" aria-label="Custom provider base URL" spellCheck={false} />
      <Input
        type="password"
        value={form.key}
        onChange={(e) => onChange({ ...form, key: e.target.value })}
        placeholder={local ? 'API key (optional)' : 'API key'}
        aria-label="Custom provider API key"
        required={!local}
        autoComplete="off"
      />
      <Button type="submit" variant="outline" disabled={!ready || pending}>
        Add custom provider
      </Button>
      {local && (
        <p className="col-span-4 text-[13px] text-fg-secondary">
          Key optional: this is a local address (loopback, private, .local, .lan), where servers usually take no key; none is sent unless you set one.
        </p>
      )}
      <ReplaceWarning name={form.name} names={names} />
    </form>
  )
}

/** 177 U2: a provider's models with Context / Output fields; Save stores them (`set_model_limits`), Clear sets both back to unknown. */
export function ModelLimits({ provider }: { provider: string }) {
  const api = useApi()
  const qc = useQueryClient()
  const key = queryKeys.providerModels(provider)
  const models = useQuery({ queryKey: key, queryFn: () => api.provider_models(provider) })
  const save = useAction((v: Limits) => api.set_model_limits(...limitsArgs(provider, v)), {
    // `list_models` carries the context window the pickers show.
    keys: [queryKeys.models],
    onSuccess: (m) => qc.setQueryData<ProviderModel[]>(key, (old) => old?.map((x) => (x.id === m.id ? m : x))),
  })
  return (
    <div className="border-b border-border-subtle pb-4" role="group" aria-label={`Model limits of ${provider}`}>
      {models.isError ? (
        <Empty>{`Could not list the models: ${models.error.message}`}</Empty>
      ) : !models.data ? (
        <Empty>Listing models…</Empty>
      ) : models.data.length === 0 ? (
        <Empty>No models listed yet; Refresh models asks the server.</Empty>
      ) : (
        <div className="grid grid-cols-[minmax(0,1fr)_13rem_6.5rem_auto] items-center gap-x-2 gap-y-2 text-[13px]">
          <span className="text-fg-tertiary">Model</span>
          <span className="text-fg-tertiary">Context (tokens)</span>
          <span className="text-fg-tertiary">Output (tokens)</span>
          <span />
          {models.data.map((m) => (
            <LimitRow key={`${m.id}\n${m.context}\n${m.output}\n${m.source}`} m={m} save={save} />
          ))}
        </div>
      )}
    </div>
  )
}

/** One model's limits: a field is empty while unknown; Save sends what is typed (empty = `null`), Clear `null` for both. */
function LimitRow({ m, save }: { m: ProviderModel; save: Action<Limits, ProviderModel> }) {
  const [ctx, setCtx] = useState(m.context ? String(m.context) : '')
  const [out, setOut] = useState(m.output ? String(m.output) : '')
  const next = limitsSave(m, ctx, out)
  const busy = save.isPending && save.variables?.id === m.id
  // R1-6: Enter in either field saves, as the Save button would.
  const enter = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && next && !busy) save.mutate(next)
  }
  return (
    <>
      <span className="flex min-w-0 flex-col">
        <span className="truncate font-mono" title={m.id}>
          {m.name}
        </span>
        <Badge variant="outline" className="mt-0.5 font-normal text-fg-secondary">
          {LIMIT_SOURCE[m.source]}
        </Badge>
      </span>
      <Input type="number" min={1} className="h-8 px-2 text-right text-[13px] md:text-[13px]" aria-label={`Context of ${m.id}`} placeholder={contextPlaceholder(m)} value={ctx} onChange={(e) => setCtx(e.target.value)} onKeyDown={enter} />
      <Input type="number" min={1} className="h-8 px-2 text-right text-[13px] md:text-[13px]" aria-label={`Output of ${m.id}`} placeholder="unknown" value={out} onChange={(e) => setOut(e.target.value)} onKeyDown={enter} />
      <span className="flex items-center gap-1">
        <Button size="xs" variant="outline" aria-label={`Save limits of ${m.id}`} disabled={!next || busy} onClick={() => next && save.mutate(next)}>
          Save
        </Button>
        <Button size="xs" variant="ghost" aria-label={`Clear limits of ${m.id}`} disabled={busy || (m.context === 0 && m.output === 0)} onClick={() => save.mutate({ id: m.id, context: null, output: null })}>
          Clear
        </Button>
      </span>
    </>
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
