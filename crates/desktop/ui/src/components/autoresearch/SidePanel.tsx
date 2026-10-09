import { Link } from '@tanstack/react-router'
import { Plus, Star, X } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { relative } from '@/components/routines/helpers'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import type { ExperimentRow } from '@/rpc/bindings/ExperimentRow'
import type { StudyDetail } from '@/rpc/bindings/StudyDetail'
import type { StudyIdea } from '@/rpc/bindings/StudyIdea'
import type { StudySteer } from '@/rpc/bindings/StudySteer'
import { useControl, useIdea, useSteer } from './queries'

/** §13.1 the side panel: Tell the study… · Needs your eyes · Ideas · Playbook. */
export function SidePanel({ detail, rows, onPick }: { detail: StudyDetail; rows: ExperimentRow[]; onPick: (n: number) => void }) {
  const st = detail.info.state.state
  const live = st === 'running' || st === 'paused' || st === 'preparing'
  return (
    <div className="grid min-w-0 grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-1">
      <div className="flex min-w-0 flex-col gap-3">
        {live && <SteerBox slug={detail.info.slug} steers={detail.steers} />}
        {detail.needs_eyes.length > 0 && <NeedsEyes detail={detail} rows={rows} onPick={onPick} />}
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

/** The steer box: posts `autoresearch.steer`, clears on success, lists the steers underneath (newest first, unseen marked). */
function SteerBox({ slug, steers }: { slug: string; steers: StudySteer[] }) {
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

function SteerHistory({ steers }: { steers: StudySteer[] }) {
  return (
    <ul className="flex flex-col gap-1.5">
      {steers.map((s) => (
        <li key={s.id} className="text-[13px]">
          <p className="text-foreground">{s.text}</p>
          <p className="text-xs text-fg-tertiary">
            {relative(s.at)}
            {s.by !== 'user' && ` · ${s.by}`}
            {s.seen_in === null ? <span className="ml-1 text-info">· new — in the next brief</span> : <span className="ml-1">· seen by #{s.seen_in}</span>}
          </p>
        </li>
      ))}
    </ul>
  )
}

/** Ideas: open ones first (★ the user's, by priority), tried/closed under a line; add (priority 3, source user), reprioritise (0–3, R-90) and close. */
function Ideas({ slug, ideas, live }: { slug: string; ideas: StudyIdea[]; live: boolean }) {
  const [adding, setAdding] = useState(false)
  const [text, setText] = useState('')
  const [category, setCategory] = useState('')
  const act = useIdea(slug)
  const open = ideas.filter((i) => i.status === 'open').sort((a, b) => b.priority - a.priority || Date.parse(b.at) - Date.parse(a.at))
  const done = ideas.filter((i) => i.status !== 'open')
  const add = () => {
    const t = text.trim()
    if (!t) return
    act.mutate(
      { op: 'add', text: t, category: category.trim() || 'general', priority: 3 },
      {
        onSuccess: () => {
          setText('')
          setCategory('')
          setAdding(false)
        },
      },
    )
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
            {live ? (
              <select
                aria-label={`Priority of idea: ${i.text}`}
                value={i.priority}
                disabled={act.isPending}
                onChange={(e) => act.mutate({ op: 'priority', id: i.id, priority: Number(e.target.value) })}
                className="h-5 shrink-0 rounded border border-border-subtle bg-layer-2 px-0.5 text-[11px] text-fg-secondary tabular-nums"
                title="Priority: 3 first"
              >
                {[3, 2, 1, 0].map((p) => (
                  <option key={p} value={p}>
                    p{p}
                  </option>
                ))}
              </select>
            ) : (
              <span className="shrink-0 text-[11px] text-fg-tertiary tabular-nums">p{i.priority}</span>
            )}
            {live && (
              <button
                type="button"
                aria-label={`Close idea: ${i.text}`}
                disabled={act.isPending}
                onClick={() => act.mutate({ op: 'close', id: i.id })}
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
              <li key={i.id} className="flex items-start gap-1.5">
                <span className="min-w-0 flex-1 line-through decoration-fg-tertiary/60">{i.text}</span>
                {i.tried_in.length > 0 && <span className="shrink-0">#{i.tried_in.join(', #')}</span>}
                {live && i.status === 'closed' && (
                  <button type="button" className="shrink-0 hover:text-foreground" disabled={act.isPending} onClick={() => act.mutate({ op: 'reopen', id: i.id })}>
                    reopen
                  </button>
                )}
              </li>
            ))}
          </ul>
        </details>
      )}
    </Panel>
  )
}

/**
 * Needs your eyes: the plugin's lines (reverted, deviations, overfit, drift, unreviewed). A line for an experiment the
 * review reverted carries Restore (`restore`: the harness re-applies the change as the next experiment and measures it
 * again) and Keep reverted (`keep_reverted`: dismisses the line and the attention row). The plugin drops a reverted
 * line once either was pressed.
 */
function NeedsEyes({ detail, rows, onPick }: { detail: StudyDetail; rows: ExperimentRow[]; onPick: (n: number) => void }) {
  const control = useControl(detail.info.slug)
  const live = detail.info.state.state === 'running' || detail.info.state.state === 'paused'
  const reverted = rows.filter((r) => r.state === 'rejected_by_review')
  // The plugin's reverted line ends "· Restore · Keep reverted"; the buttons below say that (R-96).
  const lines = detail.needs_eyes.map((l) => l.replace(/\s*·\s*Restore(\s*·\s*Keep reverted)?\s*$/, ''))
  return (
    <Panel title={`Needs your eyes · ${lines.length}`}>
      <ul className="flex flex-col gap-2">
        {lines.map((line, i) => {
          const m = /^#(\d+)\b/.exec(line)
          const n = m ? Number(m[1]) : null
          const rev = n !== null && reverted.some((r) => r.n === n)
          // An auto-opened segment (§5.3 Revision 2) is a note, not a worry.
          const tone = /overfit/i.test(line) ? 'danger' : /unreviewed|opened automatically/i.test(line) ? 'info' : 'warning'
          return (
            <Item key={`${i}-${line.slice(0, 24)}`} tone={tone}>
              {n !== null ? (
                <>
                  <button type="button" className="font-semibold hover:underline" onClick={() => onPick(n)}>
                    #{n}
                  </button>
                  {line.slice(m?.[0].length ?? 0)}
                </>
              ) : (
                line
              )}
              {rev && (
                <div className="mt-1.5 flex gap-1.5">
                  {live && (
                    <Button size="xs" variant="outline" disabled={control.isPending} onClick={() => control.mutate({ op: 'restore', n })} title="The harness re-applies the change as the next experiment and measures it again">
                      Restore
                    </Button>
                  )}
                  <Button size="xs" variant="ghost" disabled={control.isPending} onClick={() => control.mutate({ op: 'keep_reverted', n })} title="Leave it reverted; dismiss this line and its inbox row">
                    Keep reverted
                  </Button>
                </div>
              )}
            </Item>
          )
        })}
      </ul>
      {detail.info.state.state === 'finished' && detail.info.report_doc && !detail.info.report_doc.includes('/') && (
        <p className="mt-2 text-xs text-fg-tertiary">
          The{' '}
          <Link to="/notes/$id" params={{ id: detail.info.report_doc }} search={{}} className="underline">
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
