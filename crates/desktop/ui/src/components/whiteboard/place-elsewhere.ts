// 218 "Add to whiteboard…": place a card on a whiteboard that is not open. Holds the shared provider just long enough
// to sync, write one owner transaction and hand the update to the engine; the spot is right of the right-most card.

import type { EngineProvider } from '@/lib/doc/provider'
import { acquire, release } from '@/lib/doc/provider'
import type { RpcClient } from '@/rpc/client'
import type { Api } from '@/rpc/bindings/client'
import type { Artifact } from '@/rpc/bindings/event.artifact_2eshown.output'
import { SIZE } from './add'
import type { Board, CardKind, Id, Source } from './canvas/model'
import { readBoard } from './canvas/model'
import { addCard } from './canvas/ops'

export type PendingCard = { kind: CardKind; title: string; source: Source; size?: [number, number] }

const GAP = 48
const LIVE_WAIT_MS = 15_000
/** Time for the `doc_update` already in flight to leave before the provider may be released. */
const FLUSH_MS = 2_500
/** `excerpt.text` limit (contract §2.2). */
const EXCERPT_MAX = 16_000

/** Right of the right-most live card, top-aligned with it; (0, 0) on an empty whiteboard. */
export function spotOn(board: Pick<Board, 'cards'>): { x: number; y: number } {
  let best: { x: number; y: number } | null = null
  for (const c of Object.values(board.cards)) {
    if (c.state === 'archived') continue
    const right = c.x + c.w
    if (!best || right > best.x) best = { x: right, y: c.y }
  }
  return best ? { x: Math.round(best.x + GAP), y: Math.round(best.y) } : { x: 0, y: 0 }
}

/** The card an artifact makes: a page, an image, or (text kinds) an excerpt; null for kinds a whiteboard cannot show. */
export async function cardFromArtifact(a: Artifact, readText: () => Promise<string>): Promise<PendingCard | null> {
  const title = (a.title || a.name).slice(0, 120)
  if (a.kind === 'html') return { kind: 'page', title, source: { t: 'page', session: a.session, call_id: a.call_id } }
  if (a.kind === 'image' || a.kind === 'svg') return { kind: 'image', title, source: { t: 'image', session: a.session, call_id: a.call_id } }
  if (a.kind === 'markdown' || a.kind === 'text' || a.kind === 'csv' || a.kind === 'mermaid') {
    const text = (await readText()).slice(0, EXCERPT_MAX)
    if (a.kind === 'mermaid') return { kind: 'diagram', title, source: { t: 'diagram', lang: 'mermaid', code: text } }
    const end = Math.max(1, text.split('\n').length)
    return { kind: 'excerpt', title, source: { t: 'excerpt', path: a.path, start: 1, end, text, lang: a.kind === 'markdown' ? 'markdown' : undefined } }
  }
  return null
}

function untilLive(p: EngineProvider): Promise<void> {
  const { promise, resolve, reject } = Promise.withResolvers<void>()
  const check = () => {
    const s = p.status
    if (s.kind === 'live') return resolve()
    if (s.kind === 'readonly') return reject(new Error('This whiteboard is read-only for you'))
    if (s.kind === 'closed') return reject(new Error(`The whiteboard could not be opened (${s.reason})`))
  }
  const off = p.onStatus(check)
  const t = setTimeout(() => reject(new Error('The whiteboard did not open in time')), LIVE_WAIT_MS)
  check()
  return promise.finally(() => {
    off()
    clearTimeout(t)
  })
}

export async function placeOnWhiteboard(o: { client: RpcClient; api: Api; engine: string; doc: string; card: PendingCard }): Promise<Id> {
  const p = acquire(o.client, o.api, o.doc, o.engine)
  try {
    await untilLive(p)
    const [w, h] = o.card.size ?? SIZE[o.card.kind]
    const { x, y } = spotOn(readBoard(p.doc))
    return addCard(p.doc, { kind: o.card.kind, title: o.card.title, x, y, w, h, source: o.card.source })
  } finally {
    setTimeout(() => release(p), FLUSH_MS)
  }
}
