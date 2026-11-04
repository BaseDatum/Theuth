// Attachment bytes over RPC (31 §8.2, §9): every container uploads through
// `put_attachment`; rendering uses `asset://` only when the file is on this
// Mac (local daemon), otherwise `get_attachment` → blob URL, 50 cached. A
// private tree's files (51 §4.2) are always fetched naming their session.

import { convertFileSrc } from '@tauri-apps/api/core'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { toast } from 'sonner'
import { LOCAL } from '@/link/engines'
import type { Api } from '@/rpc/api'
import { type RpcClient, isTransportError } from '@/rpc/client'
import { container, isDesktop } from './platform'
// Call-time only (`ownerOf`): `private.ts` imports `forgetPrivate` back.
import { usePrivate } from './private'

const MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  svg: 'image/svg+xml',
  pdf: 'application/pdf',
  // 144 §10: the audio overview's `overview.mp3` through `get_attachment` must be typed for `<audio>` to play it.
  mp3: 'audio/mpeg',
}

/** Lower-case extension without the dot; `''` when none. */
export const extOf = (name: string): string => {
  const i = name.lastIndexOf('.')
  return i < 0 ? '' : name.slice(i + 1).toLowerCase()
}

/** `84 KB`, `1.2 MB`, `312 B`. */
export const fmtBytes = (n: number): string =>
  n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`

/** MIME subtypes whose `put_attachment` ext differs (45 §7.1: files are named by container). */
const EXT: Record<string, string> = { jpeg: 'jpg', mp4: 'm4a', mpeg: 'mp3' }

/** `put_attachment` ext for a file: from its name, else from its MIME type (pasted images and voice recordings have no name). */
export const extFor = (file: Blob & { name?: string }): string => {
  const byName = file.name ? extOf(file.name) : ''
  if (byName) return byName
  // `audio/webm;codecs=opus` → `webm`.
  const sub = file.type.split(/[/;]/)[1] ?? ''
  return EXT[sub] ?? sub
}

const toBase64 = (bytes: Uint8Array): string => {
  let s = ''
  // 32 KiB chunks keep `fromCharCode` under the argument-count limit.
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

/** Upload `file` and get back the engine-side path for `send_input.files`; a private `session` routes it to its scratch root (51 §4.5). */
export async function putAttachment(api: Api, file: Blob, ext: string, session?: string): Promise<string> {
  const b64 = toBase64(new Uint8Array(await file.arrayBuffer()))
  return session ? api.put_session_attachment(session, b64, ext) : api.put_attachment(b64, ext)
}

/** The file is on this machine (desktop shell talking to the local daemon): paths open, reveal and `asset://` work. */
export const isLocal = (active: string): boolean => isDesktop() && active === LOCAL

const LRU_MAX = 50
/** Blob URLs by engine path, insertion-ordered; re-inserted on hit so the oldest is first. */
const lru = new Map<string, Promise<string>>()

/** A private tree's scratch path (`agent/src/private.rs`: `theuth-private-<128 random bits>`). */
const SCRATCH = /\/theuth-private-[0-9a-f]{32}\//
/** Paths cached under a private session: forgotten with the tree (`forgetPrivate`). */
const owned = new Set<string>()

/**
 * The session a body fetch names (51 §4.2: the engine serves a private tree's files only to its owner): the caller's
 * `session` (only ever a private one — `authorize` fails closed on an unknown durable id), else, for a renderer that
 * carries none (the composer's held strip, queued chips, the attach picker), the owned tree's root when `path` is
 * under a scratch dir. `undefined` keeps the durable one-argument call.
 */
function ownerOf(path: string, session: string | undefined): string | undefined {
  const s = session ?? (SCRATCH.test(path) ? usePrivate.getState().current?.root : undefined)
  if (s) owned.add(path)
  return s
}

const fetchB64 = (api: Api, path: string, session: string | undefined): Promise<string> => {
  const s = ownerOf(path, session)
  return s ? api.get_attachment(path, s) : api.get_attachment(path)
}

/** 51 §4.2: nothing private outlives the tree in the client — drop its cached blob URLs and texts. */
export function forgetPrivate(): void {
  for (const path of owned) {
    void lru.get(path)?.then(URL.revokeObjectURL, () => {})
    lru.delete(path)
    texts.delete(path)
  }
  owned.clear()
}

/**
 * A URL an `<img>` can load for `path` on the engine: `asset://` (synchronous)
 * when the file is on this machine (macOS shell, local daemon) and not private
 * (the asset scope is `data_dir` only), else the bytes come over RPC and live
 * in a blob URL. Failures are not cached.
 */
export function resolveFile(api: Api, path: string, active: string, session?: string): string | Promise<string> {
  if (isLocal(active) && !ownerOf(path, session)) return convertFileSrc(path)
  const hit = lru.get(path)
  if (hit) {
    lru.delete(path)
    lru.set(path, hit)
    return hit
  }
  const p = fetchB64(api, path, session).then((b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))
    return URL.createObjectURL(new Blob([bytes], { type: MIME[extOf(path)] ?? 'application/octet-stream' }))
  })
  p.catch(() => lru.delete(path))
  lru.set(path, p)
  if (lru.size > LRU_MAX) {
    const [oldest, url] = lru.entries().next().value as [string, Promise<string>]
    lru.delete(oldest)
    void url.then(URL.revokeObjectURL, () => {})
  }
  return p
}

/** Bytes of an engine file via `get_attachment`; uncached (pdf.js transfers the buffer to its worker). */
export const readBytes = (api: Api, path: string, session?: string): Promise<Uint8Array<ArrayBuffer>> =>
  fetchB64(api, path, session).then((b64) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))

/**
 * 120 §9b: a URL a *document* frame can load — always the bytes over RPC in a typed `blob:` (`application/pdf`), on the
 * local path too, never `convertFileSrc`. A blob of that type runs no script; the asset protocol sniffs HTML out of
 * mis-named bytes and serves it at the shared `asset://localhost` origin. Images keep `resolveFile`. Uncached: the
 * caller revokes the URL when its frame unmounts.
 */
export const resolveDocument = (api: Api, path: string, session?: string): Promise<string> =>
  readBytes(api, path, session).then((bytes) => URL.createObjectURL(new Blob([bytes], { type: 'application/pdf' })))

const texts = new Map<string, Promise<string>>()

/** UTF-8 text of an engine file via `get_attachment` (works locally and through the relay); cached, failures are not. */
export function readText(api: Api, path: string, session?: string): Promise<string> {
  const hit = texts.get(path)
  if (hit) return hit
  const p = readBytes(api, path, session).then((b) => new TextDecoder().decode(b))
  p.catch(() => texts.delete(path))
  texts.set(path, p)
  // ponytail: unbounded by count, keyed by content-addressed path; an LRU when a session shows hundreds of documents.
  return p
}

/**
 * `readText`, retried once when the first read found the socket down: the read an artifact view issues right after an
 * iPad foregrounds lands in the client's `reconnecting`/probe window and rejects `not connected`; the next `open`
 * repeats it. Any other failure, and a second transport failure, reject to the caller.
 */
export async function readTextOnline(api: Api, client: Pick<RpcClient, 'onState'> | null, path: string, session?: string): Promise<string> {
  try {
    return await readText(api, path, session)
  } catch (e) {
    if (!client || !isTransportError(e)) throw e
    const { promise, resolve } = Promise.withResolvers<void>()
    // `onState` calls back at once with the current state, before `off` is assigned.
    let off: (() => void) | undefined
    let open = false
    off = client.onState((s) => {
      if (s !== 'open') return
      open = true
      off?.()
      resolve()
    })
    if (open) off()
    await promise
    return readText(api, path, session)
  }
}

/**
 * Put `blob` somewhere the user can reach. Browser: a download named `name`. Desktop shell on the local daemon:
 * the daemon's attachment store, revealed in the file manager (the only byte sink the shell has; no fs plugin).
 * ponytail: content-addressed name and fixed folder; `@tauri-apps/plugin-fs` + the `save()` dialog if Sky wants to pick.
 */
export async function saveBlob(api: Api, blob: Blob, name: string, ext: string) {
  if (container() === 'web') {
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${name}.${ext}`
    a.click()
    URL.revokeObjectURL(a.href)
    return
  }
  const path = await putAttachment(api, blob, ext)
  await revealItemInDir(path)
  toast.success(`Saved ${path}`)
}

