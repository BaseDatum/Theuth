import { useState } from 'react'
import { Count, Row, refreshInfo, useAction } from '@/components/settings/shared'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'

const HOUR = 3600
const DAY = 24 * HOUR
/** The engine's default when `info` predates the setting: 14 days. */
const DEFAULT_AFTER = 14 * DAY
export type Unit = 'hours' | 'days'
const UNIT_SECS: Record<Unit, number> = { hours: HOUR, days: DAY }

/** `agent.auto_archive.after` as the field shows it: whole days when it is some, else hours (at least 1). */
export function splitAfter(secs: number): { n: number; unit: Unit } {
  if (secs > 0 && secs % DAY === 0) return { n: secs / DAY, unit: 'days' }
  return { n: Math.max(1, Math.round(secs / HOUR)), unit: 'hours' }
}

/** Settings › Agent › Loop › Chats: `agent.auto_archive` — the switch and how long a chat waits, in hours or days. */
export function AutoArchive() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const enabled = info?.auto_archive_enabled ?? false
  const secs = info?.auto_archive_after_secs || DEFAULT_AFTER
  const shown = splitAfter(secs)
  // The unit the user picked wins over the derived one until the value no longer fits it (48 h shows as 2 days otherwise).
  const [picked, setPicked] = useState<Unit | null>(null)
  const unit = picked === 'hours' ? 'hours' : shown.unit
  const n = unit === shown.unit ? shown.n : Math.round(secs / UNIT_SECS[unit])
  const save = useAction((next: { enabled: boolean; after_secs: number }) =>
    api.set_auto_archive(next.enabled, next.after_secs).then(() => refreshInfo(api)),
  )
  const setCount = { mutate: (v: number) => save.mutate({ enabled, after_secs: v * UNIT_SECS[unit] }), isPending: save.isPending }
  return (
    <>
      <Row
        label="Auto-archive inactive chats"
        description="Archives a chat when it and its subagents have had no activity for the time below. Desks, pinned chats and chats with a running turn, job or terminal stay. Archived chats can be unarchived from the sidebar."
      >
        <Switch
          aria-label="Auto-archive inactive chats"
          checked={enabled}
          disabled={!info || save.isPending}
          onCheckedChange={(on) => save.mutate({ enabled: on, after_secs: secs })}
        />
      </Row>
      <Row label="Archive after" description="How long a chat must be inactive. Checked every hour and right after a change here.">
        <Count aria-label="Archive after" value={info ? n : undefined} min={1} disabled={!enabled} action={setCount} />
        <Select
          value={unit}
          disabled={!info || !enabled || save.isPending}
          onValueChange={(v) => {
            const next = v === 'days' ? 'days' : 'hours'
            setPicked(next)
            if (next !== unit) save.mutate({ enabled, after_secs: n * UNIT_SECS[next] })
          }}
        >
          <SelectTrigger className="w-28" aria-label="Archive after unit">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="hours">hours</SelectItem>
            <SelectItem value="days">days</SelectItem>
          </SelectContent>
        </Select>
      </Row>
    </>
  )
}
