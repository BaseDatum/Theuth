// 218 §8.2 / §8.8: the `CanvasHost` the shell gives the canvas — blob cache over `wb_blob_*` (object URLs, revoked on
// dispose), native capture of a window rect (`wb_capture`), `theuth/describe-point` on live page frames, and the
// live budget (4 on iPad and phone, 8 on desktop).

import { describePoint } from '@/lib/page-bootstrap'
import { base64Blob, wbCapture } from '@/lib/wb-capture'
import type { CanvasHost } from './canvas/api'
import type { DocId } from './wire'
import { blobBase64 } from './send/send'

export type BlobRpc = {
  blobGet(doc: DocId, key: string): Promise<{ key: string; mime: string; data: string } | null>
  blobPut(doc: DocId, key: string, mime: string, data: string): Promise<unknown>
}

export type ShellHost = CanvasHost & { dispose(): void }

export function createHost(o: { doc: DocId; rpc: () => BlobRpc | null; touch: boolean }): ShellHost {
  const urls = new Map<string, Promise<string | null>>()
  const made: string[] = []
  const remember = (blob: Blob) => {
    const url = URL.createObjectURL(blob)
    made.push(url)
    return url
  }
  return {
    doc: o.doc,
    liveBudget: o.touch ? 4 : 8,
    blobUrl(key) {
      const hit = urls.get(key)
      if (hit) return hit
      const p = (async () => {
        const rpc = o.rpc()
        if (!rpc) return null
        const r = await rpc.blobGet(o.doc, key).catch(() => null)
        return r ? remember(base64Blob(r.data, r.mime)) : null
      })()
      urls.set(key, p)
      // A miss is not cached: the snapshot may be stored by another client later.
      void p.then((u) => u === null && urls.get(key) === p && urls.delete(key))
      return p
    },
    async blobPut(key, blob) {
      const rpc = o.rpc()
      if (!rpc) throw new Error('not connected')
      await rpc.blobPut(o.doc, key, blob.type || 'image/png', await blobBase64(blob))
      urls.set(key, Promise.resolve(remember(blob)))
    },
    async capture(rect, maxSide) {
      // `width` is the snapshot width in points; the PNG is `width × scale` px wide, so the long side is aimed at `maxSide` px.
      const px = Math.max(rect.width, rect.height) * (globalThis.devicePixelRatio || 1)
      const r = await wbCapture(rect, px > maxSide ? (rect.width * maxSide) / px : undefined)
      return r ? base64Blob(r.png) : null
    },
    describePoint: (frame, x, y) => describePoint(frame, x, y),
    dispose() {
      for (const u of made) URL.revokeObjectURL(u)
      made.length = 0
      urls.clear()
    },
  }
}
