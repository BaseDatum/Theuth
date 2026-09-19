import { BellRing, ChevronRight, Trash2 } from 'lucide-react'
import { memo, type ReactNode, useState } from 'react'
import { HARNESS_NOTICES } from '@/lib/injected'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { ApprovalCard } from './ApprovalCard'
import { AskCard } from './AskCard'
import { MessageAssistant, RemoveButton } from './MessageAssistant'
import { MessageCompacted } from './MessageCompacted'
import { MessageError, MessageInfo } from './MessageInfo'
import { MessagePeer } from './MessagePeer'
import { MessageReasoning } from './MessageReasoning'
import { MessageTool } from './MessageTool'
import { MessageUser } from './MessageUser'
import { MessageVision } from './MessageVision'
import { SecretRequestCard } from './SecretRequestCard'

/** `readOnly`: an archive/inspection rendering (search's Earlier messages page): no turn, fork or context-removal controls. */
function body(entry: Entry, session: string, readOnly?: boolean) {
  switch (entry.kind) {
    case 'user':
      return <MessageUser text={entry.text} skill={entry.skill} files={entry.files} session={session} turn={entry.turn} context={entry.context} readOnly={readOnly} />
    case 'assistant':
      return <MessageAssistant id={entry.id} text={entry.text} open={entry.open} session={session} turn={entry.turn} context={entry.context} readOnly={readOnly} />
    case 'reasoning':
      return <MessageReasoning text={entry.text} done={entry.done} />
    case 'tool':
      return <MessageTool tool={entry} session={session} readOnly={readOnly} />
    case 'peer':
      return <MessagePeer from={entry.from} text={entry.text} origin={entry.origin} seq={entry.seq} session={session} />
    case 'harness':
      return <HarnessNotice notice={entry.notice} text={entry.text} />
    case 'vision':
      return <MessageVision entry={entry} />
    case 'info':
      return <MessageInfo text={entry.text} />
    case 'error':
      return <MessageError text={entry.text} />
    case 'compacted':
      return <MessageCompacted epoch={entry.epoch} style={entry.style} summary={entry.summary} />
  }
}

/** A harness notice, not a user request or a model tool call. Its body stays literal. */
function HarnessNotice({ notice, text }: Pick<Extract<Entry, { kind: 'harness' }>, 'notice' | 'text'>) {
  const { title, description } = HARNESS_NOTICES[notice]
  return (
    <details className="group/notice my-2 min-w-0 rounded-xl border border-info-border/30 bg-info-bg/40 px-4 py-3 text-sm">
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 rounded-sm focus-visible:outline-2 [&::-webkit-details-marker]:hidden">
        <BellRing className="size-4 shrink-0 text-info" aria-hidden="true" />
        <span className="min-w-0 flex-1">
          <span className="block font-medium text-info">{title}</span>
          <span className="block text-xs text-fg-secondary">{description}</span>
        </span>
        <ChevronRight className="size-4 shrink-0 text-info transition-transform group-open/notice:rotate-90" aria-hidden="true" />
      </summary>
      <div className="mt-3 border-t border-info-border/20 pt-3 leading-6 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{text}</div>
    </details>
  )
}

/** What the tombstone names. */
function describe(entry: Entry): string {
  switch (entry.kind) {
    case 'user':
      return 'your message'
    case 'assistant':
      return 'reply'
    case 'reasoning':
      return 'reasoning'
    case 'tool':
      return `tool ${entry.name}`
    case 'peer':
      return `message from ${entry.from}`
    case 'harness':
      return HARNESS_NOTICES[entry.notice].title;
    case 'compacted':
      return 'summary'
    case 'vision':
      return `vision observation · ${entry.model}`
    default:
      return 'message'
  }
}

/** Collapsed `Removed from context` marker (48 §2.2). `View original` shows the archived card: its text and files stay
 * readable, its turn/context controls are gone, and nothing re-enters the model's context. `reveal` starts it open (a search target). */
function Tombstone({ entry, session, readOnly, reveal }: { entry: Entry; session: string; readOnly?: boolean; reveal?: boolean }) {
  const [open, setOpen] = useState(!!reveal)
  return (
    <div className="my-1 flex flex-col text-xs text-fg-tertiary">
      <div className="flex items-center gap-2">
        <Trash2 className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="truncate">
          <span className="italic">Removed from context</span> · {describe(entry)}
        </span>
        <button
          type="button"
          className="min-h-11 shrink-0 px-2 underline underline-offset-2 hover:text-fg-secondary"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          {open ? 'Hide original' : 'View original'}
        </button>
      </div>
      {open && (
        <div className="opacity-70" aria-label="Archived original; not part of the model's context">
          {body(entry, session, readOnly)}
        </div>
      )}
    </div>
  )
}

/** Tool, peer and harness cards keep their own layout (`show`/scrape cards included); the remove control sits over the card's
 * top-right corner as a sibling, with the card padded so no text hides under it. Cards without stored membership
 * (live, unsaved rows) get nothing: a button without an authoritative target would only be guessing. */
function Card({ entry, session, children }: { entry: Entry; session: string; children: ReactNode }) {
  const ctx = entry.context
  if (!ctx) return children
  return (
    <div className="group relative [&>:first-child]:pr-12">
      {children}
      <RemoveButton
        session={session}
        target={ctx.target}
        active={ctx.active}
        className="absolute top-1 right-1 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
      />
    </div>
  )
}

/** An approval, an `ask_user` or a `secret_request` card. */
export function PromptCard({ prompt }: { prompt: Prompt }) {
  if (prompt.type === 'ApprovalRequested') return <ApprovalCard approval={prompt} />
  if (prompt.type === 'AskRequested') return <AskCard ask={prompt} />
  return <SecretRequestCard id={prompt.id} name={prompt.name} note={prompt.note} current={prompt.current} />
}

/** One transcript row: the entry's card plus the prompts waiting on it. Entries are immutable and `session` is stable, so `memo` skips untouched rows.
 * A row removed from context (48 §2.2) collapses to a tombstone; user/assistant cards own their remove action, tool/peer/harness cards get it from `Card`.
 * `readOnly` drops every control that would edit the session or its context; `reveal` opens a removed row's archive at once. */
export const Message = memo(function Message({
  entry,
  prompts,
  session,
  readOnly,
  reveal,
}: {
  entry: Entry
  prompts: Prompt[] | null
  session: string
  readOnly?: boolean
  reveal?: boolean
}) {
  const card =
    entry.context?.removed ? (
      <Tombstone entry={entry} session={session} readOnly={readOnly} reveal={reveal} />
    ) : !readOnly && (entry.kind === 'tool' || entry.kind === 'peer' || entry.kind === 'harness' || entry.kind === 'vision') ? (
      <Card entry={entry} session={session}>
        {body(entry, session)}
      </Card>
    ) : (
      body(entry, session, readOnly)
    )
  return (
    <>
      {card}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </>
  )
})
