// 197 SHAKE-BY-CATEGORY: the contract's TS shapes (§1–§3, §5) until the engine branch lands and the bindings carry them.
// Every line that bridges to the generated bindings ends in `197-adapter`; the integration round replaces this file's
// shapes with the generated ones and drops those lines.
import type { Api as FullApi } from '@/rpc/bindings/client' // 197-adapter
import type * as Out from '@/rpc/bindings/method.agent.output' // 197-adapter

export type ContextOwner = Out.ContextOwner // 197-adapter

/** §1: every inventory item has exactly one kind. */
export type ContextKind = 'image' | 'document' | 'surface' | 'tool' | 'note' | 'peer' | 'user' | 'assistant' | 'summary'

/** §3: 48's targets plus one surface fence of an assistant text part and the source argument of one show/app/page_preview call. */
export type ContextTarget =
  | Out.ContextTarget // 197-adapter
  | { kind: 'fence'; owner: ContextOwner; part: number; index: number }
  | { kind: 'source'; owner: ContextOwner; part: number }

/** §2: `group` is the §1 key; `tokens` what removing this target alone takes out. */
export type ContextItem = {
  target: ContextTarget
  kind: ContextKind
  group: string
  label: string
  detail?: string | null
  tokens: number
  at?: string | null
  remove_with: ContextTarget[]
}

export type ContextSnapshot = Omit<Out.ContextSnapshot, 'items'> & { items: ContextItem[] } // 197-adapter

/** §5: one fence (assistant entries) or source (§5.1: on a show/app/page_preview card's tool entry, its own call's) piece. */
export type ContextPiece = { target: ContextTarget; label: string; removed: boolean }

/** `fences` on assistant entries, in text order; `sources` on tool entries, the one piece of that card's call (§5.1). */
export type ContextEntry = Out.ContextEntry & { fences?: ContextPiece[]; sources?: ContextPiece[] } // 197-adapter

export type ContextRemoval = Out.ContextRemoval & { surfaces: number } // 197-adapter

export type Api = Pick<FullApi, 'context_items' | 'remove_context'> // 197-adapter

/** `context_items` with the extended snapshot. */
export const contextItems = (api: Api, session: string) => api.context_items(session) as unknown as Promise<ContextSnapshot> // 197-adapter

/** `remove_context` accepting fence and source targets. */
export const removeContext = (api: Api, session: string, revision: string, targets: ContextTarget[]) =>
  api.remove_context(session, revision, targets as Out.ContextTarget[]) as Promise<ContextRemoval> // 197-adapter

/** The §5 fields of a stored entry's membership. */
export const pieces = (c: Out.ContextEntry | undefined) => c as ContextEntry | undefined // 197-adapter
