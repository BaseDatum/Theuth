// 160 §7.2: presets, the custom range popover (two-month range calendar + quick picks), granularity with the reason a
// choice is disabled.

import { CalendarDays } from 'lucide-react'
import { memo, useState } from 'react'
import type { DateRange } from 'react-day-picker'
import { Calendar } from '@/components/ui/calendar'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useIsMobile } from '@/hooks/use-mobile'
import { DAY_MS, type Gran, GRANS, monday, type Range, zone } from '@/lib/costs/cube'
import { type CostsSearch, granBlock, PRESET_LABEL, PRESETS } from '@/lib/costs/search'
import { Seg } from './ui'

const GRAN_LABEL: Record<Gran, string> = { hour: 'Hour', day: 'Day', week: 'Week', month: 'Month' }

/** The browser-local calendar date of a picker `Date` as a civil day number. */
const dayOfDate = (d: Date): number => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / DAY_MS
const dateOfDay = (day: number): Date => {
  const u = new Date(day * DAY_MS)
  return new Date(u.getUTCFullYear(), u.getUTCMonth(), u.getUTCDate())
}
const fmtDay = (day: number, year: boolean) => dateOfDay(day).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', ...(year && { year: 'numeric' }) })

type Props = { search: CostsSearch; r: Range; gran: Gran; tz: string; now: number; set: (patch: Partial<CostsSearch>) => void }

export const RangeBar = memo(function RangeBar({ search, r, gran, tz, now, set }: Props) {
  const z = zone(tz)
  const mobile = useIsMobile()
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState<DateRange | undefined>()
  const custom = search.range === 'custom'
  const today = Math.floor(z.local(Math.floor(now / 3_600_000)) / 24)
  const fromDay = Math.floor(z.local(Math.floor(r.from / 3_600_000)) / 24)
  const toDay = Math.floor(z.local(Math.max(0, Math.ceil(r.to / 3_600_000) - 1)) / 24)
  /** Apply local days `[a, b]` inclusive. */
  const days = (a: number, b: number) => {
    set({ range: 'custom', from: z.midnight(a), to: z.midnight(b + 1), gran: undefined })
    setOpen(false)
  }
  const t = dateOfDay(today)
  const firstOfMonth = dayOfDate(new Date(t.getFullYear(), t.getMonth(), 1))
  const firstOfLast = dayOfDate(new Date(t.getFullYear(), t.getMonth() - 1, 1))
  const picks: [string, () => void][] = [
    ['This week', () => days(monday(today), today)],
    ['Last week', () => days(monday(today) - 7, monday(today) - 1)],
    ['Last month', () => days(firstOfLast, firstOfMonth - 1)],
  ]
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Seg
        label="Range"
        items={PRESETS.map((p) => ({ value: p, label: PRESET_LABEL[p] }))}
        value={custom ? null : (search.range ?? '30d')}
        onChange={(p) => set({ range: p === '30d' ? undefined : p, from: undefined, to: undefined, gran: undefined })}
      />
      <Popover
        open={open}
        onOpenChange={(o) => {
          setOpen(o)
          if (o) setDraft({ from: dateOfDay(fromDay), to: dateOfDay(toDay) })
        }}
      >
        <PopoverTrigger asChild>
          <button
            type="button"
            className={
              'inline-flex items-center gap-1.5 rounded-lg border bg-card px-2.5 py-1 text-xs hover:text-foreground ' +
              (custom ? 'border-info text-foreground' : 'border-border-strong text-fg-secondary hover:border-fg-tertiary')
            }
          >
            <CalendarDays className="size-3.5" />
            {custom ? `${fmtDay(fromDay, false)} – ${fmtDay(toDay, true)}` : 'Custom'}
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-auto p-0">
          <div className="flex flex-col sm:flex-row">
            <div className="flex flex-row flex-wrap gap-1 border-b border-border p-2 sm:w-32 sm:flex-col sm:border-r sm:border-b-0">
              {picks.map(([label, run]) => (
                <button key={label} type="button" onClick={run} className="rounded-md px-2 py-1 text-left text-xs text-fg-secondary hover:bg-layer-2 hover:text-foreground">
                  {label}
                </button>
              ))}
              <span className="mt-auto hidden px-2 py-1 text-[11px] text-fg-tertiary sm:block">or drag across the chart</span>
            </div>
            <Calendar
              mode="range"
              numberOfMonths={mobile ? 1 : 2}
              defaultMonth={draft?.from ? new Date(draft.from.getFullYear(), draft.from.getMonth() - (mobile ? 0 : 1), 1) : undefined}
              selected={draft}
              onSelect={(d) => {
                setDraft(d)
                if (d?.from && d.to && d.from.getTime() !== d.to.getTime()) days(dayOfDate(d.from), dayOfDate(d.to))
              }}
              disabled={{ after: dateOfDay(today) }}
              weekStartsOn={1}
            />
          </div>
        </PopoverContent>
      </Popover>
      <div className="flex-1" />
      <span className="font-mono text-[10px] tracking-[1.4px] text-fg-tertiary uppercase">by</span>
      <Seg label="Granularity" items={GRANS.map((g) => ({ value: g, label: GRAN_LABEL[g], blocked: granBlock(tz, r, g) }))} value={gran} onChange={(g) => set({ gran: g })} />
    </div>
  )
})
