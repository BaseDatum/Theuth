// Share links (59 §8.2) and the guest container's boot (§8.6): parse
// `https://<relay>/d#1.<engine>.<doc>.<secret>`, compute the §8.3 join proof.
// The fragment never leaves the page (31 §0.6).

import { b64url, unb64url } from './keys'

const subtle = globalThis.crypto.subtle

/** 184 §7.1 (W-k): how long the guest waits for one front door to open before it tries the next. */
export const HOME_DIAL_MS = 5000

/** Everything a share link carries plus the relay the page was served from. */
export type ShareTarget = {
  relayHost: string
  secure: boolean
  /** 184 §7.1: the front doors to dial, in order: v1 `[link id]`, v2 `[home link id, mesh link id]`. */
  engineIds: string[]
  doc: string
  /** The 32-byte link secret; hashed before it is used (`joinProof`). */
  secret: Uint8Array<ArrayBuffer>
}

/** The `/d` page: the bundle booted as a guest container (59 §8.2, 31 §7.7 pattern). */
export const isGuest = (): boolean => typeof location !== 'undefined' && location.pathname.replace(/\/+$/, '') === '/d'

/** `1.<engine>.<doc>.<secret>` or `2.<home>.<mesh>.<doc>.<secret>` (with or without the leading `#`); `null` when malformed. */
export function parseShareLink(fragment: string, relayHost = location.host, secure = location.protocol === 'https:'): ShareTarget | null {
  const [tag, ...rest] = fragment.replace(/^#/, '').split('.')
  if (rest.length !== (tag === '1' ? 3 : 4) || (tag !== '1' && tag !== '2')) return null
  const s = rest.pop() as string
  const doc = rest.pop() as string
  const secret = unb64url(s)
  if (!/^[a-z2-7]{26}$/.test(doc) || !secret || secret.length !== 32 || rest.some((id) => !id || unb64url(id)?.length !== 32)) return null
  return { relayHost, secure, engineIds: rest, doc, secret }
}

/**
 * 184 §12.6 (W-k): dial `ids[0]`; a connection not `open` within `ms` is closed and the next id is dialled, wrapping
 * (a one-item list redials the same id). `open()` stops the rotation; `lost()` (a later drop) restarts it.
 */
export function dialRotation(ids: string[], dial: (id: string) => { close(): void }, ms = HOME_DIAL_MS) {
  let i = 0
  let conn: { close(): void } | undefined
  let cancel: (() => void) | undefined
  let stopped = false
  const disarm = () => {
    cancel?.()
    cancel = undefined
  }
  const arm = () => {
    disarm()
    const t = setTimeout(() => {
      conn?.close()
      i = (i + 1) % ids.length
      go()
    }, ms)
    cancel = () => clearTimeout(t)
  }
  const go = () => {
    if (stopped) return
    conn = dial(ids[i] as string)
    arm()
  }
  return {
    start: go,
    open: disarm,
    lost: () => {
      if (!stopped && !cancel) arm()
    },
    stop: () => {
      stopped = true
      disarm()
    },
  }
}

/** `doc_join` answers `busy` while the engine is mid-move (§12.6): retry once after 1 s; not a refusal. */
export async function joinBusy<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call()
  } catch (e) {
    if (!(e instanceof Error && e.message.includes('busy'))) throw e
    const { promise, resolve } = Promise.withResolvers<void>()
    setTimeout(resolve, 1000)
    await promise
    return call()
  }
}

/** A `Closed` reason or `doc_join` error that only says the document's server changed (§4.7, §12.5): reconnect, never end the session. Everything else (`not_member`, `viewer_readonly`, `link_*`, …) is final. */
export const serverMoved = (code: string) => code.includes('hub_moved') || code.includes('server_moved') || code.includes('NotHub:')

/** `HMAC-SHA256(sha256(secret), handshakeHash ‖ guestPub)` as base64url (59 §8.3 as built: the engine holds only the hash). */
export async function joinProof(secret: Uint8Array<ArrayBuffer>, handshakeHash: Uint8Array<ArrayBuffer>, guestPub: Uint8Array<ArrayBuffer>): Promise<string> {
  const key = await subtle.importKey('raw', await subtle.digest('SHA-256', secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign'])
  const msg = new Uint8Array(handshakeHash.length + guestPub.length)
  msg.set(handshakeHash)
  msg.set(guestPub, handshakeHash.length)
  return b64url(new Uint8Array(await subtle.sign('HMAC', key, msg)))
}
