import { idbGet, idbPut } from './idb'

/** The device's X25519 static: a non-extractable private `CryptoKey` and its raw 32-byte public key. */
export type Identity = { priv: CryptoKey; pub: Uint8Array<ArrayBuffer> }

const subtle = globalThis.crypto.subtle

/** base64url without padding (harness ids, pairing strings). */
export function b64url(bytes: Uint8Array<ArrayBuffer>): string {
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '')
}

/** Inverse of `b64url`; `null` on bad input. */
export function unb64url(s: string): Uint8Array<ArrayBuffer> | null {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) return null
  try {
    const bin = atob(s.replaceAll('-', '+').replaceAll('_', '/'))
    return Uint8Array.from(bin, (c) => c.charCodeAt(0))
  } catch {
    return null
  }
}

/** `Device.id` as the harness shows it: the first 8 chars of the base64url public key. */
export const deviceId = (pub: Uint8Array<ArrayBuffer>): string => b64url(pub).slice(0, 8)

/** A fresh X25519 keypair; `extractable` is `false` unless a script needs to persist it as raw bytes. */
export async function generateIdentity(extractable = false): Promise<Identity> {
  const kp = (await subtle.generateKey({ name: 'X25519' }, extractable, ['deriveBits'])) as CryptoKeyPair
  return { priv: kp.privateKey, pub: new Uint8Array(await subtle.exportKey('raw', kp.publicKey)) }
}

/** PKCS#8 header for a raw 32-byte X25519 private key (RFC 8410). */
const PKCS8_X25519 = Uint8Array.from([0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20])

/** Import a raw 32-byte X25519 private key (scripts and tests; the app never sees raw private bytes). */
export async function importPrivate(raw: Uint8Array<ArrayBuffer>, extractable = false): Promise<CryptoKey> {
  const der = new Uint8Array(PKCS8_X25519.length + raw.length)
  der.set(PKCS8_X25519)
  der.set(raw, PKCS8_X25519.length)
  return subtle.importKey('pkcs8', der, { name: 'X25519' }, extractable, ['deriveBits'])
}

let identity: Promise<Identity> | undefined

/** The device identity from IndexedDB (`link`/`identity`), created on first call; the private key never leaves WebCrypto. */
export function getIdentity(): Promise<Identity> {
  identity ??= (async () => {
    const stored = await idbGet<Identity>('identity', 'device')
    if (stored) return stored
    const fresh = await generateIdentity()
    await idbPut('identity', 'device', fresh)
    return fresh
  })()
  return identity
}
