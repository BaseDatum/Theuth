// 218-TEMP(wire): hand-written until `just ts-desktop` runs after B1/B2 land. Do not add logic here.
import type { RpcClient } from '@/rpc/client'

export type DocId = string
export type SessionId = string
export const WHITEBOARD_KIND = 'whiteboard' // 218-TEMP(wire): cast to DocKind / AttachKind at use sites

export type WhiteboardCreated = { doc: DocId; title: string }
export type WbBlob = { key: string; bytes: number }
export type WbBlobData = { key: string; mime: string; data: string }
export type WbTurnInfo = { turn: number; cards: string[]; links: string[]; frames: string[]; writes: number; undone: boolean; ended: boolean }
export type WbSkipped = { id: string; alias: string | null; why: string }
export type WbUndoResult = { turn: number; undone: number; skipped: WbSkipped[] }
export type WhiteboardSession = { session: SessionId; title: string; bound_ms: number }
export type WbSentImage = { card: string | null; marks: string[] }
export type WbSendContext = { in_view?: string[]; selected?: string[]; images?: WbSentImage[]; omitted?: number; omitted_why?: 'no vision' | 'budget' | null }
export type WhiteboardViewOpened = { type: 'WhiteboardViewOpened'; session: SessionId; call_id: string; doc: DocId; title: string }
export const WHITEBOARD_VIEW_OPENED = 'artifact.whiteboard-view-opened@1'

export const WB_METHODS = ['whiteboard_create', 'wb_blob_put', 'wb_blob_get', 'whiteboard_turn', 'whiteboard_undo_turn',
  'whiteboard_bind', 'whiteboard_unbind', 'whiteboard_sessions', 'session_whiteboards', 'whiteboard_send_context'] as const

export const wbRpc = (c: RpcClient) => ({
  create: (title: string | null) => c.call<WhiteboardCreated>('whiteboard_create', [title]),
  blobPut: (doc: DocId, key: string, mime: string, data: string) => c.call<WbBlob>('wb_blob_put', [doc, key, mime, data]),
  blobGet: (doc: DocId, key: string) => c.call<WbBlobData | null>('wb_blob_get', [doc, key]),
  turn: (doc: DocId, session: SessionId) => c.call<WbTurnInfo | null>('whiteboard_turn', [doc, session]),
  undoTurn: (doc: DocId, session: SessionId, turn: number | null) => c.call<WbUndoResult>('whiteboard_undo_turn', [doc, session, turn]),
  bind: (session: SessionId, whiteboard: DocId) => c.call<null>('whiteboard_bind', [session, whiteboard]),
  unbind: (session: SessionId, whiteboard: DocId) => c.call<null>('whiteboard_unbind', [session, whiteboard]),
  sessions: (whiteboard: DocId) => c.call<WhiteboardSession[]>('whiteboard_sessions', [whiteboard]),
  ofSession: (session: SessionId) => c.call<DocId[]>('session_whiteboards', [session]),
  sendContext: (session: SessionId | null, whiteboard: DocId, context: WbSendContext) => c.call<null>('whiteboard_send_context', [session, whiteboard, context]),
})
