// Attachment bytes over RPC (31 §8.2, §9): every container uploads through
// `put_attachment`; rendering uses `asset://` only when the file is on this
// Mac (local daemon), otherwise `get_attachment` → blob URL, 50 cached.

import { convertFileSrc } from '@tauri-apps/api/core'
import { LOCAL } from '@/link/harnesses'
import type { Api } from '@/rpc/api'
import { container } from './platform'

const MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  svg: 'image/svg+xml',
  pdf: 'application/pdf',
}

/** Lower-case extension without the dot; `''` when none. */
export const extOf = (name: string): string => {
  const i = name.lastIndexOf('.')
  return i < 0 ? '' : name.slice(i + 1).toLowerCase()
}

/** `84 KB`, `1.2 MB`, `312 B`. */
export const fmtBytes = (n: number): string =>
  n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`

/** `put_attachment` ext for a file: from its name, else from its MIME type (pasted images have no name). */
export const extFor = (file: Blob & { name?: string }): string => {
  const byName = file.name ? extOf(file.name) : ''
  if (byName) return byName
  const sub = file.type.split('/')[1] ?? ''
  return sub === 'jpeg' ? 'jpg' : sub
}

const toBase64 = (bytes: Uint8Array): string => {
  let s = ''
  // 32 KiB chunks keep `fromCharCode` under the argument-count limit.
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

/** Upload `file` and get back the harness-side path for `send_input.files`. */
export async function putAttachment(api: Api, file: Blob, ext: string): Promise<string> {
  return api.put_attachment(toBase64(new Uint8Array(await file.arrayBuffer())), ext)
}

/** The file is on this machine (macOS shell talking to the local daemon): paths open, reveal and `asset://` work. */
export const isLocal = (active: string): boolean => container() === 'macos' && active === LOCAL

const LRU_MAX = 50
/** Blob URLs by harness path, insertion-ordered; re-inserted on hit so the oldest is first. */
const lru = new Map<string, Promise<string>>()

/**
 * A URL an `<img>` can load for `path` on the harness: `asset://` (synchronous)
 * when the file is on this machine (macOS shell, local daemon), else the bytes
 * come over RPC and live in a blob URL. Failures are not cached.
 */
export function resolveFile(api: Api, path: string, active: string): string | Promise<string> {
  if (isLocal(active)) return convertFileSrc(path)
  const hit = lru.get(path)
  if (hit) {
    lru.delete(path)
    lru.set(path, hit)
    return hit
  }
  const p = api.get_attachment(path).then((b64) => {
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

const texts = new Map<string, Promise<string>>()

/** UTF-8 text of a harness file via `get_attachment` (works locally and through the relay); cached, failures are not. */
export function readText(api: Api, path: string): Promise<string> {
  const hit = texts.get(path)
  if (hit) return hit
  const p = api.get_attachment(path).then((b64) => new TextDecoder().decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))))
  p.catch(() => texts.delete(path))
  texts.set(path, p)
  // ponytail: unbounded by count, keyed by content-addressed path; an LRU when a session shows hundreds of documents.
  return p
}
