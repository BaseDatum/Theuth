import { Link } from '@tanstack/react-router'
import { type ReactNode, useMemo } from 'react'
import { notify as toast } from '@/lib/notify'
import { dotLabel } from '@/components/agents/sessions'
import { usePayloadDrag, useSessionDrag } from '@/components/chat/Panes'
import { isMobileLayout } from '@/hooks/use-mobile'
import { canPopout, openPopout } from '@/lib/popout'
import { backgroundColors } from '@/lib/background-colors'
import { useTileHost } from '@/components/tiles/host'
import { openJobTerminalTile, showArtifact } from '@/components/tiles/open'
import { targetAction } from './BrowserTargets'
import type { Tray } from './TrayItems'
import { artifactDrag, browserDrag, jobsDrag, type TrayDrag, terminalDrag, vmDrag, whiteboardDrag } from './tray-drag'
import { STRIP_CAP, stripRows } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { AgentInfo } from '@/store/ui-events'
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'
import { kernelName, phaseText, useSessionKernels } from '@/components/cells/KernelStrip'

/** Each dash keeps a full-height hit target; the visible dash is the bar's 5px. */
/** 24 px tall to tap, 16 px of layout (`-my-1`): wrapped rows stack 12 px apart, the hit areas overlapping a little. */
export const DASH_HIT = 'group/dash -my-1 flex h-6 shrink-0 items-center rounded px-px outline-none focus-visible:ring-2 focus-visible:ring-ring'

/** The subagent dashes, right-aligned against the grabber's left end; the "+N" sits at the outer (left) end. */
export function SubagentDashes({ session }: { session: string }) {
  const children = useSessionField(session, 'children')
  const { shown, more } = useMemo(() => stripRows(children ?? {}), [children])
  if (!shown.length) return null
  return (
    // `wrap-reverse`: the dashes that fit stay on the bottom row (nearest the composer), the rest stack above in order.
    <div className="flex min-w-0 flex-wrap-reverse items-center justify-end gap-px" aria-label={`${shown.length + more} subagents running`}>
      {more > 0 && <span className="mr-1 shrink-0 tabular-nums">+{more}</span>}
      {shown.map((row) => <SubagentLink key={row.id} row={row} className={cn(DASH_HIT, 'subagent-dash-in origin-right')}><ActivityDash color={backgroundColors[row.state.state === 'approval' ? 'approval' : 'subagents'].bg} beat={row.beat} /></SubagentLink>)}
    </div>
  )
}

/** `drag`: the pane its press-drag opens (`usePayloadDrag`); none where the dash only reveals a card (kernels, reminders). */
type Dash = { key: string; label: string; color: string; width?: string; beat?: number; open: (() => void) | { terminal: string }; drag?: TrayDrag }

/**
 * Jobs, terminals, kernels, browsers, artifacts, the desktop then reminders, left-aligned against the grabber's right end; each
 * category's "+N" at its outer (right) end. A dash opens what it stands for (`tray`: the panels the header used to hold).
 */
export function BackgroundDashes({ session, tray }: { session: string; tray: Tray }) {
  const { sessions: store } = useScope()
  const jobs = useSessionField(session, 'jobs')
  const ptys = useSessionField(session, 'ptys')
  const terminals = useSessionField(session, 'terminals')
  const terminalIds = useMemo(() => new Set([...(terminals ?? []), ...(ptys ?? [])]), [terminals, ptys])
  const reminders = useSessionField(session, 'reminders')
  const details = useSessionField(session, 'reminderDetails')
  const beats = useSessionField(session, 'jobBeats')
  const kernels = useSessionKernels(session)
  const reveal = (card: 'jobs' | 'kernels' | 'reminders') => () => {
    store.getState().setTodosShown(session, true)
    store.getState().setBackgroundExpanded(session, card, true)
  }
  const categories: [string, Dash[]][] = [
    ['jobs', [...(jobs ?? [])].map((id) => ({ key: id, label: `Job ${id} · show running jobs`, color: backgroundColors.jobs.bg, beat: beats?.[id], open: reveal('jobs'), drag: jobsDrag(session) }))],
    ['terminals', [...terminalIds].map((id) => ({ key: id, label: `Terminal ${id} · open terminal`, color: backgroundColors.terminals.bg, beat: beats?.[id], open: { terminal: id } }))],
    ['kernels', kernels.map((k) => ({ key: JSON.stringify(k.id), label: `Kernel ${kernelName(k)} · ${phaseText(k)} · show kernels`, color: cn(backgroundColors.kernels.bg, k.phase !== 'ready' && 'motion-safe:animate-pulse'), open: reveal('kernels') }))],
    ['browsers', tray.browsers.map((t) => ({ key: t.kind === 'agent' ? 'agent' : t.id, label: targetAction(t), color: backgroundColors.browser.bg, open: () => tray.openBrowser(t), drag: browserDrag(session, t) }))],
    ['whiteboards', tray.whiteboards.map((w) => ({ key: w.doc, label: `Whiteboard ${w.title} · open`, color: backgroundColors.whiteboard.bg, open: () => tray.openWhiteboard(w), drag: whiteboardDrag(session, w) }))],
    ['artifacts', tray.artifacts.map((a) => ({ key: a.call_id, label: `Artifact ${a.title} · open`, color: backgroundColors.artifacts.bg, width: backgroundColors.artifacts.dash, open: () => showArtifact(session, a.call_id), drag: artifactDrag(session, a.call_id, a.title) }))],
    ['desktops', tray.vm ? [{ key: tray.vm.name, label: `Desktop of ${tray.vm.name}`, color: backgroundColors.desktop.bg, open: tray.vm.open, drag: vmDrag(session, tray.vm.name) }] : []],
    ['reminders', [...(reminders ?? [])].map((id) => ({ key: id, label: `Reminder ${id} · ${details?.get(id)?.message ?? 'pending'}`, color: backgroundColors.reminders.bg, open: reveal('reminders') }))],
  ]
  const groups: ReactNode[] = []
  for (const [category, dashes] of categories) {
    if (!dashes.length) continue
    groups.push(
      <div key={category} className="flex min-w-0 flex-wrap-reverse items-center gap-px" aria-label={`${dashes.length} ${category}`}>
        {dashes.slice(0, STRIP_CAP).map((d) => {
          const dash = <ActivityDash color={d.color} width={d.width} beat={d.beat} />
          return typeof d.open === 'function'
            ? <DashButton key={d.key} label={d.label} onClick={d.open} drag={d.drag}>{dash}</DashButton>
            : <TerminalLink key={d.key} session={session} job={d.open.terminal} label={d.label} name={`Terminal ${d.open.terminal}`} className={DASH_HIT}>{dash}</TerminalLink>
        })}
        {dashes.length > STRIP_CAP && <span className="ml-1 shrink-0 tabular-nums">+{dashes.length - STRIP_CAP}</span>}
      </div>,
    )
  }
  return groups.length ? <>{groups}</> : null
}

/** A dash that opens its item on a click and, pressed and dragged (a finger: held first), into a pane. */
function DashButton({ label, onClick, drag, children }: { label: string; onClick(): void; drag?: TrayDrag; children: ReactNode }) {
  const props = usePayloadDrag(drag?.payload ?? null, drag?.title ?? label, drag?.prepare)
  return <button type="button" title={label} aria-label={label} className={DASH_HIT} onClick={onClick} {...props}>{children}</button>
}

/** The strip's dash: as thick as the grabber bar, coloured and blinking like the dot it replaces; `width` overrides the standard 16 px. */
export function ActivityDash({ color, width = 'w-4', beat }: { color: string; width?: string; beat?: number }) {
  return <span key={beat ?? 0} className={cn('subagent-dot activity-dash h-[5px] shrink-0 rounded-full transition-[height,opacity]', width, color, beat !== undefined && 'subagent-blink')} />
}

export function ActivityDot({ color, beat }: { color: string; beat?: number }) {
  return <span key={beat ?? 0} className={cn('subagent-dot size-2 shrink-0 rounded-full', color, beat !== undefined && 'subagent-blink')} />
}

/** Identical drag target for a subagent's dot and detail row. */
export function SubagentLink({ row, children, className }: { row: AgentInfo; children: ReactNode; className: string }) {
  const drag = useSessionDrag(row.id, row.name)
  const label = dotLabel(row)
  return <Link to="/c/$session" params={{ session: row.id }} {...drag} title={label} aria-label={label} className={className}>{children}</Link>
}

/**
 * Same popout as PopoutTerminalButton. Where there are no windows (iOS) the terminal opens beside the chat as a pane —
 * the iPad's window, as a dragged one does — or, outside a tiling layout (the phone), in-page. Pressed and dragged (a
 * finger: held first) it opens in a pane at the drop. `name` labels that pane.
 */
export function TerminalLink({ session, job, label, name, className, children }: { session: string; job: string; label: string; name?: string; className: string; children: ReactNode }) {
  const host = useTileHost()
  const owner = host ?? session
  const drag = terminalDrag(owner, { session, job }, name ?? label)
  const props = usePayloadDrag(drag.payload, drag.title)
  return (
    <Link to="/pty/$session/$job" params={{ session, job }} title={label} aria-label={label} className={className} {...props} onClick={(e) => {
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
      if (!canPopout()) {
        if (host === null || isMobileLayout() || openJobTerminalTile(host, { session, job }, name ?? label) === null) return
        e.preventDefault()
        return
      }
      e.preventDefault()
      void openPopout({ kind: 'pty', session, job }).catch((e: unknown) => toast.error(String(e)))
    }}>{children}</Link>
  )
}
