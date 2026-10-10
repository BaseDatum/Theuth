import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { Button } from '@/components/ui/button'
import { useAction } from '@/components/settings/shared'
import { cn } from '@/lib/utils'
import type { JudgmentRow } from '@/rpc/bindings/JudgmentRow'
import { shortSha, usd } from './model'
import { KEY, useAutoresearchApi } from './queries'

/** Pairs shown at most (newest first); the pending human pairs always come first. */
const SHOWN = 40

/** One case of one experiment: the judge's two ordered verdicts and the user's pair when there is one. */
export type Pair = {
  n: number
  caseId: string
  /** The candidate tree (`a_sha`) and the incumbent (`b_sha`). */
  candidate: string
  incumbent: string
  /** The two judge rows, in the order they were asked. */
  verdicts: JudgmentRow[]
  /** `a` the candidate won both orders, `b` the incumbent, `tie` otherwise (the gate's reading). */
  outcome: 'a' | 'b' | 'tie'
  human: JudgmentRow | null
  cost: number
}

/** Group `judgments` rows into pairs: judge rows by (n, case), the `human` row beside them. Probe rows (n = 0) are left out. */
export function pairsOf(rows: readonly JudgmentRow[]): Pair[] {
  const by = new Map<string, Pair>()
  for (const r of rows) {
    if (r.n === 0 || r.b_sha === null) continue
    const key = `${r.n}:${r.case_id}`
    let p = by.get(key)
    if (!p) {
      p = { n: r.n, caseId: r.case_id, candidate: r.a_sha, incumbent: r.b_sha, verdicts: [], outcome: 'tie', human: null, cost: 0 }
      by.set(key, p)
    }
    if (r.model === 'human') p.human = r
    else {
      p.verdicts.push(r)
      p.cost += r.cost_nanos / 1e9
    }
  }
  for (const p of by.values()) {
    const [x, y] = p.verdicts
    p.outcome = x && y && x.winner === y.winner && (x.winner === 'a' || x.winner === 'b') ? x.winner : 'tie'
  }
  const rank = (p: Pair) => (p.human && !p.human.human ? 0 : 1) * 1e9 - p.n
  return [...by.values()].sort((u, v) => rank(u) - rank(v) || u.caseId.localeCompare(v.caseId))
}

/** `a` | `b` | `tie` in ledger terms → the word for the candidate. */
const WORD: Record<'a' | 'b' | 'tie', string> = { a: 'candidate', b: 'incumbent', tie: 'tie' }

/** What the judge said in the terms the user sees: which position it picked and why. */
function shown(r: JudgmentRow): string {
  if (r.winner === null) return 'unparsed'
  if (r.winner === 'tie') return 'tie'
  const candidateShownAsA = r.a_first ?? true
  const pickedA = (r.winner === 'a') === candidateShownAsA
  return pickedA ? 'A' : 'B'
}

/**
 * 159 W3: the blind pairs of a study (or one experiment when `n` is set): per case the two
 * ordered verdicts with their reasons, the gate's reading, the judge's cost, and the human
 * pair — pending ones first, answered with A / B / same through `autoresearch.human_pair`.
 * The two output texts are not in `JudgmentRow` yet (hand-off to Z); the attention row
 * carries them, so here the user answers a pair they were shown there. Owner: W3.
 */
export function PairViewer({ slug, n }: { slug: string; n: number | null }) {
  const api = useAutoresearchApi()
  const q = useQuery({ queryKey: [...KEY, slug, 'judgments', n], queryFn: () => api['autoresearch.judgments'](slug, n) })
  const answer = useAction((v: { id: bigint; winner: 'a' | 'b' | 'same' }) => api['autoresearch.human_pair'](slug, Number(v.id), v.winner), { keys: [[...KEY, slug]] })
  const pairs = useMemo(() => pairsOf(q.data ?? []), [q.data])
  if (q.isPending) return <p className="text-xs text-fg-tertiary">Loading pairs…</p>
  if (pairs.length === 0) return <p className="text-xs text-fg-tertiary">No blind pairs yet{n === null ? '' : ` for experiment ${n}`}.</p>
  const total = pairs.reduce((s, p) => s + p.cost, 0)
  return (
    <div data-pairs={pairs.length} className="mt-3 space-y-2">
      <p className="text-xs text-fg-tertiary">
        {pairs.length} blind pair{pairs.length === 1 ? '' : 's'} · each judged twice with A and B swapped · judge cost {usd(total)}
      </p>
      <ul className="divide-y divide-border-subtle rounded-md border border-border-subtle">
        {pairs.slice(0, SHOWN).map((p) => (
          <li key={`${p.n}:${p.caseId}`} data-pair={`${p.n}:${p.caseId}`} className="space-y-1 px-3 py-2 text-xs">
            <div className="flex flex-wrap items-baseline gap-2">
              <span className="font-semibold">#{p.n}</span>
              <span className="font-mono">{p.caseId}</span>
              <span className="text-fg-tertiary">
                {shortSha(p.candidate)} vs {shortSha(p.incumbent)}
              </span>
              <span className={cn('ml-auto rounded-full px-2 py-px font-semibold', p.outcome === 'a' ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300' : p.outcome === 'b' ? 'bg-rose-500/15 text-rose-700 dark:text-rose-300' : 'bg-layer-2 text-fg-secondary')}>
                {WORD[p.outcome]}
              </span>
            </div>
            <ol className="space-y-0.5 text-fg-secondary">
              {p.verdicts.map((r, i) => (
                <li key={r.id.toString()}>
                  <span className="text-fg-tertiary">order {i + 1}{r.a_first ? ' (candidate as A)' : ' (incumbent as A)'}:</span> picked {shown(r)}
                  {r.reason && <span> — {r.reason}</span>}
                </li>
              ))}
            </ol>
            {p.human && (
              <div data-human={p.human.human ? 'answered' : 'pending'} className="flex flex-wrap items-center gap-2 pt-1">
                {p.human.human ? (
                  <span>
                    You said <strong>{p.human.winner === 'tie' ? 'same' : WORD[(p.human.winner as 'a' | 'b' | null) ?? 'tie']}</strong>
                    {p.human.winner === p.outcome ? ' — agrees with the judge' : ' — the judge disagreed'}
                  </span>
                ) : (
                  <>
                    <span className="text-fg-secondary">Your pair (shown in the attention row as A and B): which is better?</span>
                    {(['a', 'b', 'same'] as const).map((w) => (
                      <Button key={w} size="xs" variant="outline" disabled={answer.isPending} onClick={() => answer.mutate({ id: p.human!.id, winner: w })}>
                        {w === 'same' ? 'same' : w.toUpperCase()}
                      </Button>
                    ))}
                  </>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>
      {pairs.length > SHOWN && <p className="text-xs text-fg-tertiary">… {pairs.length - SHOWN} older pairs not shown</p>}
    </div>
  )
}
