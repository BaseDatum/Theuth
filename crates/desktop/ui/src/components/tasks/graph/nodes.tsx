// 214 D8: the task graph's node cards and its edge — imported only by Canvas.tsx, so `@xyflow/react` stays in the
// lazily loaded chunk. Colours are theme tokens (model.ts `STATE_TONE`), so both themes read.
import { BaseEdge, type Edge, type EdgeProps, getSmoothStepPath, Handle, type Node, type NodeProps, Position } from '@xyflow/react'
import { CircleCheck, Gavel, Hourglass, Link2, ListTree, MessageCircleQuestion, TriangleAlert, UserRound } from 'lucide-react'
import { memo } from 'react'
import { cn } from '@/lib/utils'
import type { Priority } from '@/rpc/bindings/method.roster.output'
import { ownerLabel, shortId } from '../helpers'
import { accentColor, waitingOnChildren } from '../hierarchy'
import { type EdgeModel, type PillNodeModel, pillWidth, STATE_TONE, TASK_H, TASK_W, type TaskNodeModel } from './model'

export type TaskNodeData = { model: TaskNodeModel; selected: boolean }
export type PillNodeData = { model: PillNodeModel }
export type TaskNodeType = Node<TaskNodeData, 'task'>
export type PillNodeType = Node<PillNodeData, 'pill'>
export type Point = { x: number; y: number }
/** `route`: ELK's orthogonal route (start, bends, end) when the drawn layout is the model's own; else smooth-step. */
export type TaskEdgeData = { kind: EdgeModel['kind']; open: boolean; dim: boolean; route: Point[] | null }
export type TaskEdgeType = Edge<TaskEdgeData, 'task'>

const HANDLE = { opacity: 0, width: 1, height: 1, minWidth: 0, minHeight: 0, border: 0, pointerEvents: 'none' } as const

function Handles() {
  return (
    <>
      <Handle type="target" position={Position.Left} id="l" style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Right} id="r" style={HANDLE} isConnectable={false} />
    </>
  )
}

/** `!!` urgent, `!` high, nothing for medium, `↓` low. */
const PRIORITY: Record<Priority, { mark: string; tone: string } | null> = {
  urgent: { mark: '!!', tone: 'var(--color-danger)' },
  high: { mark: '!', tone: 'var(--color-warning)' },
  medium: null,
  low: { mark: '↓', tone: 'var(--color-fg-tertiary)' },
}

/** A task card: state stripe and badge, title (two lines), owner, priority, the done-when hint, children progress,
 * `#id`. An accent turns the stripe and the title band its colour (faded when inherited from an ancestor). Closed tasks
 * are dimmed; a task waiting on an open blocker or open children carries the hourglass; one on a blocker cycle the
 * warning. */
export const TaskNode = memo(function TaskNode({ data }: NodeProps<TaskNodeType>) {
  const { model: m, selected } = data
  const t = m.task
  const tone = STATE_TONE[t.state]
  const prio = PRIORITY[t.priority]
  const accent = m.accent ? accentColor(m.accent.key) : null
  const kids = waitingOnChildren(m.children ?? undefined)
  return (
    <>
      <Handles />
      <div
        className={cn(
          'tg-node tg-enter relative flex flex-col overflow-hidden rounded-[10px] border border-border-subtle bg-layer-1 py-2 pr-2.5 pl-3.5 text-left shadow-xs',
          selected && 'tg-selected',
          m.closed && 'opacity-55',
          m.inCycle && 'border-warning',
        )}
        style={{ width: TASK_W, height: TASK_H }}
        data-task-id={t.id}
      >
        {accent && (
          <span
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 h-[42px]"
            style={{ background: `linear-gradient(color-mix(in oklab, ${accent} ${m.accent!.inherited ? 9 : 18}%, transparent), transparent)` }}
          />
        )}
        <span
          aria-hidden
          className={cn('absolute inset-y-0 left-0', accent ? 'w-[5px]' : 'w-[3px]')}
          style={{ background: accent ?? tone, opacity: m.accent?.inherited ? 0.45 : 1 }}
        />
        <div className="relative flex min-h-0 items-start gap-1.5">
          {prio && (
            <span className="shrink-0 font-mono text-[12px] leading-[17px] font-bold" style={{ color: prio.tone }} title={`${t.priority} priority`}>
              {prio.mark}
            </span>
          )}
          <span className={cn('line-clamp-2 min-w-0 flex-1 text-[13px] leading-[17px] font-medium text-foreground wrap-anywhere', t.state === 'cancelled' && 'line-through')}>{t.title}</span>
          {m.inCycle && <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-warning" aria-label="on a blocker cycle" />}
          {m.waiting && <Hourglass className="mt-0.5 size-3.5 shrink-0 text-danger" aria-label={kids ?? 'waiting on an open blocker'} />}
        </div>
        <div className="mt-auto flex min-w-0 items-center gap-1.5 text-[11px] leading-4">
          <span
            className="shrink-0 rounded-full border px-1.5 font-medium"
            style={{ color: tone, borderColor: `color-mix(in oklab, ${tone} 45%, transparent)`, background: `color-mix(in oklab, ${tone} 12%, transparent)` }}
          >
            {t.state}
          </span>
          <span className="min-w-0 truncate text-fg-secondary">{ownerLabel(t.owner)}</span>
          {t.done_when && <CircleCheck className="size-3 shrink-0 text-success" aria-label="has done-when criteria" />}
          {m.children && m.children.total > 0 && (
            <span className="flex shrink-0 items-center gap-0.5 text-fg-secondary tabular-nums" title={kids ?? `${m.children.done} of ${m.children.total} children done`}>
              <ListTree className="size-3" aria-hidden />
              {m.children.done}/{m.children.total}
            </span>
          )}
          <span className="ml-auto shrink-0 font-mono text-fg-tertiary">#{shortId(t.id)}</span>
        </div>
      </div>
    </>
  )
})

const PILL_ICON = { decision: Gavel, person: UserRound, task: Link2, other: MessageCircleQuestion } as const
const PILL_TITLE = { decision: 'waiting on a decision', person: 'waiting on a person', task: 'a task outside this graph', other: 'a system blocker' } as const

/** A blocker that is not a task here (`decision:`, `person:`, a phrase, another project's task, or — `ghost` — a task
 * the "under" filter leaves out): a small pill. */
export const PillNode = memo(function PillNode({ data }: NodeProps<PillNodeType>) {
  const m = data.model
  const Icon = PILL_ICON[m.blocker.kind]
  return (
    <>
      <Handles />
      <div
        className={cn(
          'tg-pill tg-enter flex h-7 items-center gap-1.5 rounded-full border border-dashed px-2.5 text-[12px] text-foreground',
          m.ghost ? 'border-border-strong bg-layer-1 opacity-80' : 'border-danger-border bg-danger-bg',
        )}
        style={{ width: pillWidth(m.label) }}
        title={m.ghost ? `a task outside this view: ${m.label}` : `${PILL_TITLE[m.blocker.kind]}: ${m.blocker.raw}`}
      >
        <Icon className={cn('size-3.5 shrink-0', m.ghost ? 'text-fg-tertiary' : 'text-danger')} aria-hidden />
        <span className="truncate">{m.label}</span>
      </div>
    </>
  )
})

/** Arrowhead ids, one per edge tone. */
const MARK = { open: 'tg-arrow-open', done: 'tg-arrow-done', parent: 'tg-arrow-parent' } as const
const STROKE = {
  open: 'var(--color-fg-accent)',
  done: 'color-mix(in oklab, var(--color-fg-tertiary) 70%, transparent)',
  parent: 'color-mix(in oklab, var(--color-fg-secondary) 80%, transparent)',
  pill: 'var(--color-danger)',
} as const

/** The arrowheads every edge references; rendered once by the canvas. */
export function TaskEdgeMarkers() {
  const marks: [string, string][] = [
    [MARK.open, STROKE.open],
    [MARK.done, STROKE.done],
    [MARK.parent, STROKE.parent],
  ]
  return (
    <svg aria-hidden className="pointer-events-none absolute size-0">
      <defs>
        {marks.map(([id, fill]) => (
          <marker key={id} id={id} viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="9" markerHeight="9" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
            <path d="M1.5 1.5 L8.5 5 L1.5 8.5 Q3.2 5 1.5 1.5 Z" style={{ fill }} />
          </marker>
        ))}
      </defs>
    </svg>
  )
}

/** The edge key at the canvas's top left: what each stroke means (the hierarchy only while it is drawn). */
export function EdgeLegend({ hierarchy }: { hierarchy: boolean }) {
  const rows: [string, string, string | undefined][] = [
    ['blocks', STROKE.open, undefined],
    ['finished dependency', STROKE.done, undefined],
    ['outside blocker', STROKE.pill, '2 4'],
    ...(hierarchy ? ([['parent → child', STROKE.parent, '6 5']] as [string, string, string][]) : []),
  ]
  return (
    <ul aria-label="Edge legend" className="pointer-events-none absolute top-2 left-2 z-[5] space-y-0.5 rounded-md border border-border-subtle bg-background/85 px-2 py-1.5 text-[11px] text-fg-secondary backdrop-blur-sm">
      {rows.map(([label, stroke, dash]) => (
        <li key={label} className="flex items-center gap-1.5">
          <svg aria-hidden width="22" height="6">
            <line x1="1" y1="3" x2="21" y2="3" style={{ stroke }} strokeWidth="1.75" strokeDasharray={dash} strokeLinecap="round" />
          </svg>
          {label}
        </li>
      ))}
    </ul>
  )
}

/** An orthogonal polyline with its corners rounded (radius `r`, less where a segment is short). */
function roundedPath(pts: readonly Point[], r = 10): string {
  const [first, ...rest] = pts
  if (!first) return ''
  let d = `M ${first.x} ${first.y}`
  for (let i = 0; i < rest.length; i++) {
    const cur = rest[i]!
    const prev = pts[i]!
    const next = rest[i + 1]
    if (!next) {
      d += ` L ${cur.x} ${cur.y}`
      break
    }
    const inLen = Math.hypot(cur.x - prev.x, cur.y - prev.y)
    const outLen = Math.hypot(next.x - cur.x, next.y - cur.y)
    const k = Math.min(r, inLen / 2, outLen / 2)
    const a = { x: cur.x - ((cur.x - prev.x) / (inLen || 1)) * k, y: cur.y - ((cur.y - prev.y) / (inLen || 1)) * k }
    const b = { x: cur.x + ((next.x - cur.x) / (outLen || 1)) * k, y: cur.y + ((next.y - cur.y) / (outLen || 1)) * k }
    d += ` L ${a.x} ${a.y} Q ${cur.x} ${cur.y} ${b.x} ${b.y}`
  }
  return d
}

/** Blocks (solid; accent while the blocker is open, faint once it is done), a non-task blocker (dotted danger) or the
 * parent link (dashed). ELK routes them around the cards; edges touching the selected task stay bright. */
export const TaskEdge = memo(function TaskEdge(props: EdgeProps<TaskEdgeType>) {
  const d = props.data
  const path = d?.route && d.route.length >= 2 ? roundedPath(d.route) : getSmoothStepPath({ ...props, borderRadius: 12, offset: 16 })[0]
  const kind = d?.kind ?? 'blocks'
  const tone = kind === 'pill' ? 'pill' : kind === 'parent' ? 'parent' : d?.open ? 'open' : 'done'
  const marker = kind === 'parent' ? MARK.parent : kind === 'pill' || d?.open ? MARK.open : MARK.done
  return (
    <BaseEdge
      id={props.id}
      path={path}
      interactionWidth={0}
      markerEnd={kind === 'pill' ? undefined : `url(#${marker})`}
      style={{
        stroke: STROKE[tone],
        strokeWidth: kind === 'blocks' && d?.open ? 1.75 : 1.25,
        strokeDasharray: kind === 'parent' ? '6 5' : kind === 'pill' ? '2 4' : undefined,
        strokeLinecap: 'round',
        opacity: d?.dim ? 0.3 : 1,
      }}
    />
  )
})
