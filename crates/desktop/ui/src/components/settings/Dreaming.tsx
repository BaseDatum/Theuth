import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Moon, Undo2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { useModels } from '@/components/agents/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { KEY, useDreamNights, useDreamProposals, useDreamSettings, useDreamStatus, useDreamingApi } from '@/components/dreaming/queries'
import { relative } from '@/components/routines/helpers'
import { KEY as SCHEDULES } from '@/components/routines/queries'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
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
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import {
  BACKLOG_SENTENCE,
  DAYS,
  type Day,
  PAUSE_DAYS,
  RUN_MODES,
  RUN_NOW_BUDGET_USD,
  type SettingsChange,
  TIERS,
  type Tier,
  TIER_LABEL,
  USD_SYMBOL,
  countsLine,
  isPaused,
  nightText,
  parseDigest,
  reduceSettings,
  resolvedDefault,
  undoLine,
  usd,
} from '@/lib/dreaming'
import { queryKeys } from '@/lib/rpc'
import type { DreamSettings } from '@/rpc/bindings/DreamSettings'
import type { NightSummary } from '@/rpc/bindings/NightSummary'
import type { Proposal } from '@/rpc/bindings/Proposal'
import type { RunMode } from '@/rpc/bindings/RunMode'
import { useConnection } from '@/store/connection'

/** Settings › Dreaming (136 §8, §14.1): the nightly routine's status, switch, window, budget, models, digest, pause, Dream now, nights and proposals. */
export function Dreaming() {
  const api = useDreamingApi()
  const info = useConnection((s) => s.info)
  const status = useDreamStatus()
  const settings = useDreamSettings()
  const nights = useDreamNights()
  const proposals = useDreamProposals()
  // The Enabled switch is the routine's own toggle (§8): read from the schedule table, written with `enable_schedule`.
  const schedules = useQuery({ queryKey: SCHEDULES, queryFn: () => api.list_schedules() })
  const routine = schedules.data?.find((s) => s.name === 'dreaming')
  const enable = useAction((enabled: boolean) => api.enable_schedule('dreaming', enabled), { keys: [SCHEDULES, KEY] })
  const save = useAction((next: DreamSettings) => api['dreaming.set_settings'](next), { keys: [KEY] })
  const change = (c: SettingsChange) => {
    const next = settings.data && reduceSettings(settings.data, c)
    if (next) save.mutate(next)
  }
  const busy = !settings.data || save.isPending

  const models = useModels()
  const titleModel = useQuery({ queryKey: queryKeys.titleModel, queryFn: () => api.title_model() })
  const subagentModel = useQuery({ queryKey: queryKeys.subagentModel, queryFn: () => api.subagent_model() })
  const defaults = { title: titleModel.data?.id ?? null, subagent: subagentModel.data?.id ?? null, session: info?.default_model ?? 'the default model' }

  const s = status.data
  const digest = settings.data ? parseDigest(settings.data.digest) : null
  const now = new Date()
  const paused = settings.data ? isPaused(settings.data.paused_until, now) : false

  return (
    <>
      <Section title="Dreaming" actions={<DreamNow disabled={s?.enabled === false} />}>
        {status.isError ? (
          <Empty>{status.error.message}</Empty>
        ) : (
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3 rounded-[10px] border border-border-subtle p-4 text-sm md:grid-cols-3">
            <Stat label="Last night" className="col-span-2 md:col-span-3">
              {s ? nightText(s.last_night) : '…'}
            </Stat>
            <Stat label="Next window">
              {!s ? '…' : s.paused_until && isPaused(s.paused_until, now) ? `paused until ${new Date(s.paused_until).toLocaleDateString()}` : s.next_window ? <span title={new Date(s.next_window).toLocaleString()}>{relative(s.next_window)}</span> : s.enabled ? 'not scheduled' : 'off'}
            </Stat>
            <Stat label="Budget left tonight">{s ? `${usd(s.budget_left_tonight_usd)} of ${usd(s.budget_usd)}` : '…'}</Stat>
            <Stat label="Proposals pending">{s ? String(s.proposals_pending) : '…'}</Stat>
            <Stat label="Backlog" className="col-span-2 md:col-span-3">
              {s ? `${s.backlog.fresh} fresh · ${s.backlog.untidied} untidied · ${s.backlog.open_cases} open ${Number(s.backlog.open_cases) === 1 ? 'case' : 'cases'}` : '…'}
            </Stat>
          </dl>
        )}
        <Row
          label="Enabled"
          description={
            s?.enabled === false
              ? 'dreaming.enabled is off in config: no routine, no sensor, no signals. Knowledge keys are still honoured.'
              : 'The dreaming routine: the same switch as its row on the Routines page.'
          }
        >
          <Switch
            checked={routine?.enabled ?? false}
            disabled={!routine || enable.isPending}
            aria-label="Dreaming enabled"
            onCheckedChange={(v) => enable.mutate(v)}
          />
        </Row>
        <WideRow label="Window" description="Local hours in which a night may start; the sensor also waits for 20 idle minutes and power.">
          <Input type="time" dictate={false} aria-label="Window start" className="w-36 tabular-nums" disabled={busy} value={settings.data?.window[0] ?? ''} onChange={(e) => change({ kind: 'window', index: 0, time: e.target.value })} />
          <span className="text-fg-tertiary">–</span>
          <Input type="time" dictate={false} aria-label="Window end" className="w-36 tabular-nums" disabled={busy} value={settings.data?.window[1] ?? ''} onChange={(e) => change({ kind: 'window', index: 1, time: e.target.value })} />
        </WideRow>
        <Row label="Budget per night" description={`Model spend a night may reach before it stops; default ${usd(1)}. Any amount.`}>
          <span className="text-sm text-fg-secondary">{USD_SYMBOL}</span>
          <Input
            key={settings.data?.budget_usd}
            type="number"
            dictate={false}
            inputMode="decimal"
            min={0}
            step="0.5"
            aria-label="Budget per night in USD"
            className="w-28 text-right"
            disabled={busy}
            defaultValue={settings.data?.budget_usd}
            onBlur={(e) => change({ kind: 'budget', usd: Number(e.currentTarget.value) })}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
        </Row>
      </Section>

      <Section title="Models">
        {TIERS.map((tier) => {
          const { model, seed } = resolvedDefault(tier, defaults)
          const name = model === null ? 'rules only' : (models.data?.find((m) => m.id === model)?.name ?? model) + (seed ? ', another seed' : '')
          return (
            <WideRow key={tier} label={TIER_LABEL[tier]} description={TIER_HELP[tier]}>
              <ModelCombobox
                aria-label={`${TIER_LABEL[tier]} model`}
                className="w-full max-w-72"
                models={models.data ?? []}
                value={settings.data?.models[tier]}
                none={{ label: `Default (${name})` }}
                disabled={busy}
                onChange={(v) => change({ kind: 'model', tier, model: v })}
              />
            </WideRow>
          )
        })}
      </Section>

      <Section title="Preferences and digest">
        <Row label="Auto-promote preferences" description="A standing preference is added on its own when you said it explicitly at least twice; off, every one is proposed first.">
          <Switch checked={settings.data?.preferences_auto ?? false} disabled={busy} aria-label="Auto-promote preferences" onCheckedChange={(v) => change({ kind: 'preferences_auto', on: v })} />
        </Row>
        <WideRow label="Weekly digest" description="One attention row a week with the nights' counts, undos and open proposals.">
          <Select value={digest?.day} disabled={busy} onValueChange={(v) => change({ kind: 'digest_day', day: v as Day })}>
            <SelectTrigger className="w-24" aria-label="Digest day">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DAYS.map((d) => (
                <SelectItem key={d} value={d}>
                  {d}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Input type="time" dictate={false} aria-label="Digest time" className="w-36 tabular-nums" disabled={busy} value={digest?.time ?? ''} onChange={(e) => change({ kind: 'digest_time', time: e.target.value })} />
        </WideRow>
        <Row
          label="Pause"
          description={paused && settings.data?.paused_until ? `Paused until ${new Date(settings.data.paused_until).toLocaleString()}; the routine stays enabled and resumes on its own.` : `Skip every night for ${PAUSE_DAYS} days without turning the routine off.`}
        >
          {paused ? (
            <Button variant="outline" size="sm" disabled={busy} onClick={() => change({ kind: 'resume' })}>
              Resume
            </Button>
          ) : (
            <Button variant="outline" size="sm" disabled={busy} onClick={() => change({ kind: 'pause', now: new Date() })}>
              Pause for {PAUSE_DAYS} days
            </Button>
          )}
        </Row>
      </Section>

      <Section title="Recent nights">
        {nights.isError ? <Empty>{nights.error.message}</Empty> : nights.data?.length === 0 ? <Empty>No nights yet.</Empty> : null}
        {nights.data && nights.data.length > 0 && (
          <ul className="divide-y divide-border-subtle">
            {nights.data.map((n) => (
              <NightRow key={n.night} night={n} />
            ))}
          </ul>
        )}
      </Section>

      <Section title="Proposals">
        {proposals.isError ? <Empty>{proposals.error.message}</Empty> : proposals.data?.length === 0 ? <Empty>Nothing proposed.</Empty> : null}
        {proposals.data && proposals.data.length > 0 && (
          <ul className="divide-y divide-border-subtle">
            {proposals.data.map((p) => (
              <ProposalRow key={p.id} proposal={p} />
            ))}
          </ul>
        )}
      </Section>
    </>
  )
}

const TIER_HELP: Record<Tier, string> = {
  triage: 'Sorts the fresh entries. Default is the rule-based classifier with no model at all.',
  tidy: 'Retitles, merges, archives, verifies — the cheap, high-volume calls.',
  propose: 'Writes skill edits and preference lines from the day\u2019s cases.',
  judge: 'Scores replays behind the gate; another vendor than propose when one is configured.',
}

/** A `Row` whose controls need the width: they drop under the label below `md` (113). */
function WideRow({ label, description, children }: { label: string; description: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 border-b border-border-subtle py-4 last:border-0 md:flex-row md:items-center md:justify-between md:gap-6">
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        <div className="mt-0.5 text-[13px] text-fg-secondary">{description}</div>
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  )
}

function Stat({ label, className, children }: { label: string; className?: string; children: ReactNode }) {
  return (
    <div className={className}>
      <dt className="text-xs text-fg-tertiary">{label}</dt>
      <dd className="mt-0.5 tabular-nums">{children}</dd>
    </div>
  )
}

/** The Dream now button and its dialog: budget (default $5) and mode (one pass | backlog) → `dreaming.run_now` → the run's chat. */
function DreamNow({ disabled }: { disabled: boolean }) {
  const api = useDreamingApi()
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const [budget, setBudget] = useState(String(RUN_NOW_BUDGET_USD))
  const [mode, setMode] = useState<RunMode>('pass')
  const run = useAction((req: { budget_usd: number; mode: RunMode }) => api['dreaming.run_now']({ ...req, run_timeout_secs: null }), {
    keys: [KEY, SCHEDULES],
    onSuccess: (session) => {
      setOpen(false)
      void navigate({ to: '/c/$session', params: { session } })
    },
  })
  const amount = Number(budget)
  const ok = Number.isFinite(amount) && amount > 0
  return (
    <>
      <Button size="sm" disabled={disabled} onClick={() => setOpen(true)}>
        <Moon /> Dream now
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-md">
          <form
            className="contents"
            onSubmit={(e) => {
              e.preventDefault()
              if (ok) run.mutate({ budget_usd: amount, mode })
            }}
          >
            <DialogHeader>
              <DialogTitle>Dream now</DialogTitle>
              <DialogDescription>Runs the routine at once with its own budget; the night's journal names the mode.</DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-2">
              <div className="grid gap-2">
                <Label htmlFor="dream-budget">Budget</Label>
                <div className="flex items-center gap-2">
                  <span className="text-sm text-fg-secondary">{USD_SYMBOL}</span>
                  <Input id="dream-budget" type="number" dictate={false} inputMode="decimal" min={0} step="0.5" autoFocus className="w-32 text-right" value={budget} aria-invalid={!ok} onChange={(e) => setBudget(e.target.value)} />
                </div>
              </div>
              <div className="grid gap-2">
                <Label>Mode</Label>
                <ToggleGroup type="single" variant="outline" size="sm" value={mode} aria-label="Mode" onValueChange={(v) => v && setMode(v as RunMode)}>
                  {RUN_MODES.map(([id, label]) => (
                    <ToggleGroupItem key={id} value={id}>
                      {label}
                    </ToggleGroupItem>
                  ))}
                </ToggleGroup>
                <p className="text-[13px] text-fg-secondary">{BACKLOG_SENTENCE}</p>
              </div>
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!ok || run.isPending}>
                Start
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  )
}

/** One night: counts · spend · mode · undone, with Undo → confirm → `dreaming.undo(night, null)`. */
function NightRow({ night: n }: { night: NightSummary }) {
  const api = useDreamingApi()
  const undo = useAction((night: string) => api['dreaming.undo'](night, null), {
    keys: [KEY, queryKeys.knowledgeList, queryKeys.knowledgeHistories],
    onSuccess: (r) => toast.message(`Undid ${r.night}`, { description: undoLine(r) }),
  })
  return (
    <li className="flex flex-col gap-2 py-3 md:flex-row md:items-center md:gap-4">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="font-medium tabular-nums">{n.night}</span>
          <Badge variant="outline" className="text-[10px]">
            {n.mode}
          </Badge>
          {n.undone && (
            <Badge variant="secondary" className="text-[10px]">
              undone
            </Badge>
          )}
          {n.finished_at === null && <span className="text-xs text-fg-tertiary">running</span>}
        </div>
        <div className="mt-0.5 text-[13px] text-fg-secondary">
          {countsLine(n.counts)} · {usd(n.spend_usd)}
        </div>
      </div>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" size="xs" disabled={n.undone || n.finished_at === null || undo.isPending}>
            <Undo2 /> Undo
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Undo {n.night}?</AlertDialogTitle>
            <AlertDialogDescription>Reverts that night's knowledge commits, skill edits and preference lines. Nothing written since is touched; the night stays in the list as undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => undo.mutate(n.night)}>
              Undo
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  )
}

/** One proposal (§4 phase 6): title, body, the option buttons → `dreaming.proposal_act(id, choice)`. */
function ProposalRow({ proposal: p }: { proposal: Proposal }) {
  const api = useDreamingApi()
  const act = useAction((choice: string) => api['dreaming.proposal_act'](p.id, choice), { keys: [KEY, SCHEDULES] })
  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="font-medium">{p.title}</span>
        <Badge variant="outline" className="text-[10px]">
          {p.kind.replace('_', ' ')}
        </Badge>
        <span className="text-xs text-fg-tertiary" title={new Date(p.created_at).toLocaleString()}>
          {relative(p.created_at)}
        </span>
      </div>
      {p.body && <p className="text-[13px] text-fg-secondary">{p.body}</p>}
      <div className="flex flex-wrap gap-2">
        {p.options.map((o, i) => (
          <Button key={o} variant={i === 0 ? 'default' : 'outline'} size="xs" disabled={act.isPending} onClick={() => act.mutate(o)}>
            {o}
          </Button>
        ))}
      </div>
    </li>
  )
}
