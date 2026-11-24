// 214 D9: the routine graph's cards — one generic card (Trigger, Gate, Until check, Run, Deliver, End), the Prompt and
// History. Colour by role on the top strip and the icon; broken cards turn danger. Imported only by RoutineGraph.tsx
// (the lazy chunk), so `@xyflow/react` stays out of the main bundle.
import { Link } from '@tanstack/react-router'
import { Handle, type Node, type NodeProps, Position } from '@xyflow/react'
import { Bot, CalendarClock, Clock, Eye, FileText, Flag, History, type LucideIcon, Repeat, Send, ShieldCheck, SquareTerminal } from 'lucide-react'
import { createContext, memo, type ReactNode, use, useState } from 'react'
import { Pulse } from '@/components/chat/Pulse'
import { cn } from '@/lib/utils'
import type { Kind } from '../helpers'
import { KindBadge, RESULT_CLASS } from '../RoutinesView'
import { CARD_W, type CardData, type GraphLink, HANDLE_Y, type Role, type Row, type Tone } from './model'

/** Set in a popped graph window: a link opens there (a window of its own) instead of navigating the window away. */
export const LinkOut = createContext<((l: GraphLink) => void) | null>(null)

/** A run's chat or task link on a card; a click never reaches the card (its own section click). */
function Out({ link, className, children }: { link: Extract<GraphLink, { kind: 'session' | 'task' }>; className: string; children: ReactNode }) {
  const out = use(LinkOut)
  const click = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (!out) return
    e.preventDefault()
    out(link)
  }
  return link.kind === 'session' ? (
    <Link to="/c/$session" params={{ session: link.session }} search={{}} className={className} onClick={click}>
      {children}
    </Link>
  ) : (
    <Link to="/tasks/$id" params={{ id: link.id }} className={className} onClick={click}>
      {children}
    </Link>
  )
}

export type CardNode = Node<CardData & { w: number; h: number }, 'card' | 'prompt' | 'history'>

const ROLE: Record<Role, { icon: LucideIcon; strip: string; tint: string }> = {
  trigger: { icon: Clock, strip: 'bg-info', tint: 'text-info' },
  gate: { icon: ShieldCheck, strip: 'bg-project-ochre', tint: 'text-project-ochre' },
  until: { icon: SquareTerminal, strip: 'bg-project-plum', tint: 'text-project-plum' },
  run: { icon: Bot, strip: 'bg-fg-accent', tint: 'text-fg-accent' },
  prompt: { icon: FileText, strip: 'bg-project-slate', tint: 'text-project-slate' },
  deliver: { icon: Send, strip: 'bg-project-malachite', tint: 'text-project-malachite' },
  end: { icon: Flag, strip: 'bg-fg-tertiary', tint: 'text-fg-tertiary' },
  history: { icon: History, strip: 'bg-project-sand', tint: 'text-project-sand' },
}
const TRIGGER_ICON: Record<Kind, LucideIcon> = { Clock, Once: CalendarClock, Watcher: Eye, Loop: Repeat }
const TONE: Record<Tone, string> = { ok: 'text-success', warn: 'text-warning', danger: 'text-danger', muted: 'text-fg-tertiary' }
const KINDS: readonly string[] = ['Clock', 'Once', 'Watcher', 'Loop']

const HANDLE = { opacity: 0, width: 1, height: 1, minWidth: 0, minHeight: 0, border: 0, pointerEvents: 'none' } as const
const SIDE = { ...HANDLE, top: HANDLE_Y } as const

/** `bottomX`: the bottom source handle's x when it must not sit at the centre (the wide Prompt points at Run). */
function Handles({ bottomX }: { bottomX?: number }) {
  return (
    <>
      <Handle type="target" position={Position.Left} id="l" style={SIDE} isConnectable={false} />
      <Handle type="source" position={Position.Right} id="r" style={SIDE} isConnectable={false} />
      <Handle type="target" position={Position.Top} id="t" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} id="ts" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Bottom} id="b" style={bottomX === undefined ? HANDLE : { ...HANDLE, left: bottomX }} isConnectable={false} />
      <Handle type="target" position={Position.Bottom} id="bt" style={HANDLE} isConnectable={false} />
    </>
  )
}

/** Strip and header shared by every card. */
function Shell({ data, className, children }: { data: CardNode['data']; className?: string; children: React.ReactNode }) {
  const role = ROLE[data.role]
  const kind = data.role === 'trigger' && data.badge && KINDS.includes(data.badge) ? (data.badge as Kind) : null
  const Icon = kind ? TRIGGER_ICON[kind] : role.icon
  return (
    <div
      className={cn(
        'rg-card flex flex-col overflow-hidden rounded-[10px] border bg-layer-1 text-foreground shadow-xs',
        data.danger ? 'border-danger/60' : 'border-border-strong',
        data.section && 'cursor-pointer',
        className,
      )}
      style={{ width: data.w }}
      title={data.section ? `Open ${data.section}` : undefined}
    >
      <div className={cn('h-[3px] shrink-0', data.danger ? 'bg-danger' : role.strip)} />
      <div className="flex h-[34px] shrink-0 items-center gap-1.5 px-3">
        <Icon aria-hidden className={cn('size-3.5 shrink-0', data.danger ? 'text-danger' : role.tint)} />
        <span className="text-[12.5px] font-medium">{data.title}</span>
        {kind ? (
          <span className="ml-auto">
            <KindBadge kind={kind} />
          </span>
        ) : (
          data.badge && <span className="ml-auto truncate rounded bg-layer-2 px-1.5 py-px text-[10.5px] text-fg-secondary">{data.badge}</span>
        )}
      </div>
      {children}
    </div>
  )
}

function Line({ row }: { row: Row }) {
  return (
    <li className="flex h-[18px] items-baseline gap-2 text-[11.5px] leading-[18px]">
      <span className="w-[68px] shrink-0 truncate text-fg-tertiary">{row.label}</span>
      <span className={cn('min-w-0 flex-1 truncate', row.mono && 'font-mono text-[11px]', row.tone ? TONE[row.tone] : 'text-fg-secondary')} title={row.title ?? row.text}>
        {row.session ? (
          <Out link={{ kind: 'session', session: row.session }} className="nodrag underline underline-offset-2 hover:text-foreground">
            {row.text}
          </Out>
        ) : (
          row.text
        )}
      </span>
    </li>
  )
}

export const Card = memo(function Card({ data }: NodeProps<CardNode>) {
  return (
    <>
      <Handles />
      <Shell data={data}>
        <ul className="flex flex-col px-3 pb-2.5">
          {data.rows.map((r, i) => (
            <Line key={`${r.label}-${i}`} row={r} />
          ))}
        </ul>
      </Shell>
    </>
  )
})

/** The prompt's first lines; "Show all" grows the card upward so it never covers the Run card below it. */
export const PromptCard = memo(function PromptCard({ data }: NodeProps<CardNode>) {
  const [open, setOpen] = useState(false)
  return (
    <div className="relative" style={{ width: data.w, height: data.h }}>
      <Handles bottomX={CARD_W / 2} />
      <Shell data={data} className="absolute right-0 bottom-0 left-0">
        <pre className={cn('nowheel overflow-hidden px-3 font-mono text-[11px] leading-4 text-fg-secondary', open ? 'max-h-[60vh] overflow-y-auto whitespace-pre-wrap' : 'whitespace-pre', !data.full && 'pb-2.5')}>
          {open && data.full ? data.full : (data.lines ?? []).join('\n')}
        </pre>
        {data.full && (
          <button
            type="button"
            className="nodrag mb-2.5 h-6 shrink-0 px-3 text-left text-[11px] text-fg-tertiary hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation()
              setOpen((v) => !v)
            }}
          >
            {open ? 'Show less' : 'Show all'}
          </button>
        )}
      </Shell>
    </div>
  )
})

export const HistoryCard = memo(function HistoryCard({ data }: NodeProps<CardNode>) {
  const runs = data.runs ?? []
  return (
    <>
      <Handles />
      <Shell data={data}>
        <ul className="flex flex-col px-3">
          {data.rows.map((r, i) => (
            <Line key={`${r.label}-${i}`} row={r} />
          ))}
        </ul>
        <ul className="mx-3 mt-1.5 mb-2.5 flex flex-col border-t border-border-subtle">
          {runs.length === 0 && <li className="flex h-[22px] items-center text-[11.5px] text-fg-tertiary">No run yet.</li>}
          {runs.map((r) => (
            <li key={r.key} className="flex h-[22px] items-center gap-2 text-[11.5px]">
              <span className="w-[58px] shrink-0 tabular-nums text-fg-tertiary" title={r.at}>
                {r.when}
              </span>
              <span className="w-[58px] shrink-0 truncate text-fg-secondary">{r.what}</span>
              {r.running ? (
                <span className="flex items-center gap-1.5 text-success">
                  <Pulse className="size-2 bg-success" /> running
                </span>
              ) : r.result ? (
                <span className={cn('min-w-0 truncate rounded border px-1 text-[10.5px] leading-4', RESULT_CLASS[r.result])} title={r.detail ?? undefined}>
                  {r.label}
                </span>
              ) : (
                <span className="min-w-0 truncate text-fg-tertiary" title={r.detail ?? undefined}>
                  {r.label}
                </span>
              )}
              {r.session ? (
                <Out link={{ kind: 'session', session: r.session }} className="nodrag ml-auto shrink-0 text-fg-secondary underline underline-offset-2 hover:text-foreground">
                  session
                </Out>
              ) : (
                r.task && (
                  <Out link={{ kind: 'task', id: r.task }} className="nodrag ml-auto shrink-0 text-fg-secondary underline underline-offset-2 hover:text-foreground">
                    task
                  </Out>
                )
              )}
            </li>
          ))}
        </ul>
      </Shell>
    </>
  )
})
