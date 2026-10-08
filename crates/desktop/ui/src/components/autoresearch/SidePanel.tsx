import { Link } from '@tanstack/react-router'
import { Plus, Star, X } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { relative } from '@/components/routines/helpers'
import { cn } from '@/lib/utils'
import { pct } from './model'
import { useControl, useIdea, useSteer } from './queries'
import type { Attention, Idea, Steer, StudyDetail } from './types'

/** §13.1 the side panel: Tell the study… · Ideas · Playbook · Needs your eyes. */
export function SidePanel({ detail, onPick }: { detail: StudyDetail; onPick: (n: number) => void }) {
  const live = detail.info.state === 'running' || detail.info.state === 'paused' || detail.info.state === 'preparing'
  return (
    <div className="grid min-w-0 grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-1">
      <div className="flex min-w-0 flex-col gap-3">
        {live && <SteerBox slug={detail.info.slug} steers={detail.steers} />}
        {detail.attention.length > 0 && <NeedsEyes detail={detail} onPick={onPick} />}
        {!live && detail.steers.length > 0 && (
          <Panel title="Your notes">
            <SteerHistory steers={detail.steers} />
          </Panel>
        )}
      </div>
      <div className="flex min-w-0 flex-col gap-3">
        <Ideas slug={detail.info.slug} ideas={detail.ideas} live={live} />
        {detail.playbook && (
          <Panel title="Playbook" hint="kept by the study for its next experiments">
            <pre className="max-h-64 overflow-auto font-sans text-[13px] leading-5 whitespace-pre-wrap text-fg-secondary">{detail.playbook}</pre>
          </Panel>
        )}
      </div>
    </div>
  )
}

export function Panel({ title, hint, actions, children, className }: { title: ReactNode; hint?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={cn('rounded-[10px] border border-border-subtle bg-layer-1 p-3', className)}>
      <header className="mb-2 flex items-baseline gap-2">
        <h3 className="text-[13px] font-semibold">{title}</h3>
        {hint && <span className="min-w-0 truncate text-xs text-fg-tertiary">{hint}</span>}
        {actions && <div className="ml-auto flex shrink-0 items-center gap-1">{actions}</div>}
      </header>
      {children}
    </section>
  )
}

/** The steer box: posts `autoresearch.steer`, clears on success, lists the steers underneath (new ones first, unseen marked). */
function SteerBox({ slug, steers }: { slug: string; steers: Steer[] }) {
  const [text, setText] = useState('')
  const steer = useSteer(slug, () => setText(''))
  const send = () => {
    const t = text.trim()
    if (t) steer.mutate(t)
  }
  return (
    <Panel title="Tell the study…">
      <Textarea
        value={text}
        placeholder="e.g. Try SIMD for the lexer; don't touch the filters module."
        aria-label="Steer the study"
        className="min-h-16 text-[13px]"
        disabled={steer.isPending}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault()
            send()
          }
        }}
      />
      <div className="mt-2 flex items-center justify-between gap-2">
        <span className="text-xs text-fg-tertiary">Reaches the next experiment. Doesn’t pause it.</span>
        <Button size="sm" disabled={steer.isPending || text.trim() === ''} onClick={send}>
          Send
        </Button>
      </div>
      {steers.length > 0 && (
        <div className="mt-3 border-t border-border-subtle pt-2">
          <SteerHistory steers={steers} />
        </div>
      )}
    </Panel>
  )
}

function SteerHistory({ steers }: { steers: Steer[] }) {
  return (
    <ul className="flex flex-col gap-1.5">
      {[...steers].reverse().map((s) => (
        <li key={s.id} className="text-[13px]">
          <p className="text-foreground">{s.text}</p>
          <p className="text-xs text-fg-tertiary">
            {relative(s.at)}
            {s.seen_in === null ? <span className="ml-1 text-info">· new — in the next brief</span> : <span className="ml-1">· seen by #{s.seen_in}</span>}
          </p>
        </li>
      ))}
    </ul>
  )
}

/** Ideas: open ones first (★ the user's, by priority), tried/closed under a line; add (priority 3, source user) and close. */
function Ideas({ slug, ideas, live }: { slug: string; ideas: Idea[]; live: boolean }) {
  const [adding, setAdding] = useState(false)
  const [text, setText] = useState('')
  const [category, setCategory] = useState('')
  const act = useIdea(slug)
  const open = ideas.filter((i) => i.status === 'open').sort((a, b) => b.priority - a.priority || Date.parse(b.at) - Date.parse(a.at))
  const done = ideas.filter((i) => i.status !== 'open')
  const add = () => {
    const t = text.trim()
    if (!t) return
    act.mutate({ Add: { text: t, category: category.trim() || 'general', priority: 3 } }, { onSuccess: () => {
      setText('')
      setCategory('')
      setAdding(false)
    } })
  }
  return (
    <Panel
      title={`Ideas · ${open.length} open`}
      actions={
        live && (
          <Button variant="ghost" size="xs" aria-label="Add idea" aria-expanded={adding} onClick={() => setAdding((v) => !v)}>
            <Plus /> Add
          </Button>
        )
      }
    >
      {adding && (
        <form
          className="mb-2 flex flex-col gap-1.5 rounded-md border border-border-subtle bg-layer-2/60 p-2"
          onSubmit={(e) => {
            e.preventDefault()
            add()
          }}
        >
          <Input autoFocus value={text} placeholder="The idea, one line" aria-label="Idea" className="h-8 text-[13px]" onChange={(e) => setText(e.target.value)} />
          <div className="flex gap-1.5">
            <Input value={category} placeholder="category (algorithm, caching…)" aria-label="Category" className="h-8 flex-1 text-[13px]" onChange={(e) => setCategory(e.target.value)} />
            <Button type="submit" size="sm" disabled={act.isPending || text.trim() === ''}>
              Add
            </Button>
          </div>
        </form>
      )}
      {open.length === 0 && !adding ? <p className="text-[13px] text-fg-tertiary">No open ideas; the study will propose its own.</p> : null}
      <ul className="flex flex-col gap-1.5">
        {open.map((i) => (
          <li key={i.id} className="group/idea flex items-start gap-1.5 text-[13px]">
            {i.source === 'user' ? <Star className="mt-0.5 size-3.5 shrink-0 fill-warning text-warning" aria-label="Your idea" /> : <span className="mt-0.5 inline-block w-3.5 shrink-0 text-center text-fg-tertiary">·</span>}
            <span className="min-w-0 flex-1">
              {i.text} <span className="text-[11px] text-fg-tertiary">[{i.category}]</span>
              {i.source === 'advisor' && <span className="ml-1 text-[11px] text-info">advisor</span>}
            </span>
            {live && (
              <button
                type="button"
                aria-label={`Close idea: ${i.text}`}
                disabled={act.isPending}
                onClick={() => act.mutate({ Close: { id: i.id } })}
                className="shrink-0 rounded p-0.5 text-fg-tertiary opacity-0 transition-opacity group-hover/idea:opacity-100 hover:text-foreground focus-visible:opacity-100"
              >
                <X className="size-3.5" />
              </button>
            )}
          </li>
        ))}
      </ul>
      {done.length > 0 && (
        <details className="mt-2 text-xs text-fg-tertiary">
          <summary className="cursor-pointer select-none">{done.length} tried or closed</summary>
          <ul className="mt-1 flex flex-col gap-1">
            {done.map((i) => (
              <li key={i.id} className="line-through decoration-fg-tertiary/60">
                {i.text}
                {i.tried_in.length > 0 && <span className="ml-1 no-underline"> · #{i.tried_in.join(', #')}</span>}
              </li>
            ))}
          </ul>
        </details>
      )}
    </Panel>
  )
}

/** Needs your eyes: reverted by review (Restore / Keep reverted), deviations with justification, overfit, drift, unreviewed keeps. */
function NeedsEyes({ detail, onPick }: { detail: StudyDetail; onPick: (n: number) => void }) {
  const control = useControl(detail.info.slug)
  const unit = detail.spec.metric.unit
  const row = (a: Attention, i: number) => {
    const n = 'n' in a ? a.n : null
    const open = n !== null && (
      <button type="button" className="font-semibold hover:underline" onClick={() => onPick(n)}>
        #{n}
      </button>
    )
    switch (a.kind) {
      case 'reverted':
        return (
          <Item key={i} tone="warning">
            {open} <b>reverted by review</b> — “{a.reason}”
            <div className="mt-1.5 flex gap-1.5">
              <Button size="xs" variant="outline" disabled={control.isPending} onClick={() => control.mutate({ Restore: { n: a.n } })} title="Re-applies the patch as the next experiment and measures it again">
                Restore
              </Button>
              <Button size="xs" variant="ghost" disabled={control.isPending} onClick={() => control.mutate({ KeepReverted: { n: a.n } })}>
                Keep reverted
              </Button>
            </div>
          </Item>
        )
      case 'deviation':
        return (
          <Item key={i} tone="warning">
            {open} <b>deviation</b> — touched {a.paths.map((p, j) => (
              <code key={p} className="font-mono text-xs">
                {j > 0 && ', '}
                {p}
              </code>
            ))}{' '}
            outside scope{a.justification ? <>; justification: “{a.justification}”</> : <>; no justification given</>}
          </Item>
        )
      case 'overfit':
        return (
          <Item key={i} tone="danger">
            <b>overfit</b> — the search metric improved {pct(a.search_delta_pct)} but the holdout moved {pct(a.holdout_delta_pct)}. The study treats this as a plateau.
          </Item>
        )
      case 'drift':
        return (
          <Item key={i} tone="warning">
            {open} <b>drift</b> — the incumbent re-measured at {a.metric} {unit}, expected ≈ {a.expected} {unit}; the next confirm doubled.
          </Item>
        )
      case 'unreviewed':
        return (
          <Item key={i} tone="info">
            {open} <b>unreviewed keep</b> — the reviewer failed twice; the keep stands on the metric, custody and checks alone.
          </Item>
        )
    }
  }
  return (
    <Panel title={`Needs your eyes · ${detail.attention.length}`}>
      <ul className="flex flex-col gap-2">{detail.attention.map(row)}</ul>
      {detail.info.state === 'finished' && detail.report_doc && (
        <p className="mt-2 text-xs text-fg-tertiary">
          The{' '}
          <Link to="/notes/$id" params={{ id: detail.report_doc }} search={{}} className="underline">
            report
          </Link>{' '}
          lists the same rows.
        </p>
      )}
    </Panel>
  )
}

function Item({ tone, children }: { tone: 'warning' | 'danger' | 'info'; children: ReactNode }) {
  return <li className={cn('border-l-[3px] pl-2 text-[13px]', tone === 'warning' ? 'border-warning' : tone === 'danger' ? 'border-danger' : 'border-info')}>{children}</li>
}
