import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SessionOrphan } from '@/rpc/bindings/SessionOrphan'
import { useConnection } from '@/store/connection'

/** Log rows in one orphaned range; `bigint` on the wire type, a plain count here. */
export const orphanCount = (o: SessionOrphan): number => Number(o.to_seq) - Number(o.from_seq) + 1

/** `N turns from <time> were not delivered from <engine>` (68 §4.3) over every range; `''` when there are none. */
export function orphansText(orphans: readonly SessionOrphan[], engine: string): string {
  if (orphans.length === 0) return ''
  const n = orphans.reduce((sum, o) => sum + orphanCount(o), 0)
  const first = orphans.reduce((a, b) => (a.at <= b.at ? a : b))
  return `${n} ${n === 1 ? 'turn' : 'turns'} from ${when(first.at)} ${n === 1 ? 'was' : 'were'} not delivered from ${engine}`
}

const when = (rfc3339: string): string => {
  const t = new Date(rfc3339)
  return Number.isNaN(t.getTime()) ? rfc3339 : t.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

/**
 * Top of the transcript (68 §4.3): turns this engine wrote as a superseded owner and never shipped. Nothing is merged;
 * "view" opens a read-only list of the ranges (time, epoch, count).
 */
export function OrphansBanner({ session }: { session: string }) {
  const api = useApi()
  const engine = useConnection((s) => s.info?.name) || 'this node'
  const { data: orphans = [] } = useQuery({ queryKey: queryKeys.sessionOrphans(session), queryFn: () => api.session_orphans(session) })
  const [open, setOpen] = useState(false)
  if (orphans.length === 0) return null
  return (
    <div className="mx-auto flex w-full shrink-0 items-center gap-2 px-4 py-1 text-xs md:max-w-[768px]">
      <span role="status" className="min-w-0 flex-1 truncate text-warning">
        {orphansText(orphans, engine)}
      </span>
      <button type="button" className="shrink-0 underline underline-offset-2 hover:text-foreground" onClick={() => setOpen(true)}>
        view
      </button>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent className="w-full overflow-y-auto sm:w-[480px] sm:max-w-[480px]">
          <SheetHeader>
            <SheetTitle>Undelivered turns</SheetTitle>
            <SheetDescription>
              Written on {engine} while it owned this session, superseded by another node before they shipped. They
              stay here for viewing and are never merged.
            </SheetDescription>
          </SheetHeader>
          <table className="mx-4 mb-4 text-sm">
            <thead className="text-left text-xs text-fg-tertiary">
              <tr>
                <th className="pr-4 font-normal">from</th>
                <th className="pr-4 font-normal">epoch</th>
                <th className="font-normal">turns</th>
              </tr>
            </thead>
            <tbody>
              {orphans.map((o) => (
                <tr key={`${o.epoch}:${o.from_seq}`}>
                  <td className="pr-4 py-1">{when(o.at)}</td>
                  <td className="pr-4 py-1 tabular-nums">{o.epoch}</td>
                  <td className="py-1 tabular-nums">{orphanCount(o)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </SheetContent>
      </Sheet>
    </div>
  )
}
