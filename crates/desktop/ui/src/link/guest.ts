// Share links (59 §8.2) and the guest container's boot (§8.6): parse
// `https://<relay>/d#1.<engine>.<doc>.<secret>`, compute the §8.3 join proof.
// The fragment never leaves the page (31 §0.6).

import { b64url, unb64url } from './keys'

const subtle = globalThis.crypto.subtle

/** Everything a share link carries plus the relay the page was served from. */
export type ShareTarget = {
  relayHost: string
  secure: boolean
  engineId: string
  doc: string
  /** The 32-byte link secret; hashed before it is used (`joinProof`). */
  secret: Uint8Array<ArrayBuffer>
}

/** The `/d` page: the bundle booted as a guest container (59 §8.2, 31 §7.7 pattern). */
export const isGuest = (): boolean => typeof location !== 'undefined' && location.pathname.replace(/\/+$/, '') === '/d'

/** `1.<engine>.<doc>.<secret>` (with or without the leading `#`); `null` when malformed. */
export function parseShareLink(fragment: string, relayHost = location.host, secure = location.protocol === 'https:'): ShareTarget | null {
  const parts = fragment.replace(/^#/, '').split('.')
  if (parts.length !== 4 || parts[0] !== '1') return null
  const [, engineId, doc, s] = parts as [string, string, string, string]
  const secret = unb64url(s)
  if (!engineId || !/^[a-z2-7]{26}$/.test(doc) || !secret || secret.length !== 32 || unb64url(engineId)?.length !== 32) return null
  return { relayHost, secure, engineId, doc, secret }
}

/** `HMAC-SHA256(sha256(secret), handshakeHash ‖ guestPub)` as base64url (59 §8.3 as built: the engine holds only the hash). */
export async function joinProof(secret: Uint8Array<ArrayBuffer>, handshakeHash: Uint8Array<ArrayBuffer>, guestPub: Uint8Array<ArrayBuffer>): Promise<string> {
  const key = await subtle.importKey('raw', await subtle.digest('SHA-256', secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign'])
  const msg = new Uint8Array(handshakeHash.length + guestPub.length)
  msg.set(handshakeHash)
  msg.set(guestPub, handshakeHash.length)
  return b64url(new Uint8Array(await subtle.sign('HMAC', key, msg)))
}
