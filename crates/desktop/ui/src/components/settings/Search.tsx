import { useQuerySupport } from '@/rpc/capabilities'
// 16 §8: Settings › Search — the four external providers (a card each: enable, URL, key status with a masked
// "Set key" field that never shows the value, Test, order), the `web` read/budget defaults, and per-engine health
// from the last calls with a "Run a test search" field. Reads `search_settings_get`, saves with
// `search_settings_set` (persisted to config.yaml and applied live), probes with `search_provider_test`.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowDown, ArrowUp } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import {
  PROVIDER_INFO,
  type ProviderSettings,
  type SearchSettings,
  byPriority,
  isDirty,
  label,
  moveProvider,
  msOf,
  providerState,
  strip,
  updateProvider,
  validate,
} from '@/components/settings/search-settings'
import { Dot, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ProviderProbe } from "@/rpc/bindings/method.web.output"
import type { Recency } from "@/rpc/bindings/method.search_2dproviders.output"
import type { SearchTestResult } from "@/rpc/bindings/method.web.output"

const RECENCY: readonly (Recency | 'none')[] = ['none', 'day', 'week', 'month', 'year']

export function Search() {
  const canQuery1 = useQuerySupport(["search_settings_get"])
  const api = useApi()
  const qc = useQueryClient()
  const settings = useQuery({ enabled: canQuery1, queryKey: queryKeys.searchSettings, queryFn: () => api.search_settings_get() })
  // Local edits over the daemon's answer; `undefined` = showing exactly what the daemon returned.
  const [edited, setEdited] = useState<SearchSettings | undefined>()
  const draft = edited ?? settings.data
  const setDraft = (next: (d: SearchSettings) => SearchSettings) =>
    setEdited((e) => {
      const base = e ?? settings.data
      return base && next(base)
    })
  const save = useAction((s: SearchSettings) => api.search_settings_set(strip(s)), {
    keys: [queryKeys.searchSettings, queryKeys.searchStatus],
    onSuccess: () => {
      setEdited(undefined)
      toast.success('Search settings saved and applied')
    },
  })
  const dirty = isDirty(settings.data, draft)
  const problem = draft ? validate(draft) : null
  const patch = (name: string, p: Partial<ProviderSettings>) => setDraft((d) => ({ ...d, providers: updateProvider(d.providers, name, p) }))
  const defaults = (p: Partial<SearchSettings['defaults']>) => setDraft((d) => ({ ...d, defaults: { ...d.defaults, ...p } }))

  if (settings.isError) return <Empty>{errorText(settings.error)}</Empty>
  if (!draft) return <Empty>Loading…</Empty>

  return (
    <>
      <Section
        title="Providers"
        actions={
          <>
            {problem && <span className="text-[13px] text-destructive">{problem}</span>}
            <Button size="sm" variant="outline" disabled={!dirty || save.isPending} onClick={() => setEdited(undefined)}>
              Revert
            </Button>
            <Button size="sm" disabled={!dirty || !!problem || save.isPending} onClick={() => save.mutate(draft)}>
              Save
            </Button>
          </>
        }
      >
        <p className="mb-3 text-[13px] text-fg-secondary">
          External providers run alongside (or instead of, see Defaults) the keyless built-in engines. Order is the registration
          order. Changes apply on Save, without a restart.
        </p>
        <div className="flex flex-col gap-3">
          {byPriority(draft.providers).map((p, i, all) => (
            <ProviderCard
              key={p.name}
              provider={p}
              first={i === 0}
              last={i === all.length - 1}
              onPatch={(patch_) => patch(p.name, patch_)}
              onMove={(dir) => setDraft((d) => ({ ...d, providers: moveProvider(d.providers, p.name, dir) }))}
              onSecretSaved={() => void qc.invalidateQueries({ queryKey: queryKeys.searchSettings })}
            />
          ))}
        </div>
      </Section>
      <Section title="Defaults">
        <Row label="Pages read per search" description="Top hits search_web reads and excerpts when the call gives no read count (0..8).">
          <Input
            type="number"
            min={0}
            max={8}
            className="w-20"
            aria-label="pages read"
            value={draft.defaults.read_top}
            onChange={(e) => defaults({ read_top: Math.max(0, Math.min(8, Number(e.target.value) || 0)) })}
          />
        </Row>
        <Row label="Read budget (s)" description="Deadline for the whole read phase; pages still loading are reported as skipped: budget.">
          <Input
            type="number"
            step={0.5}
            min={0.5}
            className="w-20"
            aria-label="read budget seconds"
            defaultValue={draft.defaults.read_budget_ms / 1000}
            onBlur={(e) => defaults({ read_budget_ms: msOf(e.target.value, draft.defaults.read_budget_ms) })}
          />
        </Row>
        <Row label="Render cap (s)" description="Cap on one browser-rendered page inside a search; a stalled render must not eat the budget.">
          <Input
            type="number"
            step={0.5}
            min={0.5}
            className="w-20"
            aria-label="render cap seconds"
            defaultValue={draft.defaults.render_cap_ms / 1000}
            onBlur={(e) => defaults({ render_cap_ms: msOf(e.target.value, draft.defaults.render_cap_ms) })}
          />
        </Row>
        <Row label="Default recency" description="Applied when a call passes no recent= filter.">
          <Select
            value={draft.defaults.recent_default ?? 'none'}
            onValueChange={(v) => defaults({ recent_default: v === 'none' ? null : (v as Recency) })}
          >
            <SelectTrigger className="w-32" aria-label="default recency">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {RECENCY.map((r) => (
                <SelectItem key={r} value={r}>
                  {r === 'none' ? 'None' : r}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Keyless engines as fallback only"
          description="On: the built-in Google/Brave/Qwant/DuckDuckGo scrapers run only when no external provider serves the category. Off: they always run alongside."
        >
          <Switch
            checked={!draft.defaults.keyless_fallback}
            onCheckedChange={(on) => defaults({ keyless_fallback: !on })}
          />
        </Row>
        <Row
          label="Treat degraded as error"
          description="A search where no external provider answered and at most one general engine returned rows fails instead of returning a warning. Routines then retry or report."
        >
          <Switch
            checked={draft.defaults.degraded_is_error}
            onCheckedChange={(on) => defaults({ degraded_is_error: on })}
          />
        </Row>
      </Section>
      <Status />
    </>
  )
}

function ProviderCard({
  provider: p,
  first,
  last,
  onPatch,
  onMove,
  onSecretSaved,
}: {
  provider: ProviderSettings
  first: boolean
  last: boolean
  onPatch: (patch: Partial<ProviderSettings>) => void
  onMove: (dir: -1 | 1) => void
  onSecretSaved: () => void
}) {
  const api = useApi()
  const [probe, setProbe] = useState<ProviderProbe | { error: string } | undefined>()
  const test = useAction((name: string) => api.search_provider_test(name, 'rust async runtime'), {
    keys: [queryKeys.searchStatus],
    onSuccess: (r) => setProbe(r),
  })
  const info = PROVIDER_INFO[p.name]
  return (
    <div className="rounded-lg border border-border-subtle p-4" data-provider={p.name}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Dot on={p.registered} />
            {label(p.name)}
          </div>
          {info && <div className="mt-0.5 text-[13px] text-fg-secondary">{info.blurb}</div>}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button size="icon" variant="ghost" aria-label={`move ${label(p.name)} up`} disabled={first} onClick={() => onMove(-1)}>
            <ArrowUp className="size-4" />
          </Button>
          <Button size="icon" variant="ghost" aria-label={`move ${label(p.name)} down`} disabled={last} onClick={() => onMove(1)}>
            <ArrowDown className="size-4" />
          </Button>
          <Switch checked={p.enabled} aria-label={`enable ${label(p.name)}`} onCheckedChange={(on) => onPatch({ enabled: on })} />
        </div>
      </div>
      <div className="mt-3 grid gap-2 sm:grid-cols-[1fr_auto]">
        {p.base_url !== null && (
          <Input
            aria-label={`${label(p.name)} URL`}
            placeholder={p.name === 'searxng' ? 'http://127.0.0.1:8080' : 'Server URL'}
            value={p.base_url}
            onChange={(e) => onPatch({ base_url: e.target.value })}
          />
        )}
        <div className="flex items-center gap-2">
          <span className="text-[13px] text-fg-secondary">Timeout</span>
          <Input
            type="number"
            step={0.5}
            min={0.5}
            className="w-20"
            aria-label={`${label(p.name)} timeout seconds`}
            defaultValue={p.timeout_ms / 1000}
            onBlur={(e) => onPatch({ timeout_ms: msOf(e.target.value, p.timeout_ms) })}
          />
          <span className="text-[13px] text-fg-secondary">s</span>
        </div>
        {p.engines && (
          <Input
            className="sm:col-span-2"
            aria-label="OpenSERP engines"
            placeholder="google, bing, duckduckgo"
            value={p.engines.join(', ')}
            onChange={(e) => onPatch({ engines: e.target.value.split(',').map((s) => s.trim()).filter(Boolean) })}
          />
        )}
      </div>
      {p.secret !== null && <SecretRow provider={p} onPatch={onPatch} onSaved={onSecretSaved} />}
      <div className="mt-3 flex flex-wrap items-center justify-between gap-2 text-[13px] text-fg-secondary">
        <span>{providerState(p)}</span>
        <div className="flex items-center gap-2">
          {probe && ('error' in probe ? <span className="text-destructive">{probe.error}</span> : <ProbeLine probe={probe} />)}
          <Button
            size="sm"
            variant="outline"
            disabled={!p.registered || test.isPending}
            onClick={() => {
              setProbe(undefined)
              test.mutate(p.name, { onError: (e) => setProbe({ error: errorText(e) }) })
            }}
          >
            {test.isPending ? 'Testing…' : 'Test'}
          </Button>
        </div>
      </div>
    </div>
  )
}

function ProbeLine({ probe }: { probe: ProviderProbe }) {
  return probe.ok ? (
    <span>
      ok · {probe.took_ms} ms · {probe.rows} rows{probe.detail ? ` · ${probe.detail}` : ''}
    </span>
  ) : (
    <span className="text-destructive">
      failed · {probe.took_ms} ms · {probe.detail}
    </span>
  )
}

/** Key status plus a masked entry that stores through `set_secret`; the value is never read back. */
function SecretRow({ provider: p, onPatch, onSaved }: { provider: ProviderSettings; onPatch: (patch: Partial<ProviderSettings>) => void; onSaved: () => void }) {
  const api = useApi()
  const [open, setOpen] = useState(false)
  const [value, setValue] = useState('')
  const name = p.secret ?? ''
  const store = useAction((v: string) => api.set_secret(name, v, `${label(p.name)} key (Settings > Search)`), {
    keys: [queryKeys.secrets],
    onSuccess: () => {
      setValue('')
      setOpen(false)
      toast.success(`Stored ${name}; the value is encrypted and never shown`)
      onSaved()
    },
  })
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 text-[13px]">
      <span className="text-fg-secondary">Key</span>
      <Input className="w-36" aria-label={`${label(p.name)} secret name`} value={name} onChange={(e) => onPatch({ secret: e.target.value })} />
      <span className={p.secret_set ? 'text-success' : 'text-fg-tertiary'}>{p.secret_set ? 'set' : 'not set'}</span>
      {open ? (
        <form
          className="flex flex-1 items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (value.trim()) store.mutate(value)
          }}
        >
          <Input
            type="password"
            autoComplete="off"
            autoFocus
            className="min-w-40 flex-1"
            aria-label={`value of ${name}`}
            placeholder="Paste the key"
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
          <Button type="submit" size="sm" disabled={!value.trim() || store.isPending}>
            Store
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
        </form>
      ) : (
        <Button size="sm" variant="outline" disabled={!name.trim()} onClick={() => setOpen(true)}>
          {p.secret_set ? 'Replace key…' : 'Set key…'}
        </Button>
      )}
    </div>
  )
}

/** Per-engine health from the last calls plus one full test search. */
function Status() {
  const canQuery2 = useQuerySupport(["search_status"])
  const api = useApi()
  const status = useQuery({ enabled: canQuery2, queryKey: queryKeys.searchStatus, queryFn: () => api.search_status(), refetchInterval: 15_000 })
  const [query, setQuery] = useState('')
  const [result, setResult] = useState<SearchTestResult | undefined>()
  const run = useAction((q: string) => api.search_test(q, null, null), { keys: [queryKeys.searchStatus], onSuccess: (r) => setResult(r) })
  return (
    <Section title="Status">
      {status.isError && <Empty>{errorText(status.error)}</Empty>}
      {status.data?.length === 0 && <Empty>No search providers registered.</Empty>}
      {status.data
        ?.filter((s) => s.searches)
        .map((s) => (
          <Row
            key={s.name}
            label={
              <span className="flex items-center gap-2">
                <Dot on={s.ok === true} />
                {s.name}
                {s.external && <span className="text-[11px] text-fg-tertiary">external</span>}
              </span>
            }
            description={
              s.ok === null
                ? 'Not called yet'
                : s.ok
                  ? `ok · ${s.detail} · ${when(s.last_ok_at)}`
                  : `${s.kind} · ${s.detail} · ${when(s.last_error_at)}`
            }
          />
        ))}
      <form
        className="mt-4 flex flex-wrap items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (query.trim()) run.mutate(query.trim())
        }}
      >
        <Input className="min-w-48 flex-1" aria-label="test search query" placeholder="Run a test search…" value={query} onChange={(e) => setQuery(e.target.value)} />
        <Button type="submit" size="sm" variant="outline" disabled={!query.trim() || run.isPending}>
          {run.isPending ? 'Searching…' : 'Search'}
        </Button>
      </form>
      {result && (
        <pre className={`mt-3 max-h-80 overflow-auto whitespace-pre-wrap rounded-md border border-border-subtle p-3 text-xs ${result.is_error ? 'text-destructive' : ''}`}>
          {result.text}
        </pre>
      )}
    </Section>
  )
}

function when(iso: string | null): string {
  if (!iso) return ''
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleTimeString()
}
