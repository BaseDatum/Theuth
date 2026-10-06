import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { useAttentionActions } from '@/components/hud/queries'
import { useTask } from '@/components/tasks/queries'
import { age } from '@/components/tasks/helpers'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { PacketBoard } from './PacketBoard'
import { PacketMarkdown } from './PacketMarkdown'
import { Pointers } from './Pointers'
import { usePacket } from './queries'
import { Body, Chip, Footer, SectionTitle, useResolved } from './shared'

/**
 * 101 D8: who asks · the task · age → the question → the agent's board (101b R4; the `summary` when there is none) →
 * "Look at" → option cards (recommended pre-picked) + "Type your own…" → one Answer. A pick alone answers nothing
 * (mis-tap protection); no confirm after Answer. Without a packet (an item raised before this build) the question is
 * the title and the free text still works.
 */
export function DecisionScreen({ item }: { item: AttentionItem }) {
  const packet = usePacket(item.packet)
  const task = useTask(item.task ?? '', item.task !== null)
  const act = useAttentionActions()
  const onResolved = useResolved(item)
  // `undefined` = untouched, so the recommendation applies once the packet lands without an effect.
  const [picked, setPicked] = useState<number | null | undefined>(undefined)
  const [own, setOwn] = useState('')
  const p = packet.data ?? null
  const options = p?.options ?? []
  const choice = picked === undefined ? (p?.recommended ?? null) : picked
  const typed = own.trim()
  const answer = typed || (choice !== null ? (options[choice]?.label ?? '') : '')
  const busy = act.decide.isPending
  const send = () => {
    if (!item.decision || !answer) return
    void act.decide.mutateAsync({ decision: item.decision, answer }).then(onResolved, () => undefined)
  }
  return (
    <Body>
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-tertiary">
          <Chip kind="decision" />
          <span>{item.agent ? `${item.agent} asks` : 'asks'}</span>
          {item.task && (
            <Link to="/tasks/$id" params={{ id: item.task }} className="min-w-0 truncate hover:underline">
              · {task.data?.title ?? item.title}
            </Link>
          )}
          <span className="ml-auto tabular-nums">{age(item.at_ms)}</span>
        </div>
        <h1 className="text-xl font-semibold leading-snug md:text-2xl">{p?.question ?? item.title}</h1>
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
        <p className="text-sm text-fg-tertiary">
          No summary — the question is in the task thread.{' '}
          {item.task && (
            <Link to="/tasks/$id" params={{ id: item.task }} className="underline underline-offset-2">
              Open task
            </Link>
          )}
        </p>
      )}

      {p && p.pointers.length > 0 && (
        <section className="flex flex-col gap-2">
          <SectionTitle>Look at</SectionTitle>
          <Pointers list={p.pointers} />
        </section>
      )}

      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          send()
        }}
      >
        {options.length > 0 && <SectionTitle>Options</SectionTitle>}
        {options.map((c, i) => {
          const on = !typed && choice === i
          return (
            <button
              key={c.label}
              type="button"
              aria-pressed={on}
              onClick={() => {
                setPicked(i)
                setOwn('')
              }}
              className={cn('flex w-full items-start gap-2 rounded-md border px-3 py-2 text-left text-sm transition-colors', on ? 'border-fg-accent bg-accent' : 'border-border-subtle hover:bg-accent/60')}
            >
              <span className="mt-0.5 text-xs text-fg-tertiary">{on ? '●' : '○'}</span>
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex items-center gap-2">
                  {c.label}
                  {p?.recommended === i && (
                    <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
                      recommended
                    </Badge>
                  )}
                </span>
                {c.description && <span className="text-xs text-fg-secondary">{c.description}</span>}
              </span>
            </button>
          )
        })}
        <Input
          value={own}
          onChange={(e) => {
            setOwn(e.target.value)
            if (e.target.value.trim()) setPicked(null)
          }}
          placeholder="Type your own…"
          aria-label="Your own answer"
          className={cn('h-9 text-base md:text-sm', typed && 'border-fg-accent')}
        />
        <Button type="submit" size="sm" className="w-full sm:w-fit" disabled={busy || !answer || !item.decision}>
          Answer
        </Button>
      </form>

      <Footer item={item} onResolved={onResolved} />
    </Body>
  )
}
