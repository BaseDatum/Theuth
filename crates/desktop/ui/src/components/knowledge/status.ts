// 136 §8 Library › Knowledge: the status badge from the seven dreaming keys on `Meta`, and the "changed by dreaming" filter.

import type { Meta } from "@/rpc/bindings/method.knowledge.output"

/**
 * The one badge a concept earns beside its type, or `null` for a plain tidy entry: `superseded` (a `superseded_by`
 * pointer wins over the status), then the non-tidy status (`archived`, `merged`, `stale`, `retired`, `fresh`),
 * then `verified <date>` for a tidy entry with a verification date.
 */
export function statusBadge(m: Pick<Meta, 'status' | 'superseded_by' | 'verified'>): string | null {
  if (m.superseded_by !== null) return 'superseded'
  if (m.status !== 'tidy') return m.status
  if (m.verified !== null) return `verified ${m.verified.slice(0, 10)}`
  return null
}

/** Contract §3.4: `source: dreaming` or a status other than `tidy`. */
export const changedByDreaming = (m: Pick<Meta, 'source' | 'status'>): boolean => m.source === 'dreaming' || m.status !== 'tidy'
