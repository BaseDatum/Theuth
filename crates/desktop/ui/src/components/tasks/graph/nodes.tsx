// 214 D8: the task graph's node cards and its edge — imported only by Canvas.tsx, so `@xyflow/react` stays in the
// lazily loaded chunk. Colours are theme tokens (model.ts `STATE_TONE`), so both themes read.
import { BaseEdge, type Edge, type EdgeProps, getSmoothStepPath, Handle, type Node, type NodeProps, Position } from '@xyflow/react'
import { CircleCheck, Gavel, Hourglass, Link2, MessageCircleQuestion, UserRound } from 'lucide-react'
import { memo } from 'react'
import { cn } from '@/lib/utils'
import type { Priority } from '@/rpc/bindings/method.roster.output'
import { ownerLabel, shortId } from '../helpers'
import { type EdgeModel, type PillNodeModel, pillWidth, STATE_TONE, TASK_H, TASK_W, type TaskNodeModel } from './model'

export type TaskNodeData = { model: TaskNodeModel; selected: boolean }
export type PillNodeData = { model: PillNodeModel }
export type TaskNodeType = Node<TaskNodeData, 'task'>
export type PillNodeType = Node<PillNodeData, 'pill'>
export type TaskEdgeData = { kind: EdgeModel['kind']; open: boolean; dim: boolean }
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

/** A task card: state stripe and badge, title (two lines), owner, priority, the done-when hint, `#id`. Closed tasks are
 * dimmed; a task still waiting on an open blocker carries the hourglass. */
export const TaskNode = memo(function TaskNode({ data }: NodeProps<TaskNodeType>) {
  const { model: m, selected } = data
  const t = m.task
  const tone = STATE_TONE[t.state]
  const prio = PRIORITY[t.priority]
  return (
    <>
      <Handles />
      <div
        className={cn(
          'tg-node tg-enter relative flex flex-col overflow-hidden rounded-[10px] border border-border-subtle bg-layer-1 py-2 pr-2.5 pl-3.5 text-left shadow-xs',
          selected && 'tg-selected',
          m.closed && 'opacity-55',
        )}
        style={{ width: TASK_W, height: TASK_H }}
        data-task-id={t.id}
      >
        <span aria-hidden className="absolute inset-y-0 left-0 w-[3px]" style={{ background: tone }} />
        <div className="flex min-h-0 items-start gap-1.5">
          {prio && (
            <span className="shrink-0 font-mono text-[12px] leading-[17px] font-bold" style={{ color: prio.tone }} title={`${t.priority} priority`}>
              {prio.mark}
            </span>
          )}
          <span className={cn('line-clamp-2 min-w-0 flex-1 text-[13px] leading-[17px] font-medium text-foreground wrap-anywhere', t.state === 'cancelled' && 'line-through')}>{t.title}</span>
          {m.waiting && <Hourglass className="mt-0.5 size-3.5 shrink-0 text-danger" aria-label="waiting on an open blocker" />}
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
          <span className="ml-auto shrink-0 font-mono text-fg-tertiary">#{shortId(t.id)}</span>
        </div>
      </div>
    </>
  )
})

const PILL_ICON = { decision: Gavel, person: UserRound, task: Link2, other: MessageCircleQuestion } as const
const PILL_TITLE = { decision: 'waiting on a decision', person: 'waiting on a person', task: 'a task outside this graph', other: 'a system blocker' } as const

/** A blocker that is not a task here (`decision:`, `person:`, a phrase, another project's task): a small pill. */
export const PillNode = memo(function PillNode({ data }: NodeProps<PillNodeType>) {
  const m = data.model
  const Icon = PILL_ICON[m.blocker.kind]
  return (
    <>
      <Handles />
      <div
        className="tg-pill tg-enter flex h-7 items-center gap-1.5 rounded-full border border-dashed border-danger-border bg-danger-bg px-2.5 text-[12px] text-foreground"
        style={{ width: pillWidth(m.label) }}
        title={`${PILL_TITLE[m.blocker.kind]}: ${m.blocker.raw}`}
      >
        <Icon className="size-3.5 shrink-0 text-danger" aria-hidden />
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
  parent: 'var(--color-fg-tertiary)',
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

/** Blocks (solid; accent while the blocker is open, faint once it is done), a non-task blocker (dotted danger) or the
 * parent link (dashed, no layout weight of its own beyond ELK's). Edges touching the selected task stay bright. */
export const TaskEdge = memo(function TaskEdge(props: EdgeProps<TaskEdgeType>) {
  const [path] = getSmoothStepPath({ ...props, borderRadius: 12, offset: 16 })
  const d = props.data
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
