import { createHash, createHmac } from 'node:crypto'
import { describe, expect, test } from 'vitest'
import { joinProof, parseShareLink } from './guest'
import { b64url, unb64url } from './keys'

const HARNESS = b64url(new Uint8Array(32).fill(1))
const DOC = 'agqmmam62fzpdi5jaiikh63n44'
const SECRET = new Uint8Array(32).fill(7)

describe('share links (59 §8.2)', () => {
  test('parses the fragment; the relay is the page origin', () => {
    const t = parseShareLink(`#1.${HARNESS}.${DOC}.${b64url(SECRET)}`, 'relay.example:8461', true)
    expect(t).toMatchObject({ relayHost: 'relay.example:8461', secure: true, harnessId: HARNESS, doc: DOC })
    expect(t?.secret).toEqual(SECRET)
    expect(parseShareLink(`1.${HARNESS}.${DOC}.${b64url(SECRET)}`, 'h', false)?.doc).toBe(DOC)
  })

  test('refuses malformed fragments', () => {
    for (const bad of [
      '',
      `2.${HARNESS}.${DOC}.${b64url(SECRET)}`,
      `1.${HARNESS}.${DOC}`,
      `1.${HARNESS}.${DOC}.${b64url(SECRET.subarray(1))}`,
      `1.${HARNESS}.NOT-A-DOC.${b64url(SECRET)}`,
      `1.short.${DOC}.${b64url(SECRET)}`,
    ]) {
      expect(parseShareLink(bad, 'h', true), bad).toBeNull()
    }
  })

  test('the proof is HMAC-SHA256(sha256(secret), h ‖ guest_pub), base64url', async () => {
    const h = new Uint8Array(32).fill(2)
    const pub = new Uint8Array(32).fill(3)
    const proof = await joinProof(SECRET, h, pub)
    const key = createHash('sha256').update(SECRET).digest()
    const expected = createHmac('sha256', key).update(h).update(pub).digest()
    expect(unb64url(proof)).toEqual(new Uint8Array(expected))
    // Bound to the connection and the key: either change, another proof.
    expect(await joinProof(SECRET, new Uint8Array(32).fill(9), pub)).not.toBe(proof)
    expect(await joinProof(SECRET, h, new Uint8Array(32).fill(9))).not.toBe(proof)
  })
})
