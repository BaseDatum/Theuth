// Roster 67: a desk's inputs are harness text, not the user's words. The opening block (charter, journal, manual)
// is one collapsed card with a section per part; a wake is a card headed by its reason and task chips with the
// task record as its body. Nothing is hidden: every part stays readable, just folded.
// 152: the same for a restart's `<resume_handoff>`, a parent's `<child_lost>` and the endless parent's view.

import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, FlaskConical, Infinity as InfinityIcon, Radio, RotateCcw, Unplug, Zap } from 'lucide-react'
import { Streamdown } from 'streamdown'
import { shortId } from '@/components/tasks/helpers'
import { Badge } from '@/components/ui/badge'
import { fmtBytes } from '@/lib/files'
import type { ResumeSection } from '@/lib/injected'
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
