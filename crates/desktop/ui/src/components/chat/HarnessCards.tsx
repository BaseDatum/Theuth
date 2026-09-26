// Roster 67: a desk's inputs are harness text, not the user's words. The opening block (charter, journal, manual)
// is one collapsed card with a section per part; a wake is a card headed by its reason and task chips with the
// task record as its body. Nothing is hidden: every part stays readable, just folded.

import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, Zap } from 'lucide-react'
import { Streamdown } from 'streamdown'
import { shortId } from '@/components/tasks/helpers'
import { Badge } from '@/components/ui/badge'
import type { Entry } from '@/store/sessions'

const SHELL = 'my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm'
const SUMMARY = 'flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 [&::-webkit-details-marker]:hidden'

function Section({ title, text }: { title: string; text: string }) {
  return (
    <details className="group/section mt-2 border-t border-border-subtle pt-2">
      <summary className={`${SUMMARY} text-xs font-medium text-fg-secondary`}>
        <ChevronRight className="size-3.5 shrink-0 transition-transform group-open/section:rotate-90" aria-hidden="true" />
        {title}
      </summary>
      <Streamdown mode="static" className="mt-2 text-sm">
        {text}
      </Streamdown>
    </details>
  )
}

export function DeskCard({ agent, charter, journal, manual }: Extract<Entry, { kind: 'desk' }>) {
  return (
    <details className={`group/desk ${SHELL}`}>
      <summary className={`${SUMMARY} text-xs text-fg-secondary`}>
        <Bot className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">desk</span>
        <span className="rounded-full bg-layer-2 px-2 leading-5">{agent}</span>
        <ChevronRight className="ml-auto size-4 shrink-0 transition-transform group-open/desk:rotate-90" aria-hidden="true" />
      </summary>
      <Section title="Charter" text={charter} />
      <Section title="Journal" text={journal} />
      <Section title="Manual" text={manual} />
    </details>
  )
}

export function WakeCard({ id, title, reason, count, state, owner, reviewer, thread, body, roster }: Extract<Entry, { kind: 'wake' }>) {
  return (
    <div className={SHELL}>
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <Zap className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">wake</span>
        <Badge variant="outline">{reason}</Badge>
        {count !== undefined && <Badge variant="outline">×{count}</Badge>}
        {id === undefined ? (
          <span>no task</span>
        ) : (
          <Badge variant="secondary" className="max-w-full gap-1">
            <Link to="/tasks/$id" params={{ id }} className="truncate hover:underline">
              {title ?? 'task'}
            </Link>
            <span className="text-fg-tertiary">{shortId(id)}</span>
          </Badge>
        )}
        {state !== undefined && <Badge variant="secondary">{state}</Badge>}
        {owner !== undefined && <Badge variant="secondary">owner {owner}</Badge>}
        {reviewer !== undefined && <Badge variant="secondary">reviewer {reviewer}</Badge>}
        {thread !== undefined && <Badge variant="secondary">thread {thread}</Badge>}
      </div>
      {body !== undefined && body !== '' && (
        <Streamdown mode="static" className="mt-2 text-sm">
          {body}
        </Streamdown>
      )}
      {roster !== undefined && <Section title="Fit check" text={roster} />}
    </div>
  )
}
