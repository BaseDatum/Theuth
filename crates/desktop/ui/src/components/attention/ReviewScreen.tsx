import { Link } from '@tanstack/react-router'
import { CheckCircle2, Circle } from 'lucide-react'
import { useState } from 'react'
import { useAttentionActions } from '@/components/hud/queries'
import { age, ownerText } from '@/components/tasks/helpers'
import { useTask } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { PacketBoard } from './PacketBoard'
import { PacketMarkdown } from './PacketMarkdown'
import { Pointers } from './Pointers'
import { usePacket } from './queries'
import { Body, Chip, Footer, IfWrong, OptionCards, PrioritySelect, SectionTitle, useResolved } from './shared'

/**
 * 101 D9: the task · by whom · age → the agent's board (101b R4; the `summary` when there is none) → Acceptance
 * (`done_when` + the agent's checks, read-only) → "Review these" (pointers inline) → 132 D3 "Next move" (the packet's
 * `options`, recommended pre-picked; a second tap on the lit card clears the pick) → a bottom bar that sticks: 132 D2
 * `If wrong` · **Approve → ‹picked label›** (`task_move(task, 'done', note, next)`; `Approve → done` without a pick, no
 * `next` sent) · Send back (a required note) · Open task · Snooze · Dismiss · 132 D4 Priority. Without a packet the
 * summary says so and the actions still work.
 */
export function ReviewScreen({ item }: { item: AttentionItem }) {
  const packet = usePacket(item.packet)
  const task = useTask(item.task ?? '', item.task !== null)
  const act = useAttentionActions()
  const onResolved = useResolved(item)
  const [sendingBack, setSendingBack] = useState(false)
  const [note, setNote] = useState('')
  // `undefined` = untouched, so the recommendation applies once the packet lands without an effect.
  const [picked, setPicked] = useState<number | null | undefined>(undefined)
  const p = packet.data ?? null
  const moves = p?.options ?? []
  const next = picked === undefined ? (p?.recommended ?? null) : picked
  /** The lit next move, only when it indexes a real option — what `Approve` names and sends. */
  const pick = next !== null && moves[next] ? next : undefined
  const nextLabel = pick === undefined ? undefined : moves[pick]?.label
  const busy = act.move.isPending
  const by = item.agent ?? (task.data ? ownerText(task.data.owner).replace(/^agent:/, '') : null)
  const move = (state: 'done' | 'doing', n: string | null) => {
    if (!item.task) return
    void act.move.mutateAsync({ id: item.task, state, note: n, next: state === 'done' ? pick : undefined }).then(onResolved, () => undefined)
  }
  return (
    <>
      <Body>
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2 text-xs text-fg-tertiary">
            <Chip kind="review" />
            {by && <span>by {by}</span>}
            <span className="ml-auto tabular-nums">{age(item.at_ms)}</span>
          </div>
          <h1 className="text-xl font-semibold leading-snug md:text-2xl">
            {item.task ? (
              <Link to="/tasks/$id" params={{ id: item.task }} className="hover:underline">
                {task.data?.title ?? item.title}
              </Link>
            ) : (
              item.title
            )}
          </h1>
        </div>

        {item.packet !== null && packet.isPending ? (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-5/6" />
            <Skeleton className="h-4 w-2/3" />
          </div>
        ) : p ? (
          p.board ? <PacketBoard board={p.board} summary={p.summary} /> : <PacketMarkdown text={p.summary} />
        ) : (
          <p className="text-sm text-fg-tertiary">No summary from the agent — read the thread.</p>
        )}

        {(task.data?.done_when || (p && p.checks.length > 0)) && (
          <section className="flex flex-col gap-2">
            <SectionTitle>Acceptance</SectionTitle>
            {task.data?.done_when && <p className="text-sm whitespace-pre-wrap">{task.data.done_when}</p>}
            {p && p.checks.length > 0 && (
              <ul className="flex flex-col gap-1.5">
                {p.checks.map((c, i) => (
                  <li key={i} className="flex items-start gap-2 text-sm">
                    {c.done ? <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-success" aria-label="done" /> : <Circle className="mt-0.5 size-4 shrink-0 text-fg-tertiary" aria-label="not done" />}
                    <span className="min-w-0 flex-1">
                      {c.text}
                      {c.note && <span className="block text-xs text-fg-tertiary">{c.note}</span>}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>
        )}

        {p && p.pointers.length > 0 && (
          <section className="flex flex-col gap-2">
            <SectionTitle>Review these</SectionTitle>
            <Pointers list={p.pointers} />
          </section>
        )}

        {moves.length > 0 && (
          <section className="flex flex-col gap-2">
            <SectionTitle>Next move</SectionTitle>
            <OptionCards options={moves} picked={next} recommended={p?.recommended} onPick={(i) => setPicked(next === i ? null : i)} />
          </section>
        )}
      </Body>

      <div className="shrink-0 border-t border-border-subtle bg-background px-4 pt-3 pb-[calc(0.75rem+var(--safe-bottom))] md:px-6">
        <div className="mx-auto flex w-full max-w-[720px] flex-col gap-2">
          <IfWrong text={p?.if_wrong} />
          {sendingBack && (
            <form
              className="flex flex-col gap-2"
              onSubmit={(e) => {
                e.preventDefault()
                if (note.trim()) move('doing', note.trim())
              }}
            >
              <Textarea value={note} onChange={(e) => setNote(e.target.value)} placeholder="What needs to change…" aria-label="Send-back note" rows={3} autoFocus className="text-base md:text-sm" />
              <div className="flex gap-1.5">
                <Button type="submit" size="sm" variant="outline" disabled={busy || !note.trim()}>
                  Send back with note
                </Button>
                <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => setSendingBack(false)}>
                  Cancel
                </Button>
              </div>
            </form>
          )}
          <Footer item={item} onResolved={onResolved}>
            <Button size="sm" disabled={busy || !item.task} onClick={() => move('done', null)}>
              Approve → {nextLabel ?? 'done'}
            </Button>
            {!sendingBack && (
              <Button size="sm" variant="outline" disabled={busy || !item.task} onClick={() => setSendingBack(true)}>
                Send back
              </Button>
            )}
            <PrioritySelect item={item} />
          </Footer>
        </div>
      </div>
    </>
  )
}
