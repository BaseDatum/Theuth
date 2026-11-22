// 208: the graph's legend — a popover from the region's controls, every entry from legend.ts (and so from encoding.ts).
import { Info, MessageSquare } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { EDGE } from './encoding'
import { type LegendSwatch, legendSections } from './legend-entries'
import { StatusChip } from './NodeBody'

const SECTIONS = legendSections()

function Swatch({ s }: { s: LegendSwatch }) {
  switch (s.kind) {
    case 'status':
      return <StatusChip glyph={s.glyph} tone={s.tone} className="size-4" />
    case 'edge': {
      const e = EDGE[s.edge]
      return (
        <svg viewBox="0 0 36 12" className="h-3 w-9 overflow-visible" aria-hidden>
          <path d="M2 6 L31 6" className={e.flow ? 'ag-flow' : undefined} style={{ stroke: e.stroke, strokeWidth: e.width, strokeDasharray: e.dash ?? undefined, strokeLinecap: 'round', opacity: e.opacity, fill: 'none' }} markerEnd={`url(#${e.marker})`} />
        </svg>
      )
    }
    case 'pulse':
      return (
        <svg viewBox="0 0 36 12" className="h-3 w-9" aria-hidden>
          <path d="M2 6 L34 6" style={{ stroke: 'var(--color-border-strong)', strokeWidth: 1.25 }} />
          <circle cx={22} cy={6} r={6} style={{ fill: s.tone, opacity: 0.25 }} />
          <circle cx={22} cy={6} r={3.5} style={{ fill: s.tone }} />
        </svg>
      )
    case 'ring':
      return (
        <span className="relative flex h-4 w-7 items-center rounded-[5px] border border-border-subtle bg-layer-1 ring-2 ring-danger ring-offset-1 ring-offset-background">
          <span className="absolute -top-1.5 -right-1.5 rounded-full bg-danger px-1 text-[9px] leading-3 font-semibold text-white">2</span>
        </span>
      )
    case 'border':
      return <span className="h-4 w-7 rounded-[5px] border-[1.5px] bg-layer-1" style={{ borderColor: s.tone }} />
    case 'opacity':
      return <span className={cn('h-4 w-7 rounded-[5px] border border-border-strong bg-layer-1', s.dashed && 'border-dashed')} style={{ opacity: s.opacity }} />
    case 'card':
      return (
        <span className={cn('flex h-4 w-7 items-center justify-end rounded-[5px] border pr-0.5', s.heavy ? 'border-border-strong bg-elevated' : 'border-border-subtle bg-layer-1', s.dashed && 'border-dashed border-border-strong bg-background')}>
          {s.tag && <span className="text-[6px] leading-none font-semibold tracking-wide text-fg-tertiary uppercase">{s.tag}</span>}
        </span>
      )
    case 'fold':
      return <span className="flex h-4 w-7 items-center justify-center rounded-[5px] border border-dashed border-border-strong bg-layer-1 text-[8px] text-fg-secondary">+8</span>
    case 'badge':
      return (
        <span className="flex items-center gap-0.5 rounded-full border border-border-subtle bg-elevated px-1 text-[9px] leading-3 font-medium text-fg-secondary tabular-nums">
          <MessageSquare className="size-2" />3
        </span>
      )
  }
}

export function LegendPopover() {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button size="xs" variant="ghost" aria-label="Legend" title="What the colours mean" className="gap-1 px-1.5 text-fg-secondary">
          <Info />
          <span className="max-sm:hidden">Legend</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[min(36rem,calc(100vw-1rem))] p-0 text-xs" data-graph-legend="">
        <div className="grid gap-x-5 gap-y-3 p-3 sm:grid-cols-2">
          {SECTIONS.map((sec) => (
            <section key={sec.title} className={cn(sec.title === 'Cards' && 'sm:row-span-2')} aria-label={sec.title}>
              <h3 className="mb-1.5 text-[11px] font-semibold tracking-wide text-fg-tertiary uppercase">{sec.title}</h3>
              <ul className="flex flex-col gap-1.5">
                {sec.entries.map((e) => (
                  <li key={e.id} className="flex items-start gap-2" data-legend={e.id}>
                    <span className="flex h-4 w-9 shrink-0 items-center justify-center pt-px">
                      <Swatch s={e.swatch} />
                    </span>
                    <span className="min-w-0 leading-4">
                      <span className="font-medium text-foreground">{e.label}</span>
                      <span className="text-fg-tertiary"> · {e.detail}</span>
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )
}
