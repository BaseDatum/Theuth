import { RpcError } from '@/rpc/client'

/** `{"<tag>": {owner}}` out of a front-door refusal of `code`; `null` for any other error. */
function frontDoor(e: unknown, code: number, tag: string): { owner: string } | null {
  if (!(e instanceof RpcError) || e.code !== code || typeof e.data !== 'object' || e.data === null) return null
  const d = (e.data as Record<string, unknown>)[tag]
  if (typeof d !== 'object' || d === null || !('owner' in d) || typeof d.owner !== 'string') return null
  return { owner: d.owner }
}

/** 183 §4.8 / §13 row 2: `-32013 NotOwner{owner}` — the forward reached an engine that no longer owns the row; the replicated change follows. */
export const notOwner = (e: unknown) => frontDoor(e, -32013, 'NotOwner')

/** 183 §4.3: `-32014 RunsElsewhere{owner}` — a terminal or cell attach whose live output stays on `owner`. */
export const runsElsewhere = (e: unknown) => frontDoor(e, -32014, 'RunsElsewhere')
