// 126 §10: the owner's double on a decision card — the chip ("double · Allow · 91 %" + rung glyph), its explanation
// popover, the L2 countdown with take-over, and the "answered by double · undo" line. One component on every card:
// `ApprovalCard`, `AskCard` (and its relay), the attention screens and rows. Behaviour lives in the engine; this file
// renders `DoublePredicted` / `DoubleAnswered` (`store/double`) and calls `double_take_over` / `double_forget`.
import { type MouseEvent, type ReactNode, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import type { Api } from '@/rpc/bindings/client'
import type { DoublePredicted } from "@/rpc/bindings/event.double_2epredicted.output"
import { doubleStore, useAnswered, useDouble, usePrediction } from '@/store/double'
import { useScopedDisclosure } from './disclosure'

/** ○ L0 · ◐ L1 · ● L2 · ●● L3 (§10). */
export const rungGlyph = (rung: number): string => (rung <= 0 ? '○' : rung === 1 ? '◐' : rung === 2 ? '●' : '●●')

const RUNG_NAME = ['observe', 'suggest', 'act with grace', 'silent']

export const pct = (p: number): string => `${Math.round(p * 100)} %`

/** `double · Allow · 91 %`. */
export const chipText = (p: Pick<DoublePredicted, 'choice' | 'p'>): string => `double · ${p.choice} · ${pct(p.p)}`

/** Whole seconds until `iso`, never negative; `null` when `iso` is absent or unparsable. */
export function secondsLeft(iso: string | null | undefined, now = Date.now()): number | null {
  if (!iso) return null
  const t = Date.parse(iso)
  return Number.isNaN(t) ? null : Math.max(0, Math.ceil((t - now) / 1000))
}

/** `double answers Allow in 28 s — tap to take over`; at 0 the answer is on its way. */
export const countdownText = (choice: string, secs: number): string => (secs > 0 ? `double answers ${choice} in ${secs} s — tap to take over` : `double answers ${choice} now…`)

/**
 * The option the double pre-selects at L1+ (§9.2: from `DoublePredicted`, never by rewriting `Question.recommended`):
 * `index` when it names an option, else the option whose label is `choice` (case-insensitive: the wire says `Allow`,
 * the `Decision` is `allow`); `null` at L0, for `none` and for an unknown label.
 */
export function preselect(p: Pick<DoublePredicted, 'rung' | 'choice' | 'index'> | undefined, options: readonly string[]): number | null {
  if (!p || p.rung < 1 || p.choice === 'none') return null
  if (p.index !== undefined && p.index !== null && p.index >= 0 && p.index < options.length) return p.index
  const want = p.choice.trim().toLowerCase()
  const i = options.findIndex((o) => o.trim().toLowerCase() === want)
  return i < 0 ? null : i
}

/** The model recommended one option and the double (at L1+) expects another: the card shows both (§9 L1). */
export const disagrees = (p: Pick<DoublePredicted, 'rung' | 'choice'> | undefined, recommended: string | undefined): boolean =>
  !!p && p.rung >= 1 && recommended !== undefined && recommended !== p.choice && p.choice !== 'none'

/**
 * `double_take_over(id)`: `true` cancels the grace (or undoes); `false` means the double had already answered. With
 * `onError` (the Attention window, which never toasts) a failure goes there and "already answered" says nothing.
 */
export async function takeOver(api: Pick<Api, 'double_take_over'>, id: string, onError?: (error: string) => void): Promise<boolean> {
  try {
    const ok = await api.double_take_over(id)
    if (ok) doubleStore.getState().takeOver(id)
    else if (!onError) toast.message('double had already answered')
    return ok
  } catch (e) {
    const text = e instanceof Error ? e.message : String(e)
    if (onError) onError(text)
    else toast.error(text)
    return false
  }
}

/** A one-second clock while `iso` lies ahead; `null` when there is nothing to count. Plain `setInterval`, no animation. */
export function useCountdown(iso: string | null | undefined): number | null {
  const [secs, setSecs] = useState(() => secondsLeft(iso))
  useEffect(() => {
    setSecs(secondsLeft(iso))
    if (!iso || (secondsLeft(iso) ?? 0) <= 0) return
    const t = setInterval(() => {
      const s = secondsLeft(iso)
      setSecs(s)
      if (s === null || s <= 0) clearInterval(t)
    }, 1000)
    return () => clearInterval(t)
  }, [iso])
  return secs
}

/** What one card knows about its double: the prediction, the answer, the live grace / undo windows. */
export function useDoubleCard(ids: readonly (string | null | undefined)[]) {
  const prediction = usePrediction(...ids)
  const answered = useAnswered(...ids)
  const id = prediction?.question_id ?? answered?.question_id
  const taken = useDouble((s) => (id ? !!s.takenOver[id] : false))
  const grace = useCountdown(!answered && !taken && prediction?.rung === 2 ? prediction.will_act_at : null)
  const undo = useCountdown(answered && !taken ? answered.undo_until : null)
  return {
    id,
    prediction,
    answered,
    /** Seconds of L2 grace left; `null` when the card is not counting down. */
    grace: grace !== null && grace > 0 ? grace : null,
    /** Seconds of undo left after a double answer; `null` when undo is over or never existed. */
    undo: undo !== null && undo > 0 ? undo : null,
    taken,
  }
}

/** The chip. `recommended`: the model's own recommended label, shown beside the double's when they disagree. */
export function DoubleChip({ ids, recommended, className }: { ids: readonly (string | null | undefined)[]; recommended?: string; className?: string }) {
  const api = useApi()
  const card = useDoubleCard(ids)
  const [shown, setShown] = useScopedDisclosure(`double|${ids.join('|')}`)
  const p = card.prediction
  if (!p) return null
  const stop = (e: MouseEvent) => e.stopPropagation()
  if (card.answered && card.id) {
    return (
      <span className={cn('inline-flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary', className)} onClick={stop}>
        <span>answered by double · {card.answered.choice}</span>
        {card.undo !== null && (
          <Button size="xs" variant="outline" onClick={() => void takeOver(api, card.id as string)}>
            undo · {card.undo} s
          </Button>
        )}
      </span>
    )
  }
  const text = card.grace !== null ? countdownText(p.choice, card.grace) : chipText(p)
  return (
    <Popover open={shown} onOpenChange={setShown}>
      <PopoverTrigger asChild>
        <button
          type="button"
          onClick={stop}
          aria-label={`double: ${p.choice}, ${pct(p.p)}, rung ${p.rung}`}
          className={cn(
            'inline-flex max-w-full items-center gap-1.5 rounded-full border px-2 py-0.5 text-left text-xs leading-5 tabular-nums transition-colors hover:bg-accent/60',
            p.rung <= 0 && 'border-transparent text-fg-tertiary',
            p.rung === 1 && 'border-border text-fg-secondary',
            p.rung >= 2 && 'border-fg-accent bg-accent text-fg font-medium',
            card.grace !== null && 'text-sm',
            className,
          )}
        >
          <span aria-hidden>{rungGlyph(p.rung)}</span>
          <span className="min-w-0 break-words">{text}</span>
          {disagrees(p, recommended) && <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">model recommends {recommended}</Badge>}
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 text-xs" onClick={stop}>
        <Explain p={p} />
      </PopoverContent>
    </Popover>
  )
}

/** The popover: class key, rung, what would promote it, the nearest past decisions, the head's parts, "don't learn from this one". */
function Explain({ p }: { p: DoublePredicted }) {
  const api = useApi()
  const [forgotten, setForgotten] = useState(false)
  const [busy, setBusy] = useState(false)
  const forget = async (on: boolean) => {
    setBusy(true)
    try {
      // `examples` has one row per `question_id` (store.rs `examples_question`); the prediction carries no `examples.id`.
      await api.double_forget({ target: 'examples', ids: [p.question_id] })
      setForgotten(on)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  const parts = p.explain.parts
  return (
    <div className="flex flex-col gap-2">
      <div className="font-mono text-[11px] break-all text-fg-secondary">{p.class}</div>
      <div>
        {rungGlyph(p.rung)} L{p.rung} {RUNG_NAME[p.rung] ?? ''} · {p.choice} {pct(p.p)} · v{p.version}
      </div>
      {p.probs.length > 1 && <div className="text-fg-secondary">{p.probs.map(([l, q]) => `${l} ${pct(q)}`).join(' · ')}</div>}
      <div className="text-fg-secondary">oracle {pct(parts.oracle)} · memory {pct(parts.memory)} · prior {pct(parts.prior)}</div>
      {p.explain.promote_hint && <div>{p.explain.promote_hint}</div>}
      {p.explain.neighbours.length > 0 && (
        <ul className="flex flex-col gap-1 border-t border-border-subtle pt-2">
          {p.explain.neighbours.slice(0, 3).map((n, i) => (
            <li key={i} className="flex flex-col">
              <span className="text-fg-secondary tabular-nums">
                {formatAt(Date.parse(n.asked_at))} · chose <b className="text-fg">{n.chose}</b>
              </span>
              <span className="line-clamp-2 text-fg-tertiary">{n.card_excerpt}</span>
            </li>
          ))}
        </ul>
      )}
      <label className="flex items-center justify-between gap-2 border-t border-border-subtle pt-2">
        <span>don't learn from this one</span>
        <Switch checked={forgotten} disabled={busy || forgotten} onCheckedChange={(v) => void forget(v)} aria-label="don't learn from this one" />
      </label>
    </div>
  )
}

/**
 * The card frame during an L2 grace: the whole card is the take-over target (a phone's bottom stack, 112) while the
 * card's own buttons stay live — a tap on a control is not a take-over, it is the answer.
 */
/** `anchor`: the prompt id the composer's approval shield scrolls to (`data-prompt-card`). */
export function DoubleCard({ ids, className, children, anchor }: { ids: readonly (string | null | undefined)[]; className?: string; children: ReactNode; anchor?: string }) {
  const api = useApi()
  const card = useDoubleCard(ids)
  const live = card.grace !== null && card.id !== undefined
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    if (!live || (e.target as Element).closest('button,a,input,textarea,select,[role=button],[role=dialog]')) return
    void takeOver(api, card.id as string)
  }
  return (
    <div
      data-prompt-card={anchor}
      className={cn(className, live && 'cursor-pointer ring-2 ring-fg-accent/40')}
      role={live ? 'button' : undefined}
      tabIndex={live ? 0 : undefined}
      aria-label={live && card.prediction ? countdownText(card.prediction.choice, card.grace as number) : undefined}
      onClick={onClick}
      onKeyDown={(e) => {
        if (live && (e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) {
          e.preventDefault()
          void takeOver(api, card.id as string)
        }
      }}
    >
      {children}
    </div>
  )
}

/** The undo line of an attention row / screen (the inbox carries undo too, §10). */
export function DoubleUndo({ ids, size = 'xs', onError }: { ids: readonly (string | null | undefined)[]; size?: 'xs' | 'sm' | 'default'; onError?: (error: string) => void }) {
  const api = useApi()
  const card = useDoubleCard(ids)
  if (!card.answered || card.undo === null || !card.id) return null
  return (
    <Button size={size} variant="outline" onClick={() => void takeOver(api, card.id as string, onError)}>
      Undo · {card.undo} s
    </Button>
  )
}
