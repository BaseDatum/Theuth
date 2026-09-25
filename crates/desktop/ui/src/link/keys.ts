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

/** By IndexedDB key: `device` (engines) and one per mesh id (69 decision 9). */
const identities: Record<string, Promise<Identity>> = {}

/** IndexedDB row: PKCS#8 private key bytes and the raw public key. Older rows hold an `Identity` (a `CryptoKey`). */
type StoredIdentity = { pkcs8: ArrayBuffer; pub: Uint8Array<ArrayBuffer> }

const X25519 = { name: 'X25519' }

/** `priv` can still derive: WKWebView wraps a serialized `CryptoKey` with a master key that does not always survive a relaunch on iOS. */
async function usable({ priv, pub }: Identity): Promise<boolean> {
  try {
    await subtle.deriveBits({ ...X25519, public: await subtle.importKey('raw', pub, X25519, false, []) }, priv, 256)
    return true
  } catch {
    return false
  }
}

/**
 * The device identity from IndexedDB (`identity`/`device`; `identity/<mesh id>`
 * for a mesh row, so this device is a different identity to each mesh), created
 * on first call. At rest the private key is PKCS#8 bytes (a `CryptoKey` row did
 * not survive an iOS relaunch, which looked like a lost pairing); in memory it is
 * a non-extractable `CryptoKey`. A legacy `CryptoKey` row is kept while it works.
 */
export function getIdentity(meshId?: string): Promise<Identity> {
  const key = meshId ?? 'device'
  return (identities[key] ??= load(key))
}

async function load(key: string): Promise<Identity> {
  const stored = await idbGet<Identity | StoredIdentity>('identity', key).catch(() => undefined)
  if (stored && 'pkcs8' in stored) {
    return { priv: await subtle.importKey('pkcs8', stored.pkcs8, X25519, false, ['deriveBits']), pub: stored.pub }
  }
  if (stored && (await usable(stored))) return stored
  const kp = (await subtle.generateKey(X25519, true, ['deriveBits'])) as CryptoKeyPair
  const row: StoredIdentity = {
    pkcs8: await subtle.exportKey('pkcs8', kp.privateKey),
    pub: new Uint8Array(await subtle.exportKey('raw', kp.publicKey)),
  }
  await idbPut('identity', key, row)
  return { priv: await subtle.importKey('pkcs8', row.pkcs8, X25519, false, ['deriveBits']), pub: row.pub }
}
