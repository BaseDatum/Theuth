import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowRight, Check, ChevronDown, Cpu, KeyRound, LoaderCircle, Radar, Sparkles } from 'lucide-react'
import { RadioGroup } from 'radix-ui'
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { freeName, localAddArgs, probeForm, runtimeLabel } from '@/components/settings/providers'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { isIos, openExternal } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { closeAuthBrowser, onAuthBrowserDismissed, openAuthBrowser } from '@/lib/tauri'
import { LOCAL, useEngines } from '@/link/engines'
import { useQuerySupport } from '@/rpc/capabilities'
import type { LocalServer } from '@/rpc/bindings/method.agent.output'
import { on } from '@/store/sessions'
import { CARD, Facts, StepFrame } from './Frame'
import { type ConnectMode as Mode, KEY_PROVIDERS, type Login, MODE_FACTS, MORE_SUBSCRIPTIONS, SUBSCRIPTIONS, type Subscription, connectedProviders, keyNote, loginFinished, providerLabel } from './content'

const MODES: readonly { id: Mode; icon: typeof Sparkles; label: string; hint: string }[] = [
  { id: 'subscription', icon: Sparkles, label: 'Subscription', hint: 'Use your plan, no per-token bill' },
  { id: 'key', icon: KeyRound, label: 'API key', hint: 'Pay per use with your own key' },
  { id: 'local', icon: Cpu, label: 'Local model', hint: 'Runs on this computer, offline' },
]

/** A change in any of these changes what `list_models` offers. */
const KEYS = [queryKeys.models, queryKeys.providers, queryKeys.logins] as const

/** Step 2: a subscription sign-in, an API key or a local server; Continue once `list_models` offers something. */
export function ConnectModel({ onConnected, onBack, onNext }: { onConnected(provider: string): void; onBack(): void; onNext(): void }) {
  const api = useApi()
  const active = useEngines((s) => s.active)
  const models = useModels()
  const list = models.data ?? []
  const ready = list.length > 0
  const [mode, setMode] = useState<Mode>('subscription')
  const [another, setAnother] = useState(false)
  const qc = useQueryClient()
  const connected = useCallback(
    (provider: string) => {
      for (const key of KEYS) void qc.invalidateQueries({ queryKey: key })
      onConnected(provider)
    },
    [qc, onConnected],
  )
  // The sign-in lives here, not in its panel: switching to API key or Local while the browser is open must not drop
  // the `LoginFinished` that ends it.
  const [login, setLogin] = useState<Login>({ pending: null, failed: null })
  const loginRef = useRef(login)
  loginRef.current = login
  const apiRef = useRef(api)
  apiRef.current = api
  const activeRef = useRef(active)
  activeRef.current = active
  useEffect(
    () =>
      on('agent.login@1', 'LoginFinished', (e) => {
        const { next, provider } = loginFinished(loginRef.current, e)
        if (next === loginRef.current) return
        setLogin(next)
        if (isIos()) void closeAuthBrowser()
        if (provider) connected(provider)
      }),
    [connected],
  )
  useEffect(() => {
    if (!isIos()) return
    const off = onAuthBrowserDismissed(() => {
      const cur = loginRef.current.pending
      if (!cur || activeRef.current !== LOCAL) return
      setLogin({ pending: null, failed: null })
      void apiRef.current.cancel_login(cur.ns).catch(() => undefined)
    })
    return () => {
      void off.then((f) => f())
    }
  }, [])
  const ModeIcon = MODES.find((m) => m.id === mode)?.icon ?? Sparkles
  return (
    <StepFrame
      eyebrow="Step 1 of 3"
      title="Connect a model"
      description="theuth thinks with the model you bring. A subscription you already pay for is the easiest start."
      aside={
        ready && !another ? (
          <Facts
            icon={<Check className="size-5" />}
            title="Ready to go"
            facts={connectedProviders(list).map((p) => {
              const n = list.filter((m) => m.provider === p).length
              return `${providerLabel(p)}: ${n} ${n === 1 ? 'model' : 'models'}`
            })}
          />
        ) : (
          <div key={mode}>
            <Facts icon={<ModeIcon className="size-5" />} title={MODE_FACTS[mode].title} facts={MODE_FACTS[mode].facts} />
          </div>
        )
      }
      back={
        <>
          <Button variant="ghost" onClick={onBack} className="text-fg-secondary">
            Back
          </Button>
          {!ready && (
            <Button variant="ghost" onClick={onNext} className="text-fg-secondary">
              Skip for now
            </Button>
          )}
        </>
      }
      primary={
        <Button size="lg" onClick={onNext} disabled={!ready}>
          Continue <ArrowRight />
        </Button>
      }
    >
      {ready && (
        <div role="status" className="mb-6 flex max-w-2xl items-start gap-3 rounded-xl border border-success-border bg-success-bg px-4 py-3">
          <Check className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
          <div className="flex min-w-0 flex-1 flex-col gap-0.5 text-sm">
            <span className="font-medium text-foreground">You're connected</span>
            <span className="text-fg-secondary">
              {connectedProviders(list).map(providerLabel).join(', ')} · {list.length} models ready
            </span>
          </div>
          {!another && (
            <Button variant="ghost" size="sm" onClick={() => setAnother(true)} aria-expanded={false}>
              Connect another
            </Button>
          )}
        </div>
      )}
      {(!ready || another) && (
        <div className="flex max-w-2xl flex-col gap-5">
          <RadioGroup.Root value={mode} onValueChange={(v) => setMode(v as Mode)} aria-label="How to connect" className="grid gap-3 sm:grid-cols-3">
            {MODES.map(({ id, icon: Icon, label, hint }) => (
              <RadioGroup.Item key={id} value={id} className={`${CARD} flex flex-col gap-1.5 p-4 data-[state=checked]:border-fg-accent data-[state=checked]:bg-layer-3 data-[state=checked]:shadow-[inset_0_0_0_1px_var(--fg-accent)]`}>
                {/* Its own line above the title: a tag beside it does not fit a third of the column. */}
                {id === 'subscription' && <span className="w-fit rounded-full bg-layer-1 px-2 py-0.5 text-[11px] font-medium text-fg-accent">Recommended</span>}
                <span className="flex items-center gap-2 text-sm font-medium text-foreground">
                  <Icon className="size-4 shrink-0 text-fg-accent" aria-hidden /> {label}
                </span>
                <span className="text-[13px] text-fg-secondary">{hint}</span>
              </RadioGroup.Item>
            ))}
          </RadioGroup.Root>
          <div key={mode} className="onb-fade">
            {mode === 'subscription' ? <Subscriptions login={login} setLogin={setLogin} /> : mode === 'key' ? <ApiKey onConnected={connected} /> : <LocalModels onConnected={connected} />}
          </div>
        </div>
      )}
    </StepFrame>
  )
}

/** The sign-in cards: `login(ns)` opens the browser on the engine's host; `LoginFinished` (in `ConnectModel`) ends the wait. */
function Subscriptions({ login: state, setLogin }: { login: Login; setLogin(f: (s: Login) => Login): void }) {
  const api = useApi()
  const active = useEngines((s) => s.active)
  const canList = useQuerySupport(['list_logins'])
  const canLogin = useQuerySupport(['login'])
  const logins = useQuery({ enabled: canList, queryKey: queryKeys.logins, queryFn: () => api.list_logins() })
  const { pending, failed } = state
  const [more, setMore] = useState(false)
  // A Cancel still in flight: Sign in waits for it, or the engine could end the new sign-in instead of the old one.
  const cancelling = useRef<Promise<unknown>>(Promise.resolve())
  if (!canLogin) return <Note>This engine cannot sign in to subscriptions. Use an API key or a local model instead.</Note>
  // An engine that lists its logins offers only those; one that does not gets the known ones.
  const known = logins.data ? new Set(logins.data.map((l) => l.name)) : null
  const offered = (subs: readonly Subscription[]) => subs.filter((s) => !known || known.has(s.ns))
  const card = (s: Subscription) => {
    const login = logins.data?.find((l) => l.name === s.ns)
    const waiting = pending?.ns === s.ns
    const error = failed?.ns === s.ns ? failed.message : null
    const signIn = async () => {
      setLogin(() => ({ pending: { ns: s.ns, url: null }, failed: null }))
      try {
        await cancelling.current
        const url = await api.login(s.ns)
        setLogin((l) => (l.pending?.ns === s.ns ? { ...l, pending: { ns: s.ns, url } } : l))
        if (isIos() && active === LOCAL) void openAuthBrowser(url, false)
      } catch (e) {
        setLogin((l) => (l.pending?.ns === s.ns ? { pending: null, failed: { ns: s.ns, message: errorText(e) } } : l))
      }
    }
    // The engine ends the sign-in and frees its callback port, so Sign in works again at once (an older engine
    // without `cancel_login` keeps the port until the sign-in times out).
    const cancel = () => {
      setLogin(() => ({ pending: null, failed: null }))
      cancelling.current = api.cancel_login(s.ns).catch(() => undefined)
    }
    return (
      <div key={s.ns} className={`${CARD} flex flex-col gap-3 p-4 hover:bg-layer-2`}>
        <div className="flex items-center gap-3">
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-[15px] font-medium text-foreground">{s.label}</span>
            <span className="text-[13px] text-fg-secondary">{login?.logged_in ? (login.account ? `Signed in as ${login.account}` : 'Signed in') : s.plan}</span>
          </div>
          {login?.logged_in ? (
            <span className="flex items-center gap-1.5 text-sm text-success">
              <Check className="size-4" aria-hidden /> Connected
            </span>
          ) : waiting ? (
            // The browser may be closed or the sign-in abandoned: the wait must always have a way out.
            <span className="flex items-center gap-2">
              <LoaderCircle className="size-4 animate-spin text-fg-secondary motion-reduce:animate-none" aria-hidden />
              <Button variant="ghost" onClick={cancel} aria-label={`Cancel sign-in: ${s.label}`}>
                Cancel
              </Button>
            </span>
          ) : (
            <Button variant={error ? 'outline' : 'default'} disabled={!!pending} onClick={() => void signIn()} aria-label={`${error ? 'Try again' : 'Sign in'}: ${s.label}`}>
              {error ? 'Try again' : 'Sign in'}
            </Button>
          )}
        </div>
        {waiting && (
          <p role="status" className="text-[13px] text-fg-secondary">
            Finish in your browser…
            {pending?.url && (
              <>
                {' '}
                Browser didn't open? <code className="break-all font-mono text-xs text-fg-tertiary select-all">{pending.url}</code>
              </>
            )}
          </p>
        )}
        {error && (
          <p role="alert" className="text-[13px] text-danger">
            Sign-in did not finish: {error}
          </p>
        )}
      </div>
    )
  }
  const extra = offered(MORE_SUBSCRIPTIONS)
  return (
    <div className="flex flex-col gap-3">
      {offered(SUBSCRIPTIONS).map(card)}
      {extra.length > 0 && (
        <>
          <button type="button" aria-expanded={more} onClick={() => setMore(!more)} className="inline-flex w-fit shrink-0 items-center gap-1 rounded-md py-1 text-sm whitespace-nowrap text-fg-secondary hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none">
            <ChevronDown className={`size-4 shrink-0 transition-transform motion-reduce:transition-none ${more ? 'rotate-180' : ''}`} aria-hidden />
            <span>More subscriptions</span>
          </button>
          {more && extra.map(card)}
        </>
      )}
    </div>
  )
}

/** Provider + masked key → `set_secret(<adapter>, key, note)`; the key is encrypted and never read back. */
function ApiKey({ onConnected }: { onConnected(provider: string): void }) {
  const api = useApi()
  const can = useQuerySupport(['set_secret'])
  const qc = useQueryClient()
  const [secret, setSecret] = useState(KEY_PROVIDERS[0]!.secret)
  const [value, setValue] = useState('')
  const [state, setState] = useState<{ busy: boolean; saved: string | null; error: string | null }>({ busy: false, saved: null, error: null })
  if (!can) return <Note>This engine does not take keys from the app. Add one in Settings › Models › Providers on the engine's machine.</Note>
  const p = KEY_PROVIDERS.find((k) => k.secret === secret) ?? KEY_PROVIDERS[0]!
  const save = async () => {
    setState({ busy: true, saved: null, error: null })
    try {
      await api.set_secret(p.secret, value.trim(), keyNote(p))
      void qc.invalidateQueries({ queryKey: queryKeys.secrets })
      setValue('')
      setState({ busy: false, saved: p.label, error: null })
      onConnected(p.provider)
    } catch (e) {
      setState({ busy: false, saved: null, error: errorText(e) })
    }
  }
  return (
    <form
      className={`${CARD} flex flex-col gap-4 p-5 hover:bg-layer-2`}
      onSubmit={(e) => {
        e.preventDefault()
        if (value.trim()) void save()
      }}
    >
      <div className="grid gap-3 sm:grid-cols-[11rem_1fr]">
        <Select value={secret} onValueChange={setSecret}>
          <SelectTrigger aria-label="Provider" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {KEY_PROVIDERS.map((k) => (
              <SelectItem key={k.secret} value={k.secret}>
                {k.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input type="password" autoComplete="off" spellCheck={false} aria-label={`${p.label} API key`} placeholder={p.placeholder} value={value} onChange={(e) => setValue(e.target.value)} />
      </div>
      <div className="flex items-center justify-between gap-3">
        <p className="text-[13px] text-fg-secondary" role={state.error ? 'alert' : 'status'}>
          {state.error ? <span className="text-danger">{state.error}</span> : state.saved ? `${state.saved} key saved. It is encrypted and never shown again.` : 'Stored encrypted on this computer, never shown again.'}
        </p>
        <Button type="submit" variant="outline" disabled={!value.trim() || state.busy}>
          {state.busy && <LoaderCircle className="animate-spin motion-reduce:animate-none" />} Save key
        </Button>
      </div>
    </form>
  )
}

/** `local_servers_probe()` on open; each reachable server is one click to add. */
function LocalModels({ onConnected }: { onConnected(provider: string): void }) {
  const api = useApi()
  const canProbe = useQuerySupport(['local_servers_probe'])
  const canAdd = useQuerySupport(['add_provider'])
  const canProviders = useQuerySupport(['list_providers'])
  const probe = useQuery({ enabled: canProbe, queryKey: queryKeys.localServers, queryFn: () => api.local_servers_probe(), retry: false })
  const providers = useQuery({ enabled: canProviders, queryKey: queryKeys.providers, queryFn: () => api.list_providers() })
  const [adding, setAdding] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  if (!canProbe || !canAdd) return <Note>This engine cannot look for local servers. Add one in Settings › Models › Providers.</Note>
  const names = providers.data?.map((p) => p.name) ?? []
  const found = probe.data?.filter((s) => s.reachable && !s.error) ?? []
  const add = async (s: LocalServer) => {
    setAdding(s.base_url)
    setError(null)
    try {
      const form = probeForm(s)
      const added = await api.add_provider(...localAddArgs({ ...form, name: freeName(form.name, names) }))
      void probe.refetch()
      onConnected(added.name)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setAdding(null)
    }
  }
  const again = (
    <Button variant="outline" size="sm" disabled={probe.isFetching} onClick={() => void probe.refetch()}>
      <Radar className={probe.isFetching ? 'animate-pulse motion-reduce:animate-none' : undefined} /> Look again
    </Button>
  )
  if (probe.isFetching && !probe.data) return <Note><LoaderCircle className="mr-2 inline size-4 animate-spin motion-reduce:animate-none" aria-hidden />Looking for model servers on this computer…</Note>
  if (found.length === 0)
    return (
      <div className={`${CARD} flex flex-col gap-3 p-5 hover:bg-layer-2`}>
        <p className="text-sm text-fg-secondary">
          {probe.isError ? `Could not look for servers: ${probe.error.message}. ` : 'No local model server is running. '}
          The easiest start is{' '}
          <button type="button" className="text-fg-accent underline underline-offset-4 hover:text-fg-accent-hover" onClick={() => openExternal('https://ollama.com')}>
            Ollama
          </button>
          : install it, pull a model, then look again.
        </p>
        <div>{again}</div>
      </div>
    )
  return (
    <div className="flex flex-col gap-3">
      {found.map((s) => (
        <div key={s.base_url} className={`${CARD} flex items-center gap-3 p-4 hover:bg-layer-2`}>
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-[15px] font-medium text-foreground">{runtimeLabel(s.runtime)}</span>
            <span className="truncate text-[13px] text-fg-secondary">
              {s.models} {s.models === 1 ? 'model' : 'models'} · <span className="font-mono">{s.base_url}</span>
            </span>
          </div>
          {s.added_as && names.includes(s.added_as) ? (
            <span className="flex items-center gap-1.5 text-sm text-success">
              <Check className="size-4" aria-hidden /> Added
            </span>
          ) : (
            <Button disabled={adding !== null} onClick={() => void add(s)} aria-label={`Add ${runtimeLabel(s.runtime)}`}>
              {adding === s.base_url && <LoaderCircle className="animate-spin motion-reduce:animate-none" />} Add
            </Button>
          )}
        </div>
      ))}
      {error && (
        <p role="alert" className="text-[13px] text-danger">
          {error}
        </p>
      )}
      <div>{again}</div>
    </div>
  )
}

function Note({ children }: { children: ReactNode }) {
  return <p className="rounded-xl border border-border-subtle bg-layer-2 p-5 text-sm text-fg-secondary">{children}</p>
}
