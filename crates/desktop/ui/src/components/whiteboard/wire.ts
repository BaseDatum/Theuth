// 218 §4.4: the whiteboard RPCs and event, typed by the generated bindings (`just ts-desktop`). Do not add logic here.
import type { MethodMap } from '@/rpc/bindings/client'
import type { DocKind } from '@/rpc/bindings/event.document_2echanged.output'
import type { RpcClient } from '@/rpc/client'

export type { DocId, SessionId } from '@/rpc/bindings/event.cells_2eevents.output'
export type { WbBlob, WbBlobData, WbSkipped, WbTurnInfo, WbUndoResult, WhiteboardCreated } from '@/rpc/bindings/method.document.output'
export type { WhiteboardSession } from '@/rpc/bindings/method.agent.output'
export type { WbSendContext, WbSentImage } from '@/rpc/bindings/method.agent.input'
export type { WhiteboardViewOpened } from '@/rpc/bindings/event.artifact_2ewhiteboard_2dview_2dopened.output'

export const WHITEBOARD_KIND = 'whiteboard' satisfies DocKind
export const WHITEBOARD_VIEW_OPENED = 'artifact.whiteboard-view-opened@1'

export const WB_METHODS = ['whiteboard_create', 'wb_blob_put', 'wb_blob_get', 'whiteboard_turn', 'whiteboard_undo_turn',
  'whiteboard_bind', 'whiteboard_unbind', 'whiteboard_sessions', 'session_whiteboards', 'whiteboard_send_context'] as const satisfies readonly (keyof MethodMap)[]

type WbMethod = (typeof WB_METHODS)[number]
type P<M extends WbMethod> = MethodMap[M]['params']

export const wbRpc = (c: RpcClient) => {
  const call = <M extends WbMethod>(m: M, params: P<M>) => c.call<MethodMap[M]['result']>(m, params)
  return {
    create: (title: string | null) => call('whiteboard_create', [title]),
    blobPut: (...p: P<'wb_blob_put'>) => call('wb_blob_put', p),
    blobGet: (...p: P<'wb_blob_get'>) => call('wb_blob_get', p),
    turn: (...p: P<'whiteboard_turn'>) => call('whiteboard_turn', p),
    undoTurn: (...p: P<'whiteboard_undo_turn'>) => call('whiteboard_undo_turn', p),
    bind: (...p: P<'whiteboard_bind'>) => call('whiteboard_bind', p),
    unbind: (...p: P<'whiteboard_unbind'>) => call('whiteboard_unbind', p),
    sessions: (...p: P<'whiteboard_sessions'>) => call('whiteboard_sessions', p),
    ofSession: (...p: P<'session_whiteboards'>) => call('session_whiteboards', p),
    sendContext: (...p: P<'whiteboard_send_context'>) => call('whiteboard_send_context', p),
  }
}
