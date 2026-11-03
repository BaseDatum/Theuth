// 126 §10: Settings › Double — the owner's double in one pane: status header, the Improvement chart (design 123's
// `LineChart` kernel), KPI tiles, the classes table with its row actions, training controls, consent, export / delete,
// and the training log. Every number comes from `double_status` / `double_classes` / `double_class`; every control is
// one `double_*` call. Before W1/W2 land the read stubs answer empty values and the mutating ones reject with
// "not available yet" — the page renders the empty state and toasts the rejection.
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { Fragment, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { LineChartView } from '@/components/chat/surface/diagrams/LineChart'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { useMesh } from '@/components/tasks/queries'
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
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { humanSecs } from '@/lib/injected'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import type { Api } from '@/rpc/api'
import type { DoubleClass } from '@/rpc/bindings/DoubleClass'
import type { DoubleClassDetail } from '@/rpc/bindings/DoubleClassDetail'
import type { DoubleConfig } from '@/rpc/bindings/DoubleConfig'
import type { DoubleRun } from '@/rpc/bindings/DoubleRun'
import type { DoubleStatus } from '@/rpc/bindings/DoubleStatus'
import type { DoubleTrainingState } from '@/rpc/bindings/DoubleTrainingState'
import type { Venue } from '@/rpc/bindings/Venue'
import { on } from '@/store/sessions'
import { rungGlyph } from '@/components/chat/DoubleChip'

const pct = (x: number): string => `${Math.round(x * 100)} %`
const at = (iso: string | null | undefined): string => (iso ? formatAt(Date.parse(iso)) : '—')

/**
 * The overall curve out of the per-class ones (`double_class(key).curve`; `double_status` carries no curve): per week,
 * accuracy and coverage weighted by each class's `n`, current and baseline. Weeks sorted ascending.
 */
export type WeekPoint = { week: string; acc: number; accBase: number; cov: number; covBase: number }

export function mergeCurves(details: readonly DoubleClassDetail[]): WeekPoint[] {
  const weeks = new Map<string, { n: number; nb: number; acc: number; accBase: number; cov: number; covBase: number }>()
  for (const d of details) {
    for (const pt of d.curve) {
      const w = weeks.get(pt.week) ?? { n: 0, nb: 0, acc: 0, accBase: 0, cov: 0, covBase: 0 }
      w.n += pt.current.n
      w.nb += pt.baseline.n
      w.acc += pt.current.accuracy * pt.current.n
      w.cov += pt.current.coverage_2pct * pt.current.n
      w.accBase += pt.baseline.accuracy * pt.baseline.n
      w.covBase += pt.baseline.coverage_2pct * pt.baseline.n
      weeks.set(pt.week, w)
    }
  }
  return [...weeks.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([week, w]) => ({
      week,
      acc: w.n ? w.acc / w.n : 0,
      cov: w.n ? w.cov / w.n : 0,
      accBase: w.nb ? w.accBase / w.nb : 0,
      covBase: w.nb ? w.covBase / w.nb : 0,
    }))
}

/** The `LineChart` props of the Improvement chart: one label per week (`DD Mon`), four series in percent. */
export function chartProps(points: readonly WeekPoint[]) {
  const label = (iso: string) => {
    const d = new Date(iso)
    // `week` is a bare RFC 3339 date (the Monday): UTC getters, so the label never slips a day west of Greenwich.
    return Number.isNaN(d.getTime()) ? iso : `${d.getUTCDate()} ${['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][d.getUTCMonth()]}`
  }
  const r = (x: number) => Math.round(x * 1000) / 10
  return {
    labels: points.map((p) => label(p.week)),
    series: [
      { name: 'accuracy', values: points.map((p) => r(p.acc)) },
      { name: 'accuracy · baseline', values: points.map((p) => r(p.accBase)), state: 'muted' as const },
      { name: 'coverage @ 2 %', values: points.map((p) => r(p.cov)) },
      { name: 'coverage · baseline', values: points.map((p) => r(p.covBase)), state: 'muted' as const },
    ],
    unit: '%',
    yLabel: 'weekly, prequential',
  }
}

/** `tier · venue · when · duration · result` of one `runs` row. */
export function runLine(r: DoubleRun): string {
  const dur = r.ended_at ? humanSecs(Math.max(0, Math.round((Date.parse(r.ended_at) - Date.parse(r.started_at)) / 1000))) : r.state
  return [`tier ${r.tier.toUpperCase()}`, r.venue, at(r.started_at), dur, r.result ?? r.state].join(' · ')
}

// Row actions, as plain functions: the table calls them and the test checks they hit the right method.
export const setCap = (api: Pick<Api, 'double_set_cap'>, c: DoubleClass, rung: number) => api.double_set_cap(c.key, rung, c.mutating_opt_in)
export const setOptIn = (api: Pick<Api, 'double_set_cap'>, c: DoubleClass, on: boolean) => api.double_set_cap(c.key, c.cap, on)
export const forgetClass = (api: Pick<Api, 'double_forget'>, key: string) => api.double_forget({ target: 'class', key })

export function Double() {
  const api = useApi()
  const qc = useQueryClient()
  const status = useQuery({ queryKey: queryKeys.doubleStatus, queryFn: () => api.double_status(), refetchInterval: 30_000 })
  const classes = useQuery({ queryKey: queryKeys.doubleClasses, queryFn: () => api.double_classes() })
  const settings = useQuery({ queryKey: queryKeys.doubleSettings, queryFn: () => api.double_settings_get() })
  // Live: a run's state or a version switch refreshes everything; the run states also feed the log below.
  const [live, setLive] = useState<DoubleTrainingState[]>([])
  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: queryKeys.doubleAll })
    const offs = [
      on('DoubleTrainingState', (e) => {
        setLive((l) => [e, ...l.filter((x) => x.run_id !== e.run_id)].slice(0, 20))
        invalidate()
      }),
      on('DoubleVersionActivated', (e) => {
        toast.message(`double: version ${e.version} is active${e.previous ? ` (was ${e.previous})` : ''}`)
        invalidate()
      }),
    ]
    return () => offs.forEach((off) => off())
  }, [qc])
  const s = status.data
  return (
    <>
      <Section title="Status">
        {status.isError ? (
          <Empty>{errorText(status.error)}</Empty>
        ) : (
          <Status s={s} />
        )}
      </Section>
      <Section title="Improvement">
        <p className="mb-2 text-[13px] text-fg-secondary">
          Weekly prequential accuracy and coverage at ≤ 2 % error, the active version against the frozen zero-shot baseline on the same decisions. Every prediction is made before you answer, so the curve is an honest out-of-sample test.
        </p>
        <Improvement classes={classes.data ?? []} />
        <Kpis s={s} />
      </Section>
      <Section title="Classes">
        <Classes classes={classes.data ?? []} loading={classes.isPending} error={classes.isError ? errorText(classes.error) : undefined} />
      </Section>
      <Section title="Training">
        <Training s={s} settings={settings.data} />
      </Section>
      <Section title="Training log">
        <RunLog last={s?.last_run} live={live} />
      </Section>
    </>
  )
}

function Status({ s }: { s: DoubleStatus | undefined }) {
  const api = useApi()
  const pause = useAction((on: boolean) => api.double_pause(on), { keys: [queryKeys.doubleStatus] })
  if (!s) return <Empty>loading…</Empty>
  return (
    <>
      <Row label="Enabled" description={s.enabled ? 'Predicting every captured card; learning from your answers.' : '`double.enabled` is off: nothing is predicted or learned. Turn it on in config.yaml and restart.'}>
        <span className={`inline-block size-2 shrink-0 rounded-full ${s.enabled ? 'bg-success' : 'bg-fg-tertiary/40'}`} />
      </Row>
      <Row label="Pause learning" description="Predictions and rungs continue; labels stop accruing (the curve shows the gap).">
        <Switch checked={s.paused} disabled={pause.isPending || !s.enabled} onCheckedChange={(v) => pause.mutate(v)} aria-label="Pause learning" />
      </Row>
      <Row label="Active version" description={s.active_version ? `Every prediction names it; Roll back below switches it.` : 'Baseline only until the first Tier A run.'}>
        <span className="font-mono text-sm">{s.active_version ?? 'baseline'}</span>
      </Row>
      <Row label="Last run" description={s.last_run ? runLine(s.last_run) : 'No run yet.'} />
      <Row label="Next window" description={s.next_window ? `Tier B may start at ${at(s.next_window)} (${s.venue}).` : `No Tier B run scheduled (venue ${s.venue}).`} />
      {s.approvals_off_sessions > 0 && <Row label="Approvals off" description={`${s.approvals_off_sessions} session(s) run without approvals: the double learns only from asks there (§6.6).`} />}
    </>
  )
}

function Improvement({ classes }: { classes: DoubleClass[] }) {
  const api = useApi()
  // ponytail: one `double_class` per class to build the overall curve (no overall-curve RPC in the contract); fine for tens of classes.
  const details = useQueries({ queries: classes.map((c) => ({ queryKey: queryKeys.doubleClass(c.key), queryFn: () => api.double_class(c.key), staleTime: 60_000 })) })
  const points = mergeCurves(details.flatMap((d) => (d.data ? [d.data] : [])))
  if (points.length === 0) return <Empty>No labelled weeks yet — the chart draws once the double has predicted and you have answered.</Empty>
  return (
    <div className="overflow-x-auto rounded-md border border-border-subtle p-2">
      <LineChartView props={chartProps(points)} />
    </div>
  )
}

function Kpis({ s }: { s: DoubleStatus | undefined }) {
  const k = s?.kpis
  const tiles: [string, string, string][] = [
    ['Asks avoided', k ? String(k.asks_avoided) : '—', 'L2/L3 answers, last 30 days'],
    ['Minutes saved', k ? String(Math.round(k.minutes_saved)) : '—', 'your median answer time + 30 s per interruption'],
    ['Overrides', k ? `${k.overrides_per_100.toFixed(1)} / 100` : '—', `undo ${k ? k.undo_rate_per_100.toFixed(1) : '—'} / 100 · over ${k?.n ?? 0} decisions`],
  ]
  return (
    <div className="mt-3 grid grid-cols-1 gap-2 sm:grid-cols-3">
      {tiles.map(([label, value, note]) => (
        <div key={label} className="rounded-md border border-border-subtle px-3 py-2">
          <div className="text-xs text-fg-secondary">{label}</div>
          <div className="text-xl font-semibold tabular-nums">{value}</div>
          <div className="text-[11px] text-fg-tertiary">{note}</div>
        </div>
      ))}
    </div>
  )
}

function Classes({ classes, loading, error }: { classes: DoubleClass[]; loading: boolean; error: string | undefined }) {
  const api = useApi()
  const keys = [queryKeys.doubleClasses, queryKeys.doubleStatus]
  const cap = useAction(({ c, rung }: { c: DoubleClass; rung: number }) => setCap(api, c, rung), { keys })
  const optIn = useAction(({ c, on }: { c: DoubleClass; on: boolean }) => setOptIn(api, c, on), { keys })
  const forget = useAction((key: string) => forgetClass(api, key), { keys, onSuccess: (n) => toast.message(`forgot ${n} example(s)`) })
  const [open, setOpen] = useState<string | null>(null)
  const busy = cap.isPending || optIn.isPending || forget.isPending
  if (error) return <Empty>{error}</Empty>
  if (loading) return <Empty>loading…</Empty>
  if (classes.length === 0) return <Empty>No classes yet. The first approval or question the double sees creates one.</Empty>
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-xs">
        <thead className="text-fg-tertiary">
          <tr>
            {['class', 'n', 'rung', 'acc', 'ECE', 'cap', 'mutating opt-in', 'last decision', ''].map((h) => (
              <th key={h} className="py-1 pr-3 font-normal">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {classes.map((c) => (
            <Fragment key={c.key}>
              <tr className="border-t border-border-subtle align-middle">
                <td className="max-w-[260px] truncate py-1.5 pr-3 font-mono" title={c.key}>
                  {c.key}
                </td>
                <td className="py-1.5 pr-3 tabular-nums">{c.n}</td>
                <td className="py-1.5 pr-3 tabular-nums" title={c.cap_reason ?? undefined}>
                  {rungGlyph(c.rung)} L{c.rung}
                </td>
                <td className="py-1.5 pr-3 tabular-nums">{pct(c.kpis.accuracy)}</td>
                <td className="py-1.5 pr-3 tabular-nums">{c.kpis.ece.toFixed(3)}</td>
                <td className="py-1.5 pr-3">
                  <Select value={String(c.cap)} disabled={busy} onValueChange={(v) => cap.mutate({ c, rung: Number(v) })}>
                    <SelectTrigger className="h-7 w-20 text-xs" aria-label={`cap of ${c.key}`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {[0, 1, 2, 3].map((r) => (
                        <SelectItem key={r} value={String(r)}>
                          L{r}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </td>
                <td className="py-1.5 pr-3">
                  <Switch checked={c.mutating_opt_in} disabled={busy} onCheckedChange={(v) => optIn.mutate({ c, on: v })} aria-label={`mutating opt-in of ${c.key}`} />
                </td>
                <td className="py-1.5 pr-3 tabular-nums">{at(c.last_decision_at)}</td>
                <td className="py-1.5 whitespace-nowrap">
                  <Button size="xs" variant="ghost" onClick={() => setOpen(open === c.key ? null : c.key)}>
                    {open === c.key ? 'hide' : 'examples'}
                  </Button>
                  <AlertDialog>
                    <AlertDialogTrigger asChild>
                      <Button size="xs" variant="ghost" disabled={busy}>
                        forget
                      </Button>
                    </AlertDialogTrigger>
                    <AlertDialogContent>
                      <AlertDialogHeader>
                        <AlertDialogTitle>Forget this class?</AlertDialogTitle>
                        <AlertDialogDescription>
                          Every example of <span className="font-mono">{c.key}</span> stops counting (kept as excluded, weight 0); the class drops back to L0.
                        </AlertDialogDescription>
                      </AlertDialogHeader>
                      <AlertDialogFooter>
                        <AlertDialogCancel>Cancel</AlertDialogCancel>
                        <AlertDialogAction onClick={() => forget.mutate(c.key)}>Forget</AlertDialogAction>
                      </AlertDialogFooter>
                    </AlertDialogContent>
                  </AlertDialog>
                </td>
              </tr>
              {open === c.key && (
                <tr className="border-t border-border-subtle">
                  <td colSpan={9} className="py-2">
                    <Examples k={c.key} />
                  </td>
                </tr>
              )}
            </Fragment>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function Examples({ k }: { k: string }) {
  const api = useApi()
  const d = useQuery({ queryKey: queryKeys.doubleClass(k), queryFn: () => api.double_class(k) })
  const forget = useAction((id: string) => api.double_forget({ target: 'examples', ids: [id] }), { keys: [queryKeys.doubleClass(k), queryKeys.doubleClasses] })
  if (d.isError) return <Empty>{errorText(d.error)}</Empty>
  if (!d.data) return <Empty>loading…</Empty>
  return (
    <div className="flex flex-col gap-1">
      <p className="text-fg-secondary">{d.data.promote_hint}</p>
      {d.data.examples.length === 0 && <Empty>No examples kept.</Empty>}
      {d.data.examples.map((e) => (
        <div key={e.id} className="flex items-start gap-2 border-t border-border-subtle py-1">
          <span className="w-20 shrink-0 tabular-nums text-fg-tertiary">{at(e.asked_at)}</span>
          <span className="min-w-0 flex-1 truncate" title={e.card_excerpt}>
            {e.card_excerpt}
          </span>
          <span className="shrink-0 text-fg-secondary">
            chose {e.chose ?? '—'} · predicted {e.predicted ?? '—'} · {e.outcome}
          </span>
          <Button size="xs" variant="ghost" disabled={forget.isPending} onClick={() => forget.mutate(e.id)}>
            forget
          </Button>
        </div>
      ))}
    </div>
  )
}

function Training({ s, settings }: { s: DoubleStatus | undefined; settings: DoubleConfig | undefined }) {
  const api = useApi()
  const peers = useMesh().data?.peers.filter((p) => p.reachable) ?? []
  const [node, setNode] = useState('')
  const [version, setVersion] = useState('')
  const [draft, setDraft] = useState<{ window: string; venue: string } | null>(null)
  const keys = [queryKeys.doubleStatus, queryKeys.doubleSettings]
  const train = useAction(({ tier, venue }: { tier: 'a' | 'b' | 'c'; venue: Venue }) => api.double_train(tier, venue), { keys, onSuccess: (id) => toast.message(`training run ${id} started`) })
  const rollback = useAction((v: string) => api.double_rollback(v), { keys, onSuccess: () => setVersion('') })
  const save = useAction((next: DoubleConfig) => api.double_settings_set(next), { keys, onSuccess: () => setDraft(null) })
  const exportData = useAction(() => api.double_export(), { onSuccess: (r) => toast.message(`exported ${r.examples} examples`, { description: r.path }) })
  const wipe = useAction(() => api.double_delete_all(), { keys: [queryKeys.doubleAll], onSuccess: () => toast.message('double data deleted') })
  const busy = train.isPending || rollback.isPending || save.isPending || exportData.isPending || wipe.isPending
  const off = !s?.enabled
  const win = draft?.window ?? settings?.training.window ?? ''
  const venue = draft?.venue ?? settings?.training.venue ?? ''
  const patch = (f: (c: DoubleConfig) => DoubleConfig) => settings && save.mutate(f(structuredClone(settings)))
  const tierC = settings?.training.tiers.c.enabled === true
  return (
    <>
      <Row wide label="Train now" description="Tier A: temperatures, k-NN memory and heads, in process, seconds. Tier B: the LoRA adapter on the whole corpus, gate-promoted, minutes to an hour.">
        <div className="flex flex-wrap items-center gap-1.5">
          <Button size="sm" variant="outline" disabled={busy || off} onClick={() => train.mutate({ tier: 'a', venue: { venue: 'local' } })}>
            Tier A
          </Button>
          <Button size="sm" variant="outline" disabled={busy || off} onClick={() => train.mutate({ tier: 'b', venue: { venue: 'local' } })}>
            Tier B local
          </Button>
          <Select value={node} disabled={busy || off || peers.length === 0} onValueChange={setNode}>
            <SelectTrigger className="h-8 w-36 text-xs" aria-label="Tier B node">
              <SelectValue placeholder={peers.length ? 'node…' : 'no reachable node'} />
            </SelectTrigger>
            <SelectContent>
              {peers.map((p) => (
                <SelectItem key={p.id} value={p.name}>
                  {p.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button size="sm" variant="outline" disabled={busy || off || !node} onClick={() => train.mutate({ tier: 'b', venue: { venue: 'node', name: node } })}>
            Tier B on node
          </Button>
          <Button size="sm" variant="outline" disabled title={tierC ? 'Tier C (full fine-tune) arrives with wave W3.' : 'Tier C is off (`double.training.tiers.c.enabled`) and arrives with wave W3.'}>
            Rebuild (tier C)
          </Button>
        </div>
      </Row>
      <Row label="Roll back" description="Make an earlier version active; every prediction row names its version, so the curve shows the switch as a step.">
        <Input value={version} onChange={(e) => setVersion(e.target.value)} placeholder="version id" className="h-8 w-40 font-mono md:text-xs" aria-label="version to roll back to" />
        <Button size="sm" variant="outline" disabled={busy || off || !version.trim()} onClick={() => rollback.mutate(version.trim())}>
          Roll back
        </Button>
      </Row>
      <Row label="Window and venue" description="Local hours Tier B may run (HH:MM-HH:MM) and where: auto · local · node:<name> · off. Applied live.">
        <Input value={win} onChange={(e) => setDraft({ window: e.target.value, venue })} placeholder="02:00-06:00" className="h-8 w-32 font-mono md:text-xs" aria-label="training window" disabled={!settings} />
        <Input value={venue} onChange={(e) => setDraft({ window: win, venue: e.target.value })} placeholder="auto" className="h-8 w-32 font-mono md:text-xs" aria-label="training venue" disabled={!settings} />
        <Button size="sm" variant="outline" disabled={busy || !draft || !settings} onClick={() => patch((c) => ({ ...c, training: { ...c.training, window: win.trim(), venue: venue.trim() } }))}>
          Save
        </Button>
      </Row>
      <Row wide label="Capture" description="Which cards the double predicts and learns from. Secret requests and hand-offs are never captured.">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {(['approvals', 'asks', 'attention', 'packets', 'drafts'] as const).map((k) => (
            <label key={k} className="flex items-center gap-1.5 text-xs">
              <Switch checked={settings?.capture[k] ?? false} disabled={!settings || busy} onCheckedChange={(v) => patch((c) => ({ ...c, capture: { ...c.capture, [k]: v } }))} aria-label={`capture ${k}`} />
              {k}
            </label>
          ))}
        </div>
      </Row>
      <Row label="Share dataset with mesh nodes" description="Ship scrubbed training bundles to your own GPU nodes for Tier B (never a sandbox VM; nothing goes to theuth).">
        <Switch checked={settings?.training.share_data ?? false} disabled={!settings || busy} onCheckedChange={(v) => patch((c) => ({ ...c, training: { ...c.training, share_data: v } }))} aria-label="share dataset with mesh nodes" />
      </Row>
      <Row label="Export dataset" description="A scrubbed JSONL + embeddings archive under the data directory's double/export/.">
        <Button size="sm" variant="outline" disabled={busy || off} onClick={() => exportData.mutate(undefined)}>
          Export
        </Button>
      </Row>
      <Row label="Delete all double data" description="Store, models and runs removed; the config stays. The double starts cold.">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button size="sm" variant="destructive" disabled={busy}>
              Delete all
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Delete all double data?</AlertDialogTitle>
              <AlertDialogDescription>Every example, label, prediction, version and run is removed. There is no undo; export first if you want to keep it.</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={() => wipe.mutate(undefined)}>Delete all</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </Row>
    </>
  )
}

function RunLog({ last, live }: { last: DoubleRun | null | undefined; live: DoubleTrainingState[] }) {
  // `double_status` carries the most recent `runs` row; the live `DoubleTrainingState` events of this session fill in the rest.
  if (!last && live.length === 0) return <Empty>No training runs yet.</Empty>
  return (
    <table className="w-full text-left text-xs">
      <thead className="text-fg-tertiary">
        <tr>
          {['run', 'tier', 'venue', 'state', 'detail'].map((h) => (
            <th key={h} className="py-1 pr-3 font-normal">
              {h}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {live.map((e) => (
          <tr key={e.run_id} className="border-t border-border-subtle">
            <td className="py-1 pr-3 font-mono">{e.run_id.slice(0, 8)}</td>
            <td className="py-1 pr-3">{e.tier.toUpperCase()}</td>
            <td className="py-1 pr-3">{e.venue}</td>
            <td className="py-1 pr-3">
              {e.state}
              {e.progress !== undefined && e.progress !== null ? ` · ${pct(e.progress)}` : ''}
            </td>
            <td className="py-1 text-fg-secondary">{e.message}</td>
          </tr>
        ))}
        {last && !live.some((e) => e.run_id === last.id) && (
          <tr className="border-t border-border-subtle">
            <td className="py-1 pr-3 font-mono">{last.id.slice(0, 8)}</td>
            <td className="py-1 pr-3">{last.tier.toUpperCase()}</td>
            <td className="py-1 pr-3">{last.venue}</td>
            <td className="py-1 pr-3">{last.state}</td>
            <td className="py-1 text-fg-secondary">{runLine(last)}</td>
          </tr>
        )}
      </tbody>
    </table>
  )
}
