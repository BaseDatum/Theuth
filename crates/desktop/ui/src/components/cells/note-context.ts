// Owner: U2
// 171 §4.2, §13: what a note code block needs beyond its own props — the document and who is looking. `Editor`
// provides it around BlockNote (React portals of the block node views keep the context).
import { createContext } from 'react'
import type { EngineProvider } from '@/lib/doc/provider'
import type { Api } from '@/rpc/bindings/client'

/**
 * `provider`: the document (id, members for "Changed by Alice", block authors). `guest`: the share-link member's api
 * (`cell_latest` only) — set by the guest app and the notebook guest view, where cells never run (§13.1); `null` on
 * the owner's own devices, which run through the engine connection (`useApi`).
 */
export type NoteHost = { provider: EngineProvider; guest: Api | null }
export const NoteHost = createContext<NoteHost | null>(null)
