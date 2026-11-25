import type { Id } from './model'

/** Crockford base32, the ULID alphabet (no I, L, O, U). */
const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
const ULID_RE = /^[0-9A-HJKMNP-TV-Z]{26}$/

/** A ULID (§1): 10 chars of 48-bit ms time, then 16 chars of 80 random bits. */
export function ulid(now: number = Date.now()): Id {
  let time = ''
  let t = Math.max(0, Math.floor(now))
  for (let i = 0; i < 10; i++) {
    time = CROCKFORD[t % 32] + time
    t = Math.floor(t / 32)
  }
  const bytes = new Uint8Array(10)
  crypto.getRandomValues(bytes)
  // 80 bits → 16 five-bit groups, most significant first.
  let rand = ''
  let acc = 0
  let bits = 0
  for (const b of bytes) {
    acc = (acc << 8) | b
    bits += 8
    while (bits >= 5) {
      bits -= 5
      rand += CROCKFORD[(acc >> bits) & 31]
    }
    acc &= (1 << bits) - 1
  }
  return time + rand
}

export const isUlid = (s: unknown): s is Id => typeof s === 'string' && ULID_RE.test(s)
