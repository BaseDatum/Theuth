import { type Components, defineRegistry } from '@json-render/react'
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  CircleCheck,
  CircleHelp,
  CircleX,
  Copy,
  File,
  Flag,
  Folder,
  Info,
  Lightbulb,
  Square,
  SquareCheck,
  TriangleAlert,
} from 'lucide-react'
import { Children, type ComponentProps, isValidElement, type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Separator } from '@/components/ui/separator'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import { catalog } from './catalog'
import { Excerpt } from './Excerpt'
import { GraphView } from './Graph'
import { SequenceView } from './Sequence'

// Every string is a React text node — no `dangerouslySetInnerHTML` (design 30 §11). Text, lists and headings are
// `div`s with ARIA roles because the Streamdown wrapper styles `p`/`li`/`h1-h4` descendants for prose.

// A remote `src` is a fetch the model chose (design 30 §11: a block is a sink if a component carries bytes out), so it
// loads only on the reader's click; an inline `data:image/` has nothing to leak and shows at once.
function Picture({ src, alt, caption }: { src: string; alt: string; caption?: string }) {
  const [shown, setShown] = useState(() => src.toLowerCase().startsWith('data:image/'))
  return (
    <figure className="flex flex-col items-start gap-1 text-sm">
      {shown ? (
        <img src={src} alt={alt} loading="lazy" className="max-h-72 rounded-lg object-contain" />
      ) : (
        <Button variant="outline" size="sm" onClick={() => setShown(true)} title={src}>
          Load image · {alt}
        </Button>
      )}
      {caption && <figcaption className="text-xs text-fg-tertiary">{caption}</figcaption>}
    </figure>
  )
}

const GAP: Record<string, string> = { none: 'gap-0', sm: 'gap-1', md: 'gap-3', lg: 'gap-6' }
const COLUMNS: Record<number, string> = {
  1: 'grid-cols-1',
  2: 'grid-cols-2',
  3: 'grid-cols-3',
  4: 'grid-cols-4',
  5: 'grid-cols-5',
  6: 'grid-cols-6',
}
const HEADING: Record<number, string> = { 1: 'text-2xl', 2: 'text-xl', 3: 'text-lg', 4: 'text-base' }
const TEXT: Record<string, string> = {
  body: '',
  muted: 'text-fg-secondary',
  caption: 'text-xs text-fg-tertiary',
  lead: 'text-lg',
  code: 'font-mono text-xs',
}
const BADGE: Record<string, string> = {
  success: 'border-success-border bg-success-bg text-success',
  warning: 'border-warning-border bg-warning-bg text-warning',
}
const TONE: Record<string, { className: string; Icon: typeof Info }> = {
  info: { className: 'border-info-border bg-info-bg text-info', Icon: Info },
  success: { className: 'border-success-border bg-success-bg text-success', Icon: CircleCheck },
  warning: { className: 'border-warning-border bg-warning-bg text-warning', Icon: TriangleAlert },
  error: { className: 'border-danger-border bg-danger-bg text-danger', Icon: CircleX },
}
const TREND: Record<string, { className: string; Icon: typeof ArrowUp }> = {
  up: { className: 'text-success', Icon: ArrowUp },
  down: { className: 'text-danger', Icon: ArrowDown },
  flat: { className: 'text-fg-tertiary', Icon: ArrowRight },
}
const DOT: Record<string, string> = {
  done: 'bg-fg-accent border-fg-accent',
  active: 'border-fg-accent ring-2 ring-fg-accent/30',
  pending: 'border-border-strong',
}
const CALLOUT: Record<string, typeof Info> = { note: Info, tip: Lightbulb, question: CircleHelp, important: Flag }

/** The `Badge` component body, shared with `Tags` and `Kanban` cards. */
function Tag({ text, variant }: { text: string; variant?: string }) {
  const v = variant ?? 'default'
  return v in BADGE ? (
    <Badge variant="outline" className={BADGE[v]}>
      {text}
    </Badge>
  ) : (
    <Badge variant={v as 'default' | 'secondary' | 'destructive'}>{text}</Badge>
  )
}

/** Inline polyline over `values` in a 100×24 box; a flat run is a mid-height line. */
function Spark({ values, className }: { values: number[]; className?: string }) {
  if (!values.length) return null
  const lo = Math.min(...values)
  const hi = Math.max(...values)
  const step = values.length > 1 ? 100 / (values.length - 1) : 0
  const points = values.map((v, i) => `${(i * step).toFixed(1)},${(hi === lo ? 12 : 2 + (1 - (v - lo) / (hi - lo)) * 20).toFixed(1)}`).join(' ')
  return (
    <svg viewBox="0 0 100 24" preserveAspectRatio="none" className={cn('h-6 w-24 shrink-0 stroke-current text-fg-accent', className)} aria-hidden>
      <polyline points={points} fill="none" strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  )
}

type TreeNode = { label: string; detail?: string; children: TreeNode[] }
/** Nesting from the `depth` of consecutive items; a depth jumping past its parent clamps to the deepest open level. */
function nest(items: { label: string; depth?: number; detail?: string }[]): TreeNode[] {
  const roots: TreeNode[] = []
  const open: TreeNode[] = []
  for (const it of items) {
    const node: TreeNode = { label: it.label, detail: it.detail, children: [] }
    const d = Math.min(it.depth ?? 0, open.length)
    open.length = d
    ;(d === 0 ? roots : open[d - 1]!.children).push(node)
    open.push(node)
  }
  return roots
}
function TreeRows({ nodes }: { nodes: TreeNode[] }) {
  return nodes.map((n, i) =>
    n.children.length ? (
      <Collapsible key={i} defaultOpen>
        <CollapsibleTrigger className="flex w-full items-center gap-1.5 text-left [&[data-state=open]>svg]:rotate-90">
          <ArrowRight className="size-3.5 shrink-0 text-fg-tertiary transition-transform" aria-hidden />
          <span className="font-medium">{n.label}</span>
          {n.detail && <span className="text-xs text-fg-tertiary">{n.detail}</span>}
        </CollapsibleTrigger>
        <CollapsibleContent className="ml-1.5 border-l border-border-subtle pl-4">
          <TreeRows nodes={n.children} />
        </CollapsibleContent>
      </Collapsible>
    ) : (
      <div key={i} className="flex items-center gap-1.5 pl-5">
        <span>{n.label}</span>
        {n.detail && <span className="text-xs text-fg-tertiary">{n.detail}</span>}
      </div>
    ),
  )
}

/** One `Diff` column: heading over line rows; a row differing from the other side's same row is marked. */
function DiffSide({ heading, lines, other, mark }: { heading: string; lines: string[]; other: string[]; mark: string }) {
  return (
    <div className="min-w-0">
      <div className="mb-1 text-xs text-fg-tertiary">{heading}</div>
      <pre className="overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5">
        {lines.map((l, i) => (
          <div key={i} className={cn('min-h-5', l !== other[i] && mark)}>
            {l}
          </div>
        ))}
      </pre>
    </div>
  )
}

function Metric({ label, value, trend, delta, spark }: { label: string; value: string; trend?: string; delta?: string; spark?: number[] }) {
  const t = trend ? TREND[trend] : undefined
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <div className="truncate text-xs text-fg-tertiary">{label}</div>
      <div className="flex items-baseline gap-1.5">
        <span className="text-2xl font-semibold tabular-nums">{value}</span>
        {t && <t.Icon className={cn('size-4 shrink-0 self-center', t.className)} aria-label={trend} />}
        {delta && <span className="text-xs text-fg-secondary">{delta}</span>}
        {spark && <Spark values={spark} className="self-center" />}
      </div>
    </div>
  )
}

/** Resets the transcript's inherited `wrap-anywhere` (Transcript.tsx) so a table column is never narrower than its longest word. */
export const NO_MID_WORD_BREAK = 'break-normal wrap-normal hyphens-none'
const MAX_CELL_CH = 48
/** Table cell wrapping: a short or single-token value never wraps; a token wider than the column may break anywhere; prose wraps at words. */
export function cellClass(text: string): string {
  const tokens = text.split(/\s+/)
  if (tokens.some((t) => t.length > MAX_CELL_CH)) return 'wrap-anywhere'
  if (tokens.length === 1 || text.length <= 16) return 'whitespace-nowrap'
  return ''
}

/** The text of a rendered cell (markdown inlines like `**b**` or `code` are elements around strings). */
function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(textOf).join('')
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children)
  return ''
}

/** A table cell that wraps at words only, shared by the json-render `Table` and the chat's markdown `td`.
 * Engines ignore `max-width` on a `td`, so the column cap lives on a block inside it. */
export function Td({ className, children, ...rest }: ComponentProps<'td'>) {
  return (
    <td {...rest} className={cn(className, NO_MID_WORD_BREAK, cellClass(textOf(children)))}>
      <div className="min-w-[6ch] max-w-[48ch]">{children}</div>
    </td>
  )
}

/** streamdown's `components.td` (MessageAssistant): its default classes and `data-streamdown` marker on `Td`. */
export function MarkdownTd({ node: _node, className, ...props }: ComponentProps<'td'> & { node?: unknown }) {
  return <Td className={cn('px-4 py-2 text-sm', className)} data-streamdown="table-cell" {...props} />
}

const components: Components<typeof catalog> = {
  Card: ({ props, children }) => (
    <div className="flex flex-col gap-3 rounded-xl border border-border-subtle bg-layer-1 p-4">
      {(props.title || props.description) && (
        <div className="flex flex-col gap-0.5">
          {props.title && <div className="font-semibold">{props.title}</div>}
          {props.description && <div className="text-sm text-fg-secondary">{props.description}</div>}
        </div>
      )}
      {children}
    </div>
  ),
  Stack: ({ props, children }) => (
    <div className={cn('flex', props.direction === 'horizontal' ? 'flex-row flex-wrap' : 'flex-col', GAP[props.gap ?? 'md'])}>
      {children}
    </div>
  ),
  Grid: ({ props, children }) => <div className={cn('grid gap-3', COLUMNS[props.columns])}>{children}</div>,
  Separator: () => <Separator />,
  Heading: ({ props }) => (
    <div role="heading" aria-level={props.level ?? 2} className={cn('font-semibold', HEADING[props.level ?? 2])}>
      {props.text}
    </div>
  ),
  Text: ({ props }) => <div className={cn('text-sm whitespace-pre-wrap', TEXT[props.variant ?? 'body'])}>{props.text}</div>,
  Badge: ({ props }) => <Tag {...props} />,
  Alert: ({ props }) => {
    const { className, Icon } = TONE[props.type ?? 'info']!
    return (
      <div role="alert" className={cn('flex gap-2 rounded-xl border px-4 py-3 text-sm', className)}>
        <Icon className="mt-0.5 size-4 shrink-0" aria-hidden />
        <div className="flex min-w-0 flex-col gap-0.5">
          {props.title && <div className="font-medium">{props.title}</div>}
          <div className="whitespace-pre-wrap text-foreground">{props.message}</div>
        </div>
      </div>
    )
  },
  Table: ({ props }) => (
    <div className="max-w-full overflow-x-auto [scrollbar-width:thin]">
      <table className="w-max min-w-full table-auto text-sm">
        <thead>
          <tr className="border-b border-border-subtle text-left text-fg-secondary">
            {props.columns.map((c, i) => (
              <th key={i} className={cn('px-2 py-1.5 font-semibold whitespace-nowrap', NO_MID_WORD_BREAK)}>
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {props.rows.map((row, i) => (
            <tr key={i} className="border-b border-border-subtle last:border-0">
              {row.map((cell, j) => (
                <Td key={j} className="px-2 py-1.5 align-top">
                  {cell}
                </Td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  ),
  KeyValueList: ({ props }) => (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
      {props.items.map((it, i) => (
        // Positional rows; a key can legitimately repeat.
        <div key={i} className="contents">
          <dt className="text-fg-secondary">{it.key}</dt>
          <dd className="min-w-0 wrap-anywhere">{it.value}</dd>
        </div>
      ))}
    </dl>
  ),
  Metric: ({ props }) => <Metric {...props} />,
  MetricGrid: ({ props }) => (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
      {props.items.map((m, i) => (
        <Metric key={i} {...m} />
      ))}
    </div>
  ),
  Progress: ({ props }) => {
    const max = props.max ?? 100
    const pct = max > 0 ? Math.min(100, Math.max(0, (props.value / max) * 100)) : 0
    return (
      <div className="flex flex-col gap-1 text-sm">
        {props.label && (
          <div className="flex justify-between text-fg-secondary">
            <span>{props.label}</span>
            <span className="tabular-nums">{Math.round(pct)}%</span>
          </div>
        )}
        <div role="progressbar" aria-valuenow={props.value} aria-valuemin={0} aria-valuemax={max} className="h-2 overflow-hidden rounded-full bg-layer-2">
          <div className="h-full rounded-full bg-fg-accent" style={{ width: `${pct}%` }} />
        </div>
      </div>
    )
  },
  Bars: ({ props }) => {
    const max = props.max ?? Math.max(1, ...props.items.map((it) => it.value))
    return (
      <div className="flex flex-col gap-1.5 text-sm">
        {props.items.map((it, i) => (
          <div key={i} className="grid grid-cols-[minmax(4rem,max-content)_1fr_max-content] items-center gap-2">
            <span className="truncate text-fg-secondary">{it.label}</span>
            <div className="h-2 rounded-full bg-layer-2">
              <div className="h-full rounded-full bg-fg-accent" style={{ width: `${max > 0 ? Math.min(100, Math.max(0, (it.value / max) * 100)) : 0}%` }} />
            </div>
            <span className="tabular-nums">{it.value}</span>
          </div>
        ))}
      </div>
    )
  },
  Timeline: ({ props }) => (
    <div role="list" className="flex flex-col text-sm">
      {props.items.map((it, i) => (
        <div key={i} role="listitem" className="flex gap-3">
          <div className="flex flex-col items-center">
            <div className={cn('mt-1.5 size-2.5 shrink-0 rounded-full border-2', DOT[it.state ?? 'pending'])} />
            {i < props.items.length - 1 && <div className="my-1 w-px flex-1 bg-border-subtle" />}
          </div>
          <div className="flex min-w-0 flex-col pb-3">
            <div className="text-xs text-fg-tertiary">{it.when}</div>
            <div className={cn('font-medium', it.state === 'pending' && 'text-fg-secondary')}>{it.title}</div>
            {it.detail && <div className="text-fg-secondary">{it.detail}</div>}
          </div>
        </div>
      ))}
    </div>
  ),
  Checklist: ({ props }) => (
    <div role="list" className="flex flex-col gap-1 text-sm">
      {props.items.map((it, i) => (
        <div key={i} role="listitem" className="flex items-start gap-2">
          {it.done ? (
            <SquareCheck className="mt-0.5 size-4 shrink-0 text-success" aria-label="done" />
          ) : (
            <Square className="mt-0.5 size-4 shrink-0 text-fg-tertiary" aria-label="to do" />
          )}
          <span className={cn(it.done && 'text-fg-secondary line-through')}>{it.text}</span>
        </div>
      ))}
    </div>
  ),
  Tabs: ({ props, children }) => (
    <Tabs defaultValue="0">
      <TabsList>
        {props.labels.map((l, i) => (
          <TabsTrigger key={i} value={String(i)}>
            {l}
          </TabsTrigger>
        ))}
      </TabsList>
      {props.labels.map((_, i) => (
        <TabsContent key={i} value={String(i)}>
          {Children.toArray(children)[i]}
        </TabsContent>
      ))}
    </Tabs>
  ),
  Accordion: ({ props, children }) => (
    <div className="flex flex-col divide-y divide-border-subtle">
      {props.titles.map((t, i) => (
        <Collapsible key={i} defaultOpen className="group/acc py-2">
          <CollapsibleTrigger className="flex w-full items-center gap-2 text-left text-sm font-medium">
            <ArrowRight className="size-3.5 shrink-0 text-fg-tertiary transition-transform group-data-[state=open]/acc:rotate-90" aria-hidden />
            {t}
          </CollapsibleTrigger>
          <CollapsibleContent className="pt-2 pl-5.5">{Children.toArray(children)[i]}</CollapsibleContent>
        </Collapsible>
      ))}
    </div>
  ),
  // 65: the one component with a renderer of ours; `loading` = the fence is still streaming (dangling edges dropped).
  Graph: ({ props, loading }) => <GraphView props={props} loading={loading} />,
  // 71 §2.4: actors and addressable steps; `ref` is inert here (no board, no root) and surfaces only as `aria-description`.
  Sequence: ({ props }) => <SequenceView props={props} />,
  Steps: ({ props }) => (
    <div role="list" className="flex flex-col gap-2 text-sm">
      {props.items.map((it, i) => (
        <div key={i} role="listitem" className="flex gap-3">
          <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-fg-accent text-xs font-semibold text-primary-foreground tabular-nums">{i + 1}</span>
          <div className="flex min-w-0 flex-col pt-0.5">
            <div className="font-medium">{it.title}</div>
            {it.detail && <div className="text-fg-secondary">{it.detail}</div>}
          </div>
        </div>
      ))}
    </div>
  ),
  Callout: ({ props }) => {
    const v = props.variant ?? 'note'
    const Icon = CALLOUT[v]!
    return (
      <div className="flex gap-2 border-l-2 border-fg-accent pl-3 text-sm">
        <Icon className="mt-0.5 size-4 shrink-0 text-fg-accent" aria-label={v} />
        <div className="flex min-w-0 flex-col gap-0.5">
          {props.title && <div className="font-medium">{props.title}</div>}
          <div className="whitespace-pre-wrap">{props.text}</div>
        </div>
      </div>
    )
  },
  Code: ({ props }) => (
    <div className="flex flex-col rounded-lg bg-layer-2 text-sm">
      <div className="flex items-center justify-between gap-2 pt-1 pr-1 pl-3 text-xs text-fg-tertiary">
        <span className="truncate">{props.title}</span>
        <span className="flex items-center gap-1">
          {props.lang && <span className="font-mono">{props.lang}</span>}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Copy code"
            onClick={() => navigator.clipboard.writeText(props.text).then(() => toast.message('Copied'), () => toast.error('Copy failed'))}
          >
            <Copy />
          </Button>
        </span>
      </div>
      <pre className="overflow-auto p-3 pt-0 font-mono text-xs leading-5">{props.text}</pre>
    </div>
  ),
  Diff: ({ props }) => {
    // ponytail: positional line compare, so one inserted line marks every line after it; a real diff (LCS) if that grates.
    const before = props.before.split('\n')
    const after = props.after.split('\n')
    return (
      <div className="grid grid-cols-2 gap-3 text-sm">
        <DiffSide heading={props.from ?? 'Before'} lines={before} other={after} mark="bg-danger-bg" />
        <DiffSide heading={props.to ?? 'After'} lines={after} other={before} mark="bg-success-bg" />
      </div>
    )
  },
  // 71 §2.5: real code pinned to `path:start`; renders in chat and artifacts too, so the board reader reuses it as the code window.
  Excerpt: ({ props }) => <Excerpt {...props} />,
  Tree: ({ props }) => (
    <div className="flex flex-col gap-0.5 text-sm">
      <TreeRows nodes={nest(props.items)} />
    </div>
  ),
  Glossary: ({ props }) => (
    <dl className="flex flex-col gap-2 text-sm">
      {props.items.map((it, i) => (
        <div key={i}>
          <dt className="font-medium">{it.term}</dt>
          <dd className="text-fg-secondary">{it.text}</dd>
        </div>
      ))}
    </dl>
  ),
  Quote: ({ props }) => (
    <blockquote className="border-l-2 border-border-strong pl-3 text-sm">
      <div className="whitespace-pre-wrap">{props.text}</div>
      {props.source && <div className="mt-1 text-xs text-fg-tertiary">— {props.source}</div>}
    </blockquote>
  ),
  Image: ({ props }) => <Picture {...props} />,
  LinkList: ({ props }) => (
    <div role="list" className="flex flex-col gap-1 text-sm">
      {props.items.map((it, i) => (
        <div key={i} role="listitem" className="flex flex-col">
          <a href={it.url} target="_blank" rel="noopener noreferrer" className="underline underline-offset-2">
            {it.title}
          </a>
          {it.note && <span className="text-xs text-fg-tertiary">{it.note}</span>}
        </div>
      ))}
    </div>
  ),
  Tags: ({ props }) => (
    <div className="flex flex-wrap gap-1.5">
      {props.items.map((it, i) => (
        <Tag key={i} {...it} />
      ))}
    </div>
  ),
  Gauge: ({ props }) => {
    const min = props.min ?? 0
    const max = props.max ?? 100
    const pct = max > min ? Math.round(Math.min(100, Math.max(0, ((props.value - min) / (max - min)) * 100))) : 0
    return (
      <div role="meter" aria-valuenow={props.value} aria-valuemin={min} aria-valuemax={max} className="flex flex-col items-center text-sm">
        <svg viewBox="0 0 100 56" className="w-32">
          <path d="M10 50 A40 40 0 0 1 90 50" fill="none" strokeWidth={8} strokeLinecap="round" className="stroke-layer-2" />
          <path d="M10 50 A40 40 0 0 1 90 50" fill="none" strokeWidth={8} strokeLinecap="round" pathLength={100} strokeDasharray={`${pct} 100`} className="stroke-fg-accent" />
          <text x="50" y="50" textAnchor="middle" className="fill-current text-xl font-semibold tabular-nums">
            {props.value}
          </text>
        </svg>
        {props.label && <div className="text-xs text-fg-tertiary">{props.label}</div>}
      </div>
    )
  },
  Sparkline: ({ props }) => (
    <div className="flex items-center gap-2 text-sm">
      <Spark values={props.values} className="h-8 w-full max-w-64" />
      {props.label && <span className="text-xs text-fg-tertiary">{props.label}</span>}
    </div>
  ),
  Heatmap: ({ props }) => {
    const max = props.max ?? Math.max(0, ...props.cells.flat())
    return (
      <div className="overflow-x-auto">
        <table className="border-separate border-spacing-0.5 text-xs tabular-nums">
          <thead>
            <tr>
              <th />
              {props.cols.map((c, j) => (
                <th key={j} className="px-1 pb-1 font-normal text-fg-secondary">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {props.rows.map((r, i) => (
              <tr key={i}>
                <th scope="row" className="pr-2 text-left font-normal text-fg-secondary">
                  {r}
                </th>
                {props.cells[i]?.map((v, j) => {
                  const pct = max > 0 ? Math.round(Math.min(100, Math.max(0, (v / max) * 100))) : 0
                  return (
                    <td
                      key={j}
                      title={`${r} · ${props.cols[j]}: ${v}`}
                      className="size-8 rounded-sm text-center"
                      style={{ backgroundColor: `color-mix(in oklab, var(--color-fg-accent) ${pct}%, transparent)` }}
                    >
                      {v}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    )
  },
  Person: ({ props }) => (
    <div className="flex items-center gap-3 text-sm">
      <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-layer-2 text-xs font-semibold" aria-hidden>
        {props.name
          .split(/\s+/)
          .filter(Boolean)
          .slice(0, 2)
          .map((w) => w[0]!.toUpperCase())
          .join('')}
      </div>
      <div className="flex min-w-0 flex-col">
        <div className="font-medium">{props.name}</div>
        {props.role && <div className="text-xs text-fg-secondary">{props.role}</div>}
        {props.contact && <div className="text-xs text-fg-tertiary">{props.contact}</div>}
      </div>
    </div>
  ),
  FileList: ({ props }) => (
    <div role="list" className="flex flex-col text-sm">
      {props.items.map((it, i) => {
        const dir = it.kind === 'dir' || it.path.endsWith('/')
        const Icon = dir ? Folder : File
        return (
          <div key={i} role="listitem" className="flex items-center gap-2 py-0.5">
            <Icon className="size-4 shrink-0 text-fg-tertiary" aria-label={dir ? 'folder' : 'file'} />
            <span className="min-w-0 flex-1 truncate font-mono text-xs">{it.path}</span>
            {it.kind && <span className="text-xs text-fg-tertiary">{it.kind}</span>}
            {it.size && <span className="text-xs text-fg-tertiary tabular-nums">{it.size}</span>}
          </div>
        )
      })}
    </div>
  ),
  Kanban: ({ props }) => (
    <div className="flex gap-3 overflow-x-auto text-sm">
      {props.columns.map((col, i) => (
        <div key={i} className="flex min-w-48 flex-1 flex-col gap-2 rounded-xl bg-layer-2 p-2">
          <div className="flex items-baseline justify-between px-1">
            <span className="font-medium">{col.title}</span>
            <span className="text-xs text-fg-tertiary tabular-nums">{col.cards.length}</span>
          </div>
          {col.cards.map((c, j) => (
            <div key={j} className="flex flex-col items-start gap-1 rounded-lg border border-border-subtle bg-layer-1 p-2">
              <div>{c.title}</div>
              {c.detail && <div className="text-xs text-fg-secondary">{c.detail}</div>}
              {c.tag && <Tag text={c.tag} variant="secondary" />}
            </div>
          ))}
        </div>
      ))}
    </div>
  ),
  Calendar: ({ props }) => (
    <div className="flex flex-col gap-2 text-sm">
      {props.title && <div className="font-medium">{props.title}</div>}
      <div className="flex flex-wrap gap-1.5">
        {props.days.map((d, i) => (
          <div key={i} className={cn('flex min-w-12 flex-col items-center gap-0.5 rounded-lg border border-border-subtle p-1.5 text-center', d.state === 'active' && 'border-fg-accent')}>
            <span className="font-semibold tabular-nums">{d.day}</span>
            {d.label && <span className="text-xs text-fg-tertiary">{d.label}</span>}
            <span className={cn('size-2 rounded-full border-2', DOT[d.state ?? 'pending'])} aria-label={d.state ?? 'pending'} />
          </div>
        ))}
      </div>
    </div>
  ),
}

/** json-render registry over the v0 catalog; the only thing `JsonRenderFence` needs. */
export const { registry } = defineRegistry(catalog, { components })
