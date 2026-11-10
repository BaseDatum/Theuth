import { Link } from '@tanstack/react-router'
import { AppWindow, BellRing, Bot, ChevronRight, Trash2 } from 'lucide-react'
import { memo, type ReactNode } from 'react'
import { type AppInput, parseAppInput } from '@/components/apps/lifecycle'
import { ENGINE_NOTICES, parseBroadcast } from '@/lib/injected'
import { parseMeetingLine } from '@/lib/meeting-line'
import type { AgentInfo } from '@/rpc/extra'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { AdvisorNoteCard } from './AdvisorNoteCard'
import { ApprovalCard } from './ApprovalCard'
import { AskCard } from './AskCard'
import { AutoresearchCard, BroadcastCard, ChildLostCard, ChildStateCard, ContactInboxCard, ContactMessageCard, DeskCard, EngineRestartCard, MovedCard, ParentViewCard, ResumeCard, WakeCard, childLine } from './HarnessCards'
import { MessageAssistant, RemoveButton } from './MessageAssistant'
import { MessageCompacted } from './MessageCompacted'
import { MessageError, MessageInfo, MessageInterrupted } from './MessageInfo'
import { MessagePeer } from './MessagePeer'
import { MessageReasoning } from './MessageReasoning'
import { MessageTool } from './MessageTool'
import { MeetingCard } from './MeetingCard'
import { MessageUser } from './MessageUser'
import { MessageVision } from './MessageVision'
import { SecretRequestCard } from './SecretRequestCard'
import { Details, DisclosureScope, useScopedDisclosure } from './disclosure'
import { ResultCard } from '@/components/cells/ResultCard'

/** `readOnly`: an archive/inspection rendering (search's Earlier messages page): no turn, fork or context-removal controls. */
function body(entry: Entry, session: string, readOnly?: boolean) {
  switch (entry.kind) {
    case 'user': {
      // 53 §4.4: the daemon's `[meeting …]` inputs are notices, not the user's words.
      const meeting = parseMeetingLine(entry.text)
      if (meeting) return <MeetingCard line={meeting} />
      // 62 §0.4: a broadcast is the human's words to many sessions; the marker line becomes the card's header.
      const broadcast = parseBroadcast(entry.text)
      if (broadcast) return <BroadcastCard {...broadcast} />
      // 46 §6.3: an app's `@ToAssistant` action is app data with provenance, not a `you` bubble.
      const app = parseAppInput(entry.text)
      if (app) return <AppInputCard input={app} />
      return <MessageUser id={entry.id} text={entry.text} skill={entry.skill} files={entry.files} cells={entry.cells} session={session} turn={entry.turn} context={entry.context} channel={entry.channel} at={entry.at} readOnly={readOnly} />
    }
    case 'assistant':
      return <MessageAssistant id={entry.id} text={entry.text} open={entry.open} session={session} turn={entry.turn} context={entry.context} at={entry.at} readOnly={readOnly} />
    case 'reasoning':
      return <MessageReasoning id={entry.id} text={entry.text} done={entry.done} ms={entry.ms} />
    case 'tool':
      // 183 §12.4: a resumed batch's restart note is its own card above the call, never raw XML in the result.
      return entry.restart ? (
        <>
          <EngineRestartCard {...entry.restart} />
          <MessageTool tool={entry} session={session} readOnly={readOnly} />
        </>
      ) : (
        <MessageTool tool={entry} session={session} readOnly={readOnly} />
      )
    case 'peer':
      return <MessagePeer from={entry.from} text={entry.text} origin={entry.origin} seq={entry.seq} session={session} />
    case 'engine':
      return <EngineNotice notice={entry.notice} text={entry.text} />
    case 'desk':
      return <DeskCard {...entry} />
    case 'wake':
      return <WakeCard {...entry} />
    case 'resume':
      return <ResumeCard {...entry} />
    case 'lost':
      return <ChildLostCard {...entry} />
    case 'child':
      return <ChildStateCard {...entry} />
    case 'autoresearch':
      return <AutoresearchCard {...entry} />
    case 'moved':
      return <MovedCard {...entry} />
    case 'engine_restart':
      return <EngineRestartCard {...entry} />
    case 'contact_inbox':
      return <ContactInboxCard {...entry} />
    case 'contact_message':
      return <ContactMessageCard {...entry} />
    case 'cell_result': // 171:U1-ADD
      return <ResultCard run={entry.run} name={entry.name} label={entry.label} cell={entry.cell} exit={entry.exit} secs={entry.secs} />
    case 'parentView':
      return <ParentViewCard {...entry} />
    case 'vision':
      return <MessageVision entry={entry} />
    case 'info':
      return <MessageInfo text={entry.text} />
    case 'error':
      return <MessageError text={entry.text} />
    case 'compacted':
      return <MessageCompacted epoch={entry.epoch} style={entry.style} summary={entry.summary} />
    case 'interrupted':
      return <MessageInterrupted text={entry.text} />
    case 'advisory':
      return <AdvisorNoteCard notes={entry.notes} at={entry.at} question={entry.question} />
  }
}

/** An engine notice, not a user request or a model tool call. Its body stays literal. */
function EngineNotice({ notice, text }: Pick<Extract<Entry, { kind: 'engine' }>, 'notice' | 'text'>) {
  const { title, description } = ENGINE_NOTICES[notice]
  return (
    <Details
      part="notice"
      className="group/notice my-2 min-w-0 rounded-xl border border-info-border/30 bg-info-bg/40 px-4 py-3 text-sm"
      triggerClassName="flex min-h-11 items-center gap-3 rounded-sm focus-visible:outline-2"
      trigger={
        <>
          <BellRing className="size-4 shrink-0 text-info" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <span className="block font-medium text-info">{title}</span>
            <span className="block text-xs text-fg-secondary">{description}</span>
          </span>
          <ChevronRight className="size-4 shrink-0 text-info transition-transform group-data-[state=open]/notice:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <div className="mt-3 border-t border-info-border/20 pt-3 leading-6 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{text}</div>
    </Details>
  )
}

/** An app's `@ToAssistant` input: provenance chips (app, version, action), the message, then the context and form values as sent. */
function AppInputCard({ input }: { input: AppInput }) {
  const sections: [string, string][] = []
  if (input.context !== undefined) sections.push(['context', input.context])
  if (input.form) sections.push([input.form.name ? `form ${input.form.name}` : 'form', input.form.values])
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <div className="flex flex-wrap items-center gap-2 text-xs text-fg-secondary">
        <AppWindow className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">app</span>
        <span className="rounded-full bg-layer-2 px-2 leading-5" title={input.record}>
          {input.name} v{input.version}
        </span>
        {input.action !== undefined && (
          <span className="rounded-full bg-layer-2 px-2 font-mono leading-5" title={input.action}>
            action {input.action.slice(0, 8)}
          </span>
        )}
      </div>
      <div className="mt-2 leading-6 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{input.message}</div>
      {sections.map(([label, text]) => (
        <div key={label} className="mt-2">
          <div className="text-xs text-fg-tertiary">{label}</div>
          <pre className="mt-1 overflow-x-auto rounded-md bg-layer-2 px-3 py-2 font-mono text-xs leading-5 text-fg-secondary">{text}</pre>
        </div>
      ))}
    </div>
  )
}

/** What the tombstone names. */
function describe(entry: Entry): string {
  switch (entry.kind) {
    case 'user': {
      const app = parseAppInput(entry.text)
      return app ? `app ${app.name}` : 'your message'
    }
    case 'assistant':
      return 'reply'
    case 'reasoning':
      return 'reasoning'
    case 'tool':
      return `tool ${entry.name}`
    case 'peer':
      return `message from ${entry.from}`
    case 'engine':
      return ENGINE_NOTICES[entry.notice].title;
    case 'desk':
      return `desk ${entry.agent}`
    case 'wake':
      return `wake · ${entry.reason}`
    case 'resume':
      return entry.restored ? 'resumed after restart' : 'restarted fresh'
    case 'lost':
      return `subagent lost · ${entry.name}`
    case 'child':
      return childLine(entry)
    case 'autoresearch':
      return `Autoresearch · ${entry.slug} · ${entry.event}`
    case 'parentView':
      return "parent's memory"
    case 'compacted':
      return 'summary'
    case 'vision':
      return `vision observation · ${entry.model}`
    case 'advisory':
      return entry.question === undefined ? 'advisor note' : 'consultation'
    default:
      return 'message'
  }
}

/** Collapsed `Removed from context` marker (48 §2.2). `View original` shows the archived card: its text and files stay
 * readable, its turn/context controls are gone, and nothing re-enters the model's context. `reveal` starts it open (a search target). */
function Tombstone({ entry, session, readOnly, reveal }: { entry: Entry; session: string; readOnly?: boolean; reveal?: boolean }) {
  const [open, setOpen] = useScopedDisclosure('tombstone', !!reveal)
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

/** Tool, peer and engine cards keep their own layout (`show`/scrape cards included); the remove control sits over the card's
 * top-right corner as a sibling, with the card padded so no text hides under it. Cards without stored membership
 * (live, unsaved rows) get no control: a button without an authoritative target would only be guessing. The wrapper is
 * the same element either way, so the settled reload adding `context` never remounts the card (171F B13). */
function Card({ entry, session, children }: { entry: Entry; session: string; children: ReactNode }) {
  const ctx = entry.context
  return (
    <div className={ctx ? 'group relative [&>:first-child]:pr-12' : 'group relative'}>
      {children}
      {ctx && (
        <RemoveButton
          session={session}
          target={ctx.target}
          active={ctx.active}
          className="absolute top-1 right-1 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
        />
      )}
    </div>
  )
}

/** An approval, an `ask_user` or a `secret_request` card. `from`: a subagent's prompt shown in an ancestor's transcript,
 * headed by the child's name and a link to its session; answering here or there resolves the same request. */
export function PromptCard({ prompt, from }: { prompt: Prompt; from?: AgentInfo }) {
  const card =
    prompt.type === 'ApprovalRequested' ? (
      <ApprovalCard approval={prompt} />
    ) : prompt.type === 'AskRequested' ? (
      <AskCard ask={prompt} />
    ) : (
      <SecretRequestCard id={prompt.id} note={prompt.note} fields={[{ name: prompt.name, label: prompt.name, masked: true, current: prompt.current }]} />
    )
  if (!from) return card
  return (
    <div className="mt-2 [&>*:last-child]:mt-0">
      <div className="flex items-center gap-2 text-xs text-fg-secondary">
        <Bot className="size-3.5 shrink-0" aria-hidden="true" />
        <span>
          subagent <span className="font-medium text-fg">{from.name}</span> is waiting on you
        </span>
        <Link to="/c/$session" params={{ session: from.id }} className="text-fg-accent hover:underline">
          open
        </Link>
      </div>
      {card}
    </div>
  )
}

/** One transcript row: the entry's card plus the prompts waiting on it. Entries are immutable and `session` is stable, so `memo` skips untouched rows.
 * A row removed from context (48 §2.2) collapses to a tombstone; user/assistant cards own their remove action, tool/peer/engine cards get it from `Card`.
 * `readOnly` drops every control that would edit the session or its context; `reveal` opens a removed row's archive at once.
 * Every disclosure inside is scoped by the entry id (`DisclosureScope`, 171F B13). */
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
  // 183 §5.3: a move during an open turn writes its `moved` row outside the context (the model reads `<engine_restart/>`);
  // it is still the card that says where the chat went, never a "Removed from context" tombstone.
  const card =
    entry.context?.removed && entry.kind !== 'moved' ? (
      <Tombstone entry={entry} session={session} readOnly={readOnly} reveal={reveal} />
    ) : !readOnly && (entry.kind === 'tool' || entry.kind === 'peer' || entry.kind === 'engine' || entry.kind === 'vision' || entry.kind === 'desk' || entry.kind === 'wake' || entry.kind === 'advisory' || entry.kind === 'resume' || entry.kind === 'lost' || entry.kind === 'child' || entry.kind === 'autoresearch' || entry.kind === 'parentView') ? (
      <Card entry={entry} session={session}>
        {body(entry, session)}
      </Card>
    ) : (
      body(entry, session, readOnly)
    )
  return (
    <DisclosureScope value={entry.id}>
      {card}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </DisclosureScope>
  )
})
