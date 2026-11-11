import { Link } from '@tanstack/react-router'
import { ChevronRight, Pause, Pencil, Play, Square, Trash2, X } from 'lucide-react'
import { Fragment, useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { DraftField } from '@/rpc/bindings/DraftField'
import type { DraftView } from '@/rpc/bindings/DraftView'
import type { StartWhen } from '@/rpc/bindings/StartWhen'
import { experimentsText, usd } from './model'
import { useControl, useDiscardDraft, useDraftConfirm, useDraftEdit, useStartDraft, useStudy } from './queries'
import { ADVANCED, LEAVES, PROBE_OF, STATE_BADGE, discardMessage, editMessage, leafValue, rowLabel, sparkPoints } from './setup'
import { Delta, StatePill } from './StudyList'

const MONO: Record<string, true> = { repo: true, bench: true, checks: true, holdout: true, scope: true, warm: true, evaluator_paths: true }
/** The default idle-window sensor when the spec names none (`tools::IDLE_SENSOR`). */
const IDLE_SENSOR = 'idle.py'
const MAIN_ROWS = 9

/**
 * 151 §4/§5 — the live Study draft panel. The engine computes every state; this renders `fields` in order with a
 * badge each, evidence (a sparkline from `probes`), OK on proposed rows, an inline editor per leaf, Advanced collapsed,
 * the footer from `blocking`, the Start buttons (enabled only when `ready`) and Discard. Before the first draft:
 * skeleton rows. `started` → the §7 status card.
 */
export function DraftPanel({ draft, session, onDiscarded, className }: { draft: DraftView | undefined; /** The setup session the §4.6 edit / discard messages go to. */ session: string | null; onDiscarded?: () => void; className?: string }) {
  return (
    <div className={cn('flex min-h-0 flex-col', className)}>
      <header className="shrink-0 border-b border-border-subtle px-3 py-2">
        <div className="flex items-baseline gap-2">
          <h3 className="text-[13px] font-semibold">{draft?.started ? 'Study' : 'Study draft'}</h3>
          {draft && (
            <span className="min-w-0 truncate font-mono text-xs text-fg-tertiary" title={draft.draft_id}>
              {draft.draft_id}
            </span>
          )}
          {draft && !draft.started && (
            <span className="ml-auto shrink-0 text-[11px] text-fg-secondary tabular-nums">
              {draft.settled} of {draft.total} settled
            </span>
          )}
        </div>
        {draft && !draft.started && (
          <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-layer-3">
            <div className="h-full rounded-full bg-success transition-[width]" style={{ width: `${draft.total ? (100 * draft.settled) / draft.total : 0}%` }} />
          </div>
        )}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">{!draft ? <Skeletons /> : draft.started ? <StatusCard slug={draft.draft_id} /> : <Rows draft={draft} session={session} onDiscarded={onDiscarded} />}</div>
    </div>
  )
}

function Skeletons() {
  return (
    <div className="flex flex-col gap-2 p-3">
      <p className="text-xs text-fg-tertiary">theuth fills this in as you talk.</p>
      {Array.from({ length: MAIN_ROWS }, (_, i) => (
        <div key={i} className="flex items-center gap-2">
          <Skeleton className="h-3 w-16" />
          <Skeleton className={cn('h-3 flex-1', i % 3 === 1 && 'w-1/2 flex-none')} />
        </div>
      ))}
    </div>
  )
}

function Rows({ draft, session, onDiscarded }: { draft: DraftView; session: string | null; onDiscarded?: () => void }) {
  const api = useApi()
  const slug = draft.draft_id
  const [editing, setEditing] = useState<string | null>(null)
  const confirm = useDraftConfirm(slug)
  // §4.6: one `send_input` per successful leaf edit, so the agent sees what the user set.
  const edit = useDraftEdit(slug, (v) => {
    if (session) void api.send_input(session, editMessage(slug, v.field, v.value), [], null)
  })
  const discard = useDiscardDraft(() => {
    if (session) void api.send_input(session, discardMessage(slug), [], null)
    onDiscarded?.()
  })
  const start = useStartDraft(slug)
  const busy = confirm.isPending || edit.isPending || discard.isPending || start.isPending
  const startWith = (when: StartWhen, includeDirty = false) => async () => {
    // R-11: the base of "Start with my uncommitted changes" is a snapshot of the dirty tree; `create` reads `spec.include_dirty`.
    if (includeDirty) await edit.mutateAsync({ field: 'include_dirty', value: 'true' })
    start.mutate(when)
  }
  const main = draft.fields.filter((f) => !ADVANCED[f.name])
  const advanced = draft.fields.filter((f) => ADVANCED[f.name])
  const row = (f: DraftField) => (
    <Row
      key={f.name}
      field={f}
      draft={draft}
      editing={editing === f.name}
      busy={busy}
      onOk={() => confirm.mutate(f.name)}
      onEdit={() => setEditing(editing === f.name ? null : f.name)}
      onSave={async (changes) => {
        for (const c of changes) await edit.mutateAsync(c)
        setEditing(null)
      }}
    />
  )
  return (
    <div className="flex flex-col">
      <div className="divide-y divide-border-subtle">{main.map(row)}</div>
      {advanced.length > 0 && (
        <Collapsible>
          <CollapsibleTrigger asChild>
            <button type="button" className="group flex w-full items-center gap-1 border-t border-border-subtle px-3 py-2 text-xs text-fg-secondary hover:text-foreground">
              <ChevronRight className="size-3 transition-transform group-data-[state=open]:rotate-90" /> Advanced
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent className="divide-y divide-border-subtle border-t border-border-subtle">{advanced.map(row)}</CollapsibleContent>
        </Collapsible>
      )}
      <footer className="flex flex-col gap-2 border-t border-border-subtle p-3">
        <div className="flex flex-wrap gap-1.5">
          <Button size="xs" disabled={!draft.ready || busy} onClick={() => void startWith({ when: 'now' })()}>
            <Play /> Start now
          </Button>
          <Button size="xs" variant="outline" disabled={!draft.ready || busy} onClick={() => void startWith({ when: 'window', sensor: draft.spec.window?.trim() || IDLE_SENSOR })()}>
            Start in idle window
          </Button>
          {draft.dirty && (
            <Button size="xs" variant="outline" disabled={!draft.ready || busy} onClick={() => void startWith({ when: 'now' }, true)()}>
              Start with my uncommitted changes
            </Button>
          )}
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => discard.mutate(slug)} title="Deletes the draft; the chat stays">
            <Trash2 /> Discard
          </Button>
        </div>
        <p className="text-[11px] text-fg-secondary">{footerText(draft)}</p>
        <p className="text-[11px] text-fg-tertiary">Verified = theuth's engine ran it · Confirmed = you said so · Needs your OK = theuth's proposal · Edit any row to set it yourself</p>
      </footer>
    </div>
  )
}

type Change = { field: string; value: string }

/**
 * R1 H5: a `ready` draft can still carry theuth's proposal on a non-required row (Budget, Holdout, …); the footer
 * says so instead of "Everything is settled".
 */
export function footerText(draft: DraftView): string {
  if (!draft.ready) return `Start unlocks when: ${draft.blocking.join(' · ')}`
  const proposed = draft.fields.filter((f) => f.state === 'proposed').map((f) => rowLabel(f.name))
  if (proposed.length === 0) return 'Everything is settled. Nothing runs on your repo until you press Start.'
  return `Ready to start · ${proposed.join(', ')} ${proposed.length === 1 ? 'is theuth\u2019s proposal — OK or edit it' : 'are theuth\u2019s proposals — OK or edit them'}`
}

/** R1 U1: label + badge + controls on one line, the value full width below (mono only for commands, paths, globs), evidence under it. */
function Row({ field: f, draft, editing, busy, onOk, onEdit, onSave }: { field: DraftField; draft: DraftView; editing: boolean; busy: boolean; onOk: () => void; onEdit: () => void; onSave: (changes: Change[]) => Promise<void> }) {
  const badge = STATE_BADGE[f.state]
  const probe = PROBE_OF[f.name] ? draft.probes.find((p) => p.kind === PROBE_OF[f.name]) : undefined
  const spark = f.state === 'verified' && probe ? sparkPoints(probe.values) : null
  const label = rowLabel(f.name)
  return (
    <div className={cn('flex flex-col gap-1 px-3 py-2', f.state === 'proposed' && 'bg-warning-bg/30')} data-row={f.name} data-state={f.state}>
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-fg-secondary">{label}</span>
        <span className={cn('inline-flex h-5 shrink-0 items-center rounded-full border px-2 text-[11px] font-medium whitespace-nowrap', badge.className)}>{badge.text}</span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {f.state === 'proposed' && !editing && (
            <Button size="xs" variant="outline" disabled={busy} onClick={onOk} aria-label={`OK ${label}`}>
              OK
            </Button>
          )}
          <Button size="icon-xs" variant="ghost" disabled={busy} onClick={onEdit} aria-label={editing ? `Cancel editing ${label}` : `Edit ${label}`} title={editing ? 'Cancel' : 'Edit'}>
            {editing ? <X /> : <Pencil />}
          </Button>
        </span>
      </div>
      <div className={cn('min-w-0 text-[13px] leading-5 break-words', MONO[f.name] && 'font-mono text-xs', !f.value && 'text-fg-tertiary')}>{f.value || (f.state === 'missing' ? 'not set yet' : 'none')}</div>
      {(f.evidence || spark) && (
        <div className="flex items-center gap-2 text-[11px] text-fg-tertiary">
          {spark && (
            <svg width="56" height="12" viewBox="0 0 56 12" aria-hidden className="shrink-0 text-success">
              <polyline points={spark} fill="none" stroke="currentColor" strokeWidth="1.5" />
            </svg>
          )}
          {f.evidence && <span className="min-w-0 truncate">{f.evidence}</span>}
        </div>
      )}
      {editing && <Editor key={draft.version} row={f.name} draft={draft} busy={busy} onSave={onSave} />}
    </div>
  )
}

/** §4.3: one text input per leaf of the row, in the engine's grammar (a `models.*` leaf is the standard model picker; empty = default); Save sends `draft_edit` for every changed leaf. */
function Editor({ row, draft, busy, onSave }: { row: string; draft: DraftView; busy: boolean; onSave: (changes: Change[]) => Promise<void> }) {
  const leaves = LEAVES[row] ?? [row]
  const models = useModels()
  const initial = Object.fromEntries(leaves.map((l) => [l, leafValue(draft.spec, l)]))
  const [values, setValues] = useState<Record<string, string>>(initial)
  const changes = leaves.filter((l) => values[l] !== initial[l]).map((l) => ({ field: l, value: values[l] ?? '' }))
  return (
    <form
      className="grid grid-cols-[auto_1fr] items-center gap-x-2 gap-y-1 text-xs"
      onSubmit={(e) => {
        e.preventDefault()
        if (changes.length) void onSave(changes)
      }}
    >
      {leaves.map((l) => (
        <Fragment key={l}>
          <label htmlFor={`leaf-${l}`} className="font-mono text-[11px] text-fg-tertiary">
            {l}
          </label>
          {l.startsWith('models.') ? (
            <ModelCombobox id={`leaf-${l}`} aria-label={l} size="sm" className="h-7 w-full text-xs" models={models.data ?? []} value={values[l] || null} none={{ label: 'Default' }} onChange={(id) => setValues({ ...values, [l]: id ?? '' })} />
          ) : (
            <Input id={`leaf-${l}`} value={values[l] ?? ''} className="h-7 font-mono text-base md:text-xs" autoFocus={l === leaves[0]} onChange={(e) => setValues({ ...values, [l]: e.target.value })} />
          )}
        </Fragment>
      ))}
      <Button type="submit" size="xs" className="col-start-2 justify-self-start" disabled={busy || changes.length === 0}>
        Save
      </Button>
    </form>
  )
}

/** §7: after Start the panel is the study's status card from `autoresearch.get`. */
export function StatusCard({ slug }: { slug: string }) {
  const study = useStudy(slug)
  const control = useControl(slug)
  const info = study.data?.info
  const st = info?.state
  return (
    <div className="flex flex-col gap-3 p-3">
      {info && st ? (
        <>
          <div className="flex items-center gap-2">
            <StatePill state={st} currentN={info.current} stop={info.stop_reason} />
            {info.phase && <span className="text-xs text-fg-secondary">{info.phase}</span>}
          </div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[13px]">
            <dt className="text-xs text-fg-secondary">Experiments</dt>
            <dd className="tabular-nums">{experimentsText(info)}</dd>
            <dt className="text-xs text-fg-secondary">Budget</dt>
            <dd className="tabular-nums">
              {usd(info.budget_used.usd)} of {usd(info.budget.usd)}
            </dd>
            <dt className="text-xs text-fg-secondary">Best vs base</dt>
            <dd>{info.delta_pct === null ? <span className="text-fg-tertiary">{st.state === 'preparing' ? 'measuring baseline' : '—'}</span> : <Delta direction={info.metric.direction} delta_pct={info.delta_pct} className="font-medium" />}</dd>
          </dl>
          <div className="flex flex-wrap gap-1.5">
            {st.state === 'paused' ? (
              <Button size="xs" variant="outline" disabled={control.isPending} onClick={() => control.mutate({ op: 'resume' })}>
                <Play /> Resume
              </Button>
            ) : (
              <Button size="xs" variant="outline" disabled={control.isPending || !(st.state === 'running' || st.state === 'preparing')} onClick={() => control.mutate({ op: 'pause' })}>
                <Pause /> Pause
              </Button>
            )}
            <Button size="xs" variant="outline" disabled={control.isPending || st.state === 'finished' || st.state === 'failed' || st.state === 'discarded'} onClick={() => control.mutate({ op: 'stop_after' })}>
              <Square /> Stop
            </Button>
            <Button asChild size="xs" variant="ghost">
              <Link to="/autoresearch/$slug" params={{ slug }} search={{}}>
                Open study <ChevronRight />
              </Link>
            </Button>
          </div>
        </>
      ) : study.isError ? (
        <p className="text-sm text-danger">{study.error.message}</p>
      ) : (
        <>
          <Skeleton className="h-5 w-24" />
          <Skeleton className="h-16 w-full" />
        </>
      )}
      <p className="text-[11px] text-fg-tertiary">This chat stays the study's line: ask here to pause, steer or add ideas. The study page links back as "Setup chat".</p>
    </div>
  )
}
