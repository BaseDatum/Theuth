// Roster 67: a desk's inputs are harness text, not the user's words. The opening block (charter, journal, manual)
// is one collapsed card with a section per part; a wake is a card headed by its reason and task chips with the
// task record as its body. Nothing is hidden: every part stays readable, just folded.
// 152: the same for a restart's `<resume_handoff>`, a parent's `<child_lost>` and the endless parent's view.
// 183 §12.4: the same for a move's `<session_moved/>` and a resumed batch's `<engine_restart/>` — fields, never the tag.

import { Link } from '@tanstack/react-router'
import { Activity, ArrowLeftRight, Bot, ChevronRight, Contact, FlaskConical, Hourglass, Infinity as InfinityIcon, Radio, RotateCcw, TriangleAlert, Unplug, Zap } from 'lucide-react'
import { Streamdown } from 'streamdown'
import { shortId } from '@/components/tasks/helpers'
import { Badge } from '@/components/ui/badge'
import { CAP } from '@/lib/contacts'
import { fmtBytes } from '@/lib/files'
import { humanSecs, type MovedNote, type RestartNote, type ResumeSection, tagAttrs } from '@/lib/injected'
import { cn } from '@/lib/utils'
import type { Risk } from '@/rpc/bindings/Risk'
import type { Entry } from '@/store/sessions'
import { markdownComponents, plugins, shikiTheme } from './MessageAssistant'
import { Details, FenceText } from './disclosure'
import { FenceContext } from './surface/JsonRenderFence'

const SHELL = 'my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm'
const SUMMARY = 'flex items-center gap-2 rounded-sm focus-visible:outline-2'
const CODE = 'mt-2 overflow-x-auto rounded-md bg-layer-2 px-3 py-2 font-mono text-xs leading-5 whitespace-pre text-fg-secondary'
/** 152: a card header — chips wrap on their own rows while the chevron stays at the right; the muted line sits below,
 * one line on a phone and two on a desktop (`md` is `useIsMobile`'s 768 px). */
const HEAD = 'flex flex-wrap items-center gap-1.5'
const MUTED = 'mt-1 line-clamp-1 text-fg-tertiary md:line-clamp-2'

/** `code`: the body is machine output (git status / diff), shown verbatim instead of as markdown. */
function Section({ title, text, code }: { title: string; text: string; code?: boolean }) {
  return (
    <Details
      part={`section|${title}`}
      className="group/section mt-2 border-t border-border-subtle pt-2"
      triggerClassName={`${SUMMARY} text-xs font-medium text-fg-secondary`}
      trigger={
        <>
          <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/section:rotate-90" aria-hidden="true" />
          {title}
        </>
      }
    >
      {code ? (
        <pre className={CODE}>{text}</pre>
      ) : (
        <Streamdown mode="static" className="mt-2 text-sm">
          {text}
        </Streamdown>
      )}
    </Details>
  )
}

export function DeskCard({ agent, charter, journal, manual }: Extract<Entry, { kind: 'desk' }>) {
  return (
    <Details
      part="desk"
      className={`group/desk ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <Bot className="size-3.5 shrink-0" aria-hidden="true" />
          <span className="font-medium">desk</span>
          <span className="rounded-full bg-layer-2 px-2 leading-5">{agent}</span>
          <ChevronRight className="ml-auto size-4 shrink-0 transition-transform group-data-[state=open]/desk:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <Section title="Charter" text={charter} />
      <Section title="Journal" text={journal} />
      <Section title="Manual" text={manual} />
    </Details>
  )
}

export function WakeCard({ task, title, reason, count, state, owner, reviewer, thread, body, roster }: Extract<Entry, { kind: 'wake' }>) {
  return (
    <div className={SHELL}>
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <Zap className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">wake</span>
        <Badge variant="outline">{reason}</Badge>
        {count !== undefined && <Badge variant="outline">×{count}</Badge>}
        {task === undefined ? (
          <span>no task</span>
        ) : (
          <Badge variant="secondary" className="max-w-full gap-1">
            <Link to="/tasks/$id" params={{ id: task }} className="truncate hover:underline">
              {title ?? 'task'}
            </Link>
            <span className="text-fg-tertiary">{shortId(task)}</span>
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

/** 62 §0.4: the human's one message sent to `count` sessions at once; the marker id names the broadcast. */
export function BroadcastCard({ id, count, text }: { id: string; count: number; text: string }) {
  return (
    <div className={SHELL}>
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <Radio className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">broadcast</span>
        <span className="text-fg-tertiary">{id}</span>
        <Badge variant="outline">{count} sessions</Badge>
      </div>
      <Streamdown mode="static" className="mt-2 text-sm">
        {text}
      </Streamdown>
    </div>
  )
}

/** The fresh handoff's section title with its count: `Uncommitted work · 12 lines`, `Queued messages · 2`. */
function sectionTitle(s: ResumeSection): string {
  if (s.count === undefined) return s.title
  return s.title === 'Uncommitted work' || s.title === 'Diff' ? `${s.title} · ${s.count} lines` : `${s.title} · ${s.count}`
}

/** 152 D4: `<resume_handoff>` — restored (the harness kept its context) or fresh (a new process got the handoff's sections). */
export function ResumeCard({ restored, clock, working, previous, sections, omitted, text }: Extract<Entry, { kind: 'resume' }>) {
  return (
    <Details
      part="resume"
      className={`group/resume ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <span className="min-w-0 flex-1">
            <span className={HEAD}>
              <RotateCcw className={`size-3.5 shrink-0 ${restored ? '' : 'text-warning'}`} aria-hidden="true" />
              <span className="font-medium">{restored ? 'resumed after restart' : 'restarted fresh'}</span>
              {clock !== '' && <Badge variant="outline">{clock}</Badge>}
            </span>
            {restored ? (
              working !== undefined && <span className={MUTED}>Working on: {working}</span>
            ) : (
              <span className={MUTED}>previous harness session{previous === undefined ? '' : ` ${shortId(previous)}`} could not be loaded — context not restored</span>
            )}
          </span>
          <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]/resume:rotate-90" aria-hidden="true" />
        </>
      }
    >
      {restored ? (
        <Streamdown mode="static" className="mt-2 border-t border-border-subtle pt-2 text-sm">
          {text}
        </Streamdown>
      ) : (
        <>
          {sections.map((s) => (
            <Section key={s.title} title={sectionTitle(s)} text={s.text} code={s.title === 'Uncommitted work' || s.title === 'Diff'} />
          ))}
          {omitted !== undefined && <p className="mt-2 text-xs text-fg-tertiary">omitted for the handoff budget: {omitted.join(', ')}</p>}
        </>
      )}
    </Details>
  )
}

/** 152 D4: `<child_lost>` in a parent — a subagent the restart could not respawn; the chip opens the child's session
 * (its click does not also toggle the card). */
export function ChildLostCard({ name, session, clock, why, text }: Extract<Entry, { kind: 'lost' }>) {
  return (
    <Details
      part="lost"
      className={`group/lost ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <span className="min-w-0 flex-1">
            <span className={HEAD}>
              <Unplug className="size-3.5 shrink-0 text-danger" aria-hidden="true" />
              <span className="font-medium">subagent lost</span>
              <Badge variant="secondary" className="max-w-full gap-1">
                {session === '' ? (
                  <span className="truncate">{name}</span>
                ) : (
                  <>
                    <Link to="/c/$session" params={{ session }} className="truncate hover:underline" onClick={(e) => e.stopPropagation()}>
                      {name}
                    </Link>
                    <span className="text-fg-tertiary">{shortId(session)}</span>
                  </>
                )}
              </Badge>
              {clock !== '' && <Badge variant="outline">{clock}</Badge>}
            </span>
            {why !== '' && <span className={MUTED}>{why}</span>}
          </span>
          <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]/lost:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <Streamdown mode="static" className="mt-2 border-t border-border-subtle pt-2 text-sm">
        {text}
      </Streamdown>
    </Details>
  )
}

/** 155 §4.1 phases and §5.3 states as the card says them. */
const CHILD_WORDS: Record<string, string> = { no_first_update: 'no first update', silent: 'gone quiet', not_running: 'not running' }
const childWord = (word: string) => CHILD_WORDS[word] ?? word.replaceAll('_', ' ')
const childSecs = ({ event, secs }: Pick<Extract<Entry, { kind: 'child' }>, 'event' | 'secs'>) => (secs === '' ? '' : `${event === 'stalled' ? 'quiet' : 'waited'} ${humanSecs(secs)}`)

/** `w171f4-e · no first update · quiet 1 min · 15:03`: the card's one line (and its label). */
export function childLine(c: Extract<Entry, { kind: 'child' }>): string {
  return [c.child, childWord(c.word), childSecs(c), c.clock].filter(Boolean).join(' · ')
}

/** 155 §4.1 `<child_stalled>` / §5.3 `<peer_state>` in a parent: one line — the child (its chip opens the session, as
 * on `ChildLostCard`), the phase or state, the seconds, the time — with the child's last activity below; the guidance
 * sentence is the folded body. */
export function ChildStateCard(c: Extract<Entry, { kind: 'child' }>) {
  const Icon = c.event === 'stalled' ? Hourglass : Activity
  const secs = childSecs(c)
  return (
    <Details
      part="child"
      className={`group/child ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <span className="min-w-0 flex-1">
            <span className={HEAD}>
              <Icon className={`size-3.5 shrink-0 ${c.event === 'stalled' ? 'text-warning' : ''}`} aria-hidden="true" />
              <Badge variant="secondary" className="max-w-full gap-1">
                {c.session === '' ? (
                  <span className="truncate">{c.child}</span>
                ) : (
                  <Link to="/c/$session" params={{ session: c.session }} className="truncate hover:underline" onClick={(e) => e.stopPropagation()}>
                    {c.child}
                  </Link>
                )}
              </Badge>
              <span className="font-medium">{childWord(c.word)}</span>
              {secs !== '' && <Badge variant="outline">{secs}</Badge>}
              {c.clock !== '' && <Badge variant="outline">{c.clock}</Badge>}
            </span>
            {c.last !== undefined && <span className={`${MUTED} font-mono`}>last: {c.last}</span>}
          </span>
          <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]/child:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <Streamdown mode="static" className="mt-2 border-t border-border-subtle pt-2 text-sm">
        {c.text}
      </Streamdown>
    </Details>
  )
}

/** 152 D4: the view an endless parent handed a native child (137 R1-D1). The lines mount only while open, inside their
 * own scroll box, so a 128 KB view costs nothing folded and never shifts the transcript (135 STEADY-SCROLL). */
export function ParentViewCard({ lines, bytes }: Extract<Entry, { kind: 'parentView' }>) {
  return (
    <Details
      part="view"
      className={`group/view ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <span className="min-w-0 flex-1">
            <span className={HEAD}>
              <InfinityIcon className="size-3.5 shrink-0" aria-hidden="true" />
              <span className="font-medium">parent's memory</span>
              <Badge variant="outline">{lines.length} lines</Badge>
              <Badge variant="outline">≈ {fmtBytes(bytes)}</Badge>
            </span>
            <span className={MUTED}>What the parent knew when it handed this task over.</span>
          </span>
          <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]/view:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <div className="mt-2 max-h-80 overflow-y-auto border-t border-border-subtle pt-2">
        {lines.map((l) => (
          <div key={`${l.id}+${l.n}`} className={`flex gap-2 px-2 py-1 text-sm leading-5 ${l.earlier ? 'text-xs text-fg-tertiary italic' : ''}`} role={l.earlier ? 'note' : undefined}>
            <span className="w-16 shrink-0 text-right font-mono text-[11px] leading-5 text-fg-tertiary not-italic tabular-nums">
              {l.id}+{l.n}
            </span>
            <span className="min-w-0 flex-1 wrap-anywhere">{l.text}</span>
          </div>
        ))}
      </div>
    </Details>
  )
}

/** A settled card: a broken fence shows its one dim line, never a skeleton. */
const SETTLED: FenceContext = { running: false, hasLaterAssistant: false }

/**
 * 151 R1 (D7): `<autoresearch slug event>` — the study's start / result / failure card the engine delivered into its
 * setup chat (no messaging channel renders a desktop session). The body is the card markdown (a `json-render` fence or
 * one line) through the chat's fence renderers; the header links to the study page.
 */
export function AutoresearchCard({ slug, event, text }: Extract<Entry, { kind: 'autoresearch' }>) {
  return (
    <div className={SHELL}>
      <div className={`${HEAD} text-xs text-fg-secondary`}>
        <FlaskConical className={`size-3.5 shrink-0 ${event === 'failed' ? 'text-danger' : ''}`} aria-hidden="true" />
        <span className="font-medium">Autoresearch · {slug}</span>
        {event !== '' && <Badge variant={event === 'failed' ? 'destructive' : 'outline'}>{event}</Badge>}
        <Link to="/autoresearch/$slug" params={{ slug }} className="ml-auto inline-flex items-center gap-0.5 hover:underline">
          Open study <ChevronRight className="size-3" aria-hidden="true" />
        </Link>
      </div>
      <FenceContext value={SETTLED}>
        <FenceText value={text}>
          <Streamdown mode="static" plugins={plugins} components={markdownComponents} shikiTheme={shikiTheme} linkSafety={{ enabled: false }} className="mt-2 text-sm [&_p]:my-1">
            {text}
          </Streamdown>
        </FenceText>
      </FenceContext>
    </div>
  )
}

/** `/home/sky/x` and `/Users/sky/x` as `~/x`: the card is read, not copied. */
export const tilde = (path: string): string => path.replace(/^\/(?:home|Users)\/[^/]+(?=\/|$)/, '~')

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`

/** Mock panel 4's title: `Session moved to xeus · Arch Linux` (the cwd follows as code). */
export const movedTitle = (m: MovedNote): string => [`Session moved to ${m.to || 'another machine'}`, m.os].filter(Boolean).join(' · ')

/** Mock panel 4's subline: `from Sky's MacBook · 11:42 · Python runtime reset · 1 job left on Sky's MacBook · approvals kept: Ask`. */
export function movedLine(m: MovedNote): string {
  return [
    m.from && `from ${m.from}`,
    m.clock.slice(0, 5),
    m.runtime === 'reset' && 'Python runtime reset',
    m.jobsLeft > 0 && `${plural(m.jobsLeft, 'job')} left on ${m.from || 'the old machine'}`,
    m.approvals && `approvals kept: ${m.approvals === 'ask' ? 'Ask' : m.approvals === 'off' ? 'Off' : m.approvals}`,
    !m.messaging && 'agent messaging off',
    m.pending && 'the open turn waits for Resume',
  ]
    .filter(Boolean)
    .join(' · ')
}

/** The collapsed body both 183 cards share: the note the model read, as its fields (never the raw tag). */
function Told({ text }: { text: string }) {
  return (
    <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 border-t border-border-subtle pt-2 text-xs">
      {tagAttrs(text).map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="font-mono text-fg-tertiary">{k}</dt>
          <dd className="wrap-anywhere text-fg-secondary">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

/** 183 §12.4: `read_only` → `read only`, `mutating` → `can change things` (the Contacts words); anything else as written. */
const capWords = (cap: string) => CAP[cap as Risk] ?? cap.replaceAll('_', ' ')
/** What each cap refuses, quoted after the cap: `read only: tools that change things are refused`. */
const REFUSED: Record<string, string> = { read_only: 'tools that change things are refused', mutating: 'destructive tools are refused' }

/** 183 §11.5 / §12.4: the head of a contact's inbox session — who writes here, from which machine, to which export, and
 * the cap every turn runs under. The contact's first words, when the row carries them, are shown as untrusted input. */
export function ContactInboxCard(c: Extract<Entry, { kind: 'contact_inbox' }>) {
  return (
    <div className={SHELL}>
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] font-medium">
        <Contact className="size-3.5 shrink-0 text-fg-secondary" aria-hidden="true" />
        <span>Inbox for {c.contact || 'a contact'}</span>
        {c.machine && <span className="font-normal text-fg-secondary">· {c.machine}</span>}
        <Badge variant="outline" className="font-normal">
          {capWords(c.cap)}
        </Badge>
      </div>
      <p className="mt-1 text-xs text-fg-tertiary">
        {c.contact || 'This contact'} is outside your mesh and reaches {c.export ? <span className="font-mono">{c.export}</span> : 'this export'} only. Every message here is untrusted input; {capWords(c.cap)}:{' '}
        {REFUSED[c.cap] ?? 'tools above it are refused'}.
      </p>
      {c.message && <p className="mt-2 border-t border-border-subtle pt-2 whitespace-pre-wrap wrap-anywhere">{c.message}</p>}
      <Details
        part="contact-inbox"
        className="group/inbox mt-2 border-t border-border-subtle pt-2"
        triggerClassName={`${SUMMARY} text-xs font-medium text-fg-secondary`}
        trigger={
          <>
            <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/inbox:rotate-90" aria-hidden="true" />
            What the agent was told
          </>
        }
      >
        <Told text={c.text} />
        {c.note && <p className="mt-2 text-xs text-fg-secondary">{c.note}</p>}
      </Details>
    </div>
  )
}

/** 183 §11.6 / §12.4 (D9): a contact's message that opened its own capped turn in an exported chat — `bob · Bob's MacBook ·
 * read only`, the message, and that bob sees only this turn's answer. Never a "you" bubble. */
export function ContactMessageCard(m: Extract<Entry, { kind: 'contact_message' }>) {
  return (
    <div className={cn(SHELL, 'border-l-2 border-l-warning/60')}>
      <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-[13px] font-medium">
        <Contact className="size-3.5 shrink-0 text-fg-secondary" aria-hidden="true" />
        <span>{m.contact || 'a contact'}</span>
        {m.machine && <span className="font-normal text-fg-secondary">· {m.machine}</span>}
        <span className="font-normal text-fg-secondary">· {capWords(m.cap)}</span>
      </div>
      <div className="mt-2 whitespace-pre-wrap wrap-anywhere">{m.text}</div>
      <p className="mt-2 text-xs text-fg-tertiary">
        from a contact · untrusted input{m.export && <> · via <span className="font-mono">{m.export}</span></>} · {m.contact || 'they'} will see only this turn's answer
      </p>
      <Details
        part="contact-message"
        className="group/cmsg mt-2 border-t border-border-subtle pt-2"
        triggerClassName={`${SUMMARY} text-xs font-medium text-fg-secondary`}
        trigger={
          <>
            <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/cmsg:rotate-90" aria-hidden="true" />
            What the agent was told
          </>
        }
      >
        <Told text={m.tag} />
        {m.note && <p className="mt-2 text-xs text-fg-secondary">{m.note}</p>}
      </Details>
    </div>
  )
}

/** 183 §12.4 / mock panel 4: one move or take, from `messages.moved` (or a `<session_moved/>` input). A cwd missing on
 * the taker (W-9) says where it works instead; "What the agent was told" folds the note's fields. */
export function MovedCard(m: Extract<Entry, { kind: 'moved' }>) {
  return (
    <div className={SHELL}>
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] font-medium">
        <ArrowLeftRight className="size-3.5 shrink-0 text-fg-secondary" aria-hidden="true" />
        <span className="min-w-0">{movedTitle(m)}</span>
        {m.cwd && (
          <>
            <span aria-hidden="true">·</span>
            <code className="min-w-0 truncate font-mono text-xs">{tilde(m.cwd)}</code>
          </>
        )}
      </div>
      <p className="mt-1 text-xs text-fg-tertiary">{movedLine(m)}</p>
      {m.cwdMissing && (
        <p className="mt-1 flex items-start gap-1.5 text-xs text-warning">
          <TriangleAlert className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
          {tilde(m.cwdWas ?? 'the folder')} does not exist on {m.to} — working in {tilde(m.cwd)}
        </p>
      )}
      <Details
        part="moved"
        className="group/moved mt-2 border-t border-border-subtle pt-2"
        triggerClassName={`${SUMMARY} text-xs font-medium text-fg-secondary`}
        trigger={
          <>
            <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/moved:rotate-90" aria-hidden="true" />
            What the agent was told
          </>
        }
      >
        <Told text={m.text} />
      </Details>
    </div>
  )
}

const CAUSE: Record<string, string> = { crash: 'the engine crashed', shutdown: 'the engine shut down', update: 'the engine updated' }

/** §12.4: `Resumed on xeus after 4 s · moved from Sky's MacBook · 1 call interrupted`; a boot restart names its cause. */
export function restartLine(r: RestartNote): string {
  const moved = r.cause === 'transfer' || r.cause === 'takeover'
  return [
    moved ? `Resumed on ${r.to ?? 'this machine'} after ${humanSecs(r.after)}` : `Resumed after ${humanSecs(r.after)}`,
    moved ? r.from && `${r.cause === 'takeover' ? 'taken over' : 'moved'} from ${r.from}` : (CAUSE[r.cause] ?? (r.cause && `${r.cause} restart`)),
    r.replayed > 0 && `${plural(r.replayed, 'call')} replayed`,
    r.interrupted > 0 && `${plural(r.interrupted, 'call')} interrupted`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** 183 §12.4: the `<engine_restart/>` a resumed batch's first result opened with (`splitRestart`), above that tool card. */
export function EngineRestartCard(r: RestartNote & { text: string }) {
  return (
    <Details
      part="restart"
      className={`group/restart ${SHELL}`}
      triggerClassName={`${SUMMARY} text-xs text-fg-secondary`}
      trigger={
        <>
          <RotateCcw className="size-3.5 shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1 font-medium">{restartLine(r)}</span>
          <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]/restart:rotate-90" aria-hidden="true" />
        </>
      }
    >
      <p className="mt-2 text-xs text-fg-tertiary">What the agent was told</p>
      <Told text={r.text} />
    </Details>
  )
}
