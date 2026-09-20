// Noise Protocol (revision 34) initiator for `Noise_IK_25519_AESGCM_SHA256`
// and `Noise_IKpsk2_25519_AESGCM_SHA256` on WebCrypto only (31 §7.2, §4.3).
// Every DH is `deriveBits(X25519)`, so private keys stay inside `CryptoKey`s;
// cipher keys are imported non-extractable.

import { type Identity, generateIdentity } from './keys'

const subtle = globalThis.crypto.subtle
const HASHLEN = 32
const TAG = 16
/** A Noise message is at most 65 535 bytes. */
export const NOISE_MAX = 65_535
/** Largest plaintext one transport message carries. */
export const NOISE_MAX_PLAINTEXT = NOISE_MAX - TAG
const EMPTY = new Uint8Array(0)

/** `left ‖ right`. */
export function concat(...parts: Uint8Array<ArrayBuffer>[]): Uint8Array<ArrayBuffer> {
  let n = 0
  for (const p of parts) n += p.length
  const out = new Uint8Array(n)
  let at = 0
  for (const p of parts) {
    out.set(p, at)
    at += p.length
  }
  return out
}

/** `X25519(priv, pub)` → 32 bytes. */
export async function dh(priv: CryptoKey, pub: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
  const pk = await subtle.importKey('raw', pub, { name: 'X25519' }, true, [])
  return new Uint8Array(await subtle.deriveBits({ name: 'X25519', public: pk }, priv, 256))
}

async function hmac(key: Uint8Array<ArrayBuffer>, data: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
  const k = await subtle.importKey('raw', key, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign'])
  return new Uint8Array(await subtle.sign('HMAC', k, data))
}

/**
 * Noise `HKDF(ck, ikm, 3)` = RFC 5869 with `salt = ck`, `info = ""`, written
 * with HMAC rather than `deriveBits(HKDF)`: `Split()` calls it with an empty
 * `ikm`, and a zero-length HKDF key import is not portable across WebKit versions.
 * Always yields three outputs; callers needing two ignore the last (same bytes as `n = 2`).
 */
async function hkdf(ck: Uint8Array<ArrayBuffer>, ikm: Uint8Array<ArrayBuffer>): Promise<[Uint8Array<ArrayBuffer>, Uint8Array<ArrayBuffer>, Uint8Array<ArrayBuffer>]> {
  const temp = await hmac(ck, ikm)
  const o1 = await hmac(temp, Uint8Array.of(1))
  const o2 = await hmac(temp, concat(o1, Uint8Array.of(2)))
  const o3 = await hmac(temp, concat(o2, Uint8Array.of(3)))
  return [o1, o2, o3]
}

/** AES-GCM under one key with Noise's u64 nonce; `additionalData` is the caller's. */
export class CipherState {
  #key: CryptoKey | null = null
  #n = 0n

  /** `InitializeKey(k)`; `k` is imported non-extractable and dropped. */
  async init(k: Uint8Array<ArrayBuffer>): Promise<void> {
    this.#key = await subtle.importKey('raw', k, { name: 'AES-GCM' }, false, ['encrypt', 'decrypt'])
    this.#n = 0n
  }

  /** `HasKey()`. */
  get hasKey(): boolean {
    return this.#key !== null
  }

  #nonce(): Uint8Array<ArrayBuffer> {
    if (this.#n >= 0xffff_ffff_ffff_ffffn) throw new Error('noise: nonce exhausted')
    const iv = new Uint8Array(12)
    new DataView(iv.buffer).setBigUint64(4, this.#n++, false)
    return iv
  }

  /** `EncryptWithAd(ad, plaintext)`; without a key the plaintext is returned as is. */
  async encrypt(ad: Uint8Array<ArrayBuffer>, plaintext: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
    if (!this.#key) return plaintext
    if (plaintext.length > NOISE_MAX_PLAINTEXT) throw new Error('noise: plaintext too long')
    const ct = await subtle.encrypt({ name: 'AES-GCM', iv: this.#nonce(), additionalData: ad, tagLength: 128 }, this.#key, plaintext)
    return new Uint8Array(ct)
  }

  /** `DecryptWithAd(ad, ciphertext)`; throws on a bad tag (the nonce still advances, per spec the state is then unusable). */
  async decrypt(ad: Uint8Array<ArrayBuffer>, ciphertext: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
    if (!this.#key) return ciphertext
    if (ciphertext.length > NOISE_MAX) throw new Error('noise: ciphertext too long')
    const pt = await subtle.decrypt({ name: 'AES-GCM', iv: this.#nonce(), additionalData: ad, tagLength: 128 }, this.#key, ciphertext)
    return new Uint8Array(pt)
  }
}

/** `SymmetricState`: chaining key, handshake hash and the handshake cipher. */
export class SymmetricState {
  ck!: Uint8Array<ArrayBuffer>
  h!: Uint8Array<ArrayBuffer>
  readonly cs = new CipherState()

  /** `InitializeSymmetric(protocol_name)`. */
  static async init(protocolName: string): Promise<SymmetricState> {
    const s = new SymmetricState()
    const name = new TextEncoder().encode(protocolName)
    if (name.length <= HASHLEN) {
      s.h = new Uint8Array(HASHLEN)
      s.h.set(name)
    } else s.h = new Uint8Array(await subtle.digest('SHA-256', name))
    s.ck = s.h
    return s
  }

  async mixKey(ikm: Uint8Array<ArrayBuffer>): Promise<void> {
    const [ck, k] = await hkdf(this.ck, ikm)
    this.ck = ck
    await this.cs.init(k)
  }

  async mixHash(data: Uint8Array<ArrayBuffer>): Promise<void> {
    this.h = new Uint8Array(await subtle.digest('SHA-256', concat(this.h, data)))
  }

  async mixKeyAndHash(ikm: Uint8Array<ArrayBuffer>): Promise<void> {
    const [ck, th, k] = await hkdf(this.ck, ikm)
    this.ck = ck
    await this.mixHash(th)
    await this.cs.init(k)
  }

  async encryptAndHash(plaintext: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
    const ct = await this.cs.encrypt(this.h, plaintext)
    await this.mixHash(ct)
    return ct
  }

  async decryptAndHash(ciphertext: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
    const pt = await this.cs.decrypt(this.h, ciphertext)
    await this.mixHash(ciphertext)
    return pt
  }

  /** `Split()` → `(c1, c2)`: the initiator sends with `c1`, receives with `c2`. */
  async split(): Promise<[CipherState, CipherState]> {
    const [k1, k2] = await hkdf(this.ck, EMPTY)
    const c1 = new CipherState()
    const c2 = new CipherState()
    await c1.init(k1)
    await c2.init(k2)
    return [c1, c2]
  }
}

/** The transport keys once both handshake messages are processed, plus msg2's decrypted payload. */
export type Session = { send: CipherState; recv: CipherState; payload: Uint8Array<ArrayBuffer>; handshakeHash: Uint8Array<ArrayBuffer> }

/** Inputs shared by both patterns. `ephemeral` is a test seam; production generates one per handshake. */
export type HandshakeInput = {
  /** Our static keypair. */
  s: Identity
  /** The responder's static public key (32 bytes). */
  rs: Uint8Array<ArrayBuffer>
  prologue: Uint8Array<ArrayBuffer>
  /** Payload of message 1. */
  payload?: Uint8Array<ArrayBuffer>
  ephemeral?: Identity
}

/** Half-done handshake: message 1 is built, `finish` consumes message 2. */
export type Initiator = { message: Uint8Array<ArrayBuffer>; finish(msg2: Uint8Array<ArrayBuffer>): Promise<Session> }

/**
 * `-> e, es, s, ss` then `<- e, ee, se [, psk]` from the initiator's side.
 * `psk` present selects `IKpsk2`; the pre-message `<- s` is `rs`.
 */
async function ik(input: HandshakeInput, psk?: Uint8Array<ArrayBuffer>): Promise<Initiator> {
  const ss = await SymmetricState.init(psk ? 'Noise_IKpsk2_25519_AESGCM_SHA256' : 'Noise_IK_25519_AESGCM_SHA256')
  await ss.mixHash(input.prologue)
  await ss.mixHash(input.rs)
  const e = input.ephemeral ?? (await generateIdentity())
  // -> e
  await ss.mixHash(e.pub)
  if (psk) await ss.mixKey(e.pub)
  // es
  await ss.mixKey(await dh(e.priv, input.rs))
  // s
  const encS = await ss.encryptAndHash(input.s.pub)
  // ss
  await ss.mixKey(await dh(input.s.priv, input.rs))
  const message = concat(e.pub, encS, await ss.encryptAndHash(input.payload ?? EMPTY))
  if (message.length > NOISE_MAX) throw new Error('noise: message too long')
  return {
    message,
    async finish(msg2) {
      if (msg2.length < HASHLEN + TAG) throw new Error('noise: short message')
      const re = msg2.subarray(0, HASHLEN)
      // <- e
      await ss.mixHash(re)
      if (psk) await ss.mixKey(re)
      // ee
      await ss.mixKey(await dh(e.priv, re))
      // se (initiator: DH(s, re))
      await ss.mixKey(await dh(input.s.priv, re))
      if (psk) await ss.mixKeyAndHash(psk)
      const payload = await ss.decryptAndHash(msg2.subarray(HASHLEN))
      const [send, recv] = await ss.split()
      return { send, recv, payload, handshakeHash: ss.h }
    },
  }
}

/** `Noise_IK_25519_AESGCM_SHA256` initiator (session, 31 §4.3). */
export const handshakeIK = (input: HandshakeInput): Promise<Initiator> => ik(input)

/** `Noise_IKpsk2_25519_AESGCM_SHA256` initiator (pairing, 31 §4.3); `psk` is the 32-byte pairing secret. */
export const handshakeIKpsk2 = (input: HandshakeInput, psk: Uint8Array<ArrayBuffer>): Promise<Initiator> => ik(input, psk)

/** `"theuth-link-1" ‖ harness id` (ASCII). */
export const prologue = (harnessId: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(`theuth-link-1${harnessId}`)
