// The composer's uploads in flight: each chosen, pasted or dropped file holds a tile in the held strip from the moment
// it is chosen until its `put_attachment` lands (the thumbnail replaces it) or fails (the tile turns into an error with
// retry and remove). On a phone the base64 + RPC round trip of a photo takes seconds; without the tile nothing showed.

import { useState, useSyncExternalStore } from 'react'

/** One file on its way: `preview` is a local blob URL of an image (shown dimmed under the spinner), revoked when it goes. */
export type Upload = { id: string; name: string; preview?: string; status: 'pending' | 'error'; error?: string }

/** What an upload resolves to: the engine path and, for an image, its pixel size. */
export type Uploaded = { path: string; dims?: [number, number] }

const isRasterImage = (type: string) => type.startsWith('image/') && type !== 'image/svg+xml'

/** A tile's name: the file's, else what it is (a pasted image has none). */
export const uploadName = (file: Blob & { name?: string }): string => file.name || (isRasterImage(file.type) ? 'Pasted image' : 'File')

/** Why Send waits, or `null` when nothing is uploading. */
export function uploadWait(uploads: readonly Upload[]): string | null {
  const n = uploads.filter((u) => u.status === 'pending').length
  return n === 0 ? null : n === 1 ? 'Waiting for the attachment to upload' : `Waiting for ${n} attachments to upload`
}

type Entry = Upload & { file: Blob; run: number }

/**
 * The uploads list, framework-free. `start` puts each file's tile in synchronously, then runs `upload` per file, so
 * several files land one by one. `cancel` drops a tile (an RPC cannot be aborted: its late result is ignored), `retry`
 * runs a failed one again. `done` gets each landed file; a cancelled or retried-over run never reaches it.
 */
export function createUploads(upload: (file: Blob) => Promise<Uploaded>, done: (u: Uploaded) => void) {
  let list: Entry[] = []
  let view: readonly Upload[] = []
  const subs = new Set<() => void>()
  let seq = 0
  const emit = () => {
    view = list.map(({ file: _f, run: _r, ...u }) => u)
    for (const s of subs) s()
  }
  const revoke = (e: Entry) => e.preview && URL.revokeObjectURL(e.preview)
  const go = (id: string) => {
    const e = list.find((x) => x.id === id)
    if (!e) return
    const run = ++e.run
    const live = () => list.find((x) => x.id === id && x.run === run)
    upload(e.file).then(
      (r) => {
        const cur = live()
        if (!cur) return
        list = list.filter((x) => x !== cur)
        revoke(cur)
        emit()
        done(r)
      },
      (err: unknown) => {
        const cur = live()
        if (!cur) return
        list = list.map((x) => (x === cur ? { ...x, status: 'error', error: err instanceof Error ? err.message : String(err) } : x))
        emit()
      },
    )
  }
  return {
    get: () => view,
    subscribe(fn: () => void) {
      subs.add(fn)
      return () => void subs.delete(fn)
    },
    start(files: Blob[]) {
      const added = files.map((file): Entry => {
        const image = isRasterImage(file.type) && typeof URL.createObjectURL === 'function'
        return { id: `u${++seq}`, name: uploadName(file), preview: image ? URL.createObjectURL(file) : undefined, status: 'pending', file, run: 0 }
      })
      if (!added.length) return
      list = [...list, ...added]
      emit()
      for (const e of added) go(e.id)
    },
    cancel(id: string) {
      const e = list.find((x) => x.id === id)
      if (!e) return
      list = list.filter((x) => x !== e)
      revoke(e)
      emit()
    },
    retry(id: string) {
      if (!list.some((x) => x.id === id && x.status === 'error')) return
      list = list.map((x) => (x.id === id ? { ...x, status: 'pending', error: undefined } : x))
      emit()
      go(id)
    },
    /** Drop everything (the composer unmounts): late results are ignored, previews revoked. */
    clear() {
      for (const e of list) revoke(e)
      list = []
      emit()
    },
  }
}

export type Uploads = ReturnType<typeof createUploads>

/** `createUploads` for a component: `upload` and `done` are read through refs by the caller, so the controller is made once. */
export function useUploads(upload: (file: Blob) => Promise<Uploaded>, done: (u: Uploaded) => void): [readonly Upload[], Uploads] {
  const [ctl] = useState(() => createUploads(upload, done))
  return [useSyncExternalStore(ctl.subscribe, ctl.get), ctl]
}
