import { type Components, defineRegistry } from '@json-render/react'
import { ArrowDown, ArrowRight, ArrowUp, CircleCheck, CircleX, Info, Square, SquareCheck, TriangleAlert } from 'lucide-react'
import { Children } from 'react'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Separator } from '@/components/ui/separator'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import { catalog } from './catalog'
import { GraphView } from './Graph'

// Every string is a React text node — no `dangerouslySetInnerHTML` (design 30 §11). Text, lists and headings are
// `div`s with ARIA roles because the Streamdown wrapper styles `p`/`li`/`h1-h4` descendants for prose.

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

function Metric({ label, value, trend, delta }: { label: string; value: string; trend?: string; delta?: string }) {
  const t = trend ? TREND[trend] : undefined
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <div className="truncate text-xs text-fg-tertiary">{label}</div>
      <div className="flex items-baseline gap-1.5">
        <span className="text-2xl font-semibold tabular-nums">{value}</span>
        {t && <t.Icon className={cn('size-4 shrink-0 self-center', t.className)} aria-label={trend} />}
        {delta && <span className="text-xs text-fg-secondary">{delta}</span>}
      </div>
    </div>
  )
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
  Badge: ({ props }) => {
    const v = props.variant ?? 'default'
    return v in BADGE ? (
      <Badge variant="outline" className={BADGE[v]}>
        {props.text}
      </Badge>
    ) : (
      <Badge variant={v as 'default' | 'secondary' | 'destructive'}>{props.text}</Badge>
    )
  },
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
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border-subtle text-left text-fg-secondary">
            {props.columns.map((c, i) => (
              <th key={i} className="px-2 py-1.5 font-semibold">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {props.rows.map((row, i) => (
            <tr key={i} className="border-b border-border-subtle last:border-0">
              {row.map((cell, j) => (
                <td key={j} className="px-2 py-1.5 align-top">
                  {cell}
                </td>
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
}

/** json-render registry over the v0 catalog; the only thing `JsonRenderFence` needs. */
export const { registry } = defineRegistry(catalog, { components })
