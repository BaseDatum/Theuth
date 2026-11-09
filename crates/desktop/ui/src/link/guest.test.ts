import { createHash, createHmac } from 'node:crypto'
import { afterEach, describe, expect, test, vi } from 'vitest'
import { HOME_DIAL_MS, dialRotation, joinBusy, joinProof, parseShareLink, serverMoved } from './guest'
import { b64url, unb64url } from './keys'

const ENGINE = b64url(new Uint8Array(32).fill(1))
const MESH = b64url(new Uint8Array(32).fill(2))
const DOC = 'agqmmam62fzpdi5jaiikh63n44'
const SECRET = new Uint8Array(32).fill(7)

describe('share links (59 §8.2)', () => {
  test('parses the fragment; the relay is the page origin', () => {
    const t = parseShareLink(`#1.${ENGINE}.${DOC}.${b64url(SECRET)}`, 'relay.example:8461', true)
    expect(t).toMatchObject({ relayHost: 'relay.example:8461', secure: true, engineIds: [ENGINE], doc: DOC })
    expect(t?.secret).toEqual(SECRET)
    expect(parseShareLink(`1.${ENGINE}.${DOC}.${b64url(SECRET)}`, 'h', false)?.doc).toBe(DOC)
  })

  test('refuses malformed fragments', () => {
    for (const bad of [
      '',
      `3.${ENGINE}.${DOC}.${b64url(SECRET)}`,
      `2.${ENGINE}.${DOC}.${b64url(SECRET)}`,
      `2.${ENGINE}.${MESH}.${DOC}.short`,
      `2.${ENGINE}.short.${DOC}.${b64url(SECRET)}`,
      `1.${ENGINE}.${MESH}.${DOC}.${b64url(SECRET)}`,
      `1.${ENGINE}.${DOC}`,
      `1.${ENGINE}.${DOC}.${b64url(SECRET.subarray(1))}`,
      `1.${ENGINE}.NOT-A-DOC.${b64url(SECRET)}`,
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

describe('v2 share links (184 §7.1)', () => {
  test('2.<home>.<mesh>.<doc>.<secret> dials [home, mesh]', () => {
    const t = parseShareLink(`#2.${ENGINE}.${MESH}.${DOC}.${b64url(SECRET)}`, 'h', true)
    expect(t).toMatchObject({ engineIds: [ENGINE, MESH], doc: DOC })
    expect(t?.secret).toEqual(SECRET)
  })
})

describe('dial rotation (184 §12.6)', () => {
  afterEach(() => vi.useRealTimers())
  const setup = (ids: string[]) => {
    vi.useFakeTimers()
    const dialled: string[] = []
    const closed: string[] = []
    const r = dialRotation(ids, (id) => (dialled.push(id), { close: () => closed.push(id) }))
    r.start()
    return { r, dialled, closed }
  }

  test('home never opens: mesh after 5 s, then wraps to home', () => {
    const { dialled, closed } = setup([ENGINE, MESH])
    vi.advanceTimersByTime(HOME_DIAL_MS - 1)
    expect(dialled).toEqual([ENGINE])
    vi.advanceTimersByTime(1)
    expect(dialled).toEqual([ENGINE, MESH])
    expect(closed).toEqual([ENGINE])
    vi.advanceTimersByTime(HOME_DIAL_MS)
    expect(dialled).toEqual([ENGINE, MESH, ENGINE])
  })

  test('an open stops the rotation; a later drop restarts it', () => {
    const { r, dialled } = setup([ENGINE, MESH])
    vi.advanceTimersByTime(HOME_DIAL_MS - 1)
    r.open()
    vi.advanceTimersByTime(60_000)
    expect(dialled).toEqual([ENGINE])
    r.lost()
    vi.advanceTimersByTime(HOME_DIAL_MS)
    expect(dialled).toEqual([ENGINE, MESH])
  })

  test('one id (v1 / single engine) redials the same id, never another', () => {
    const { dialled } = setup([ENGINE])
    vi.advanceTimersByTime(HOME_DIAL_MS * 2)
    expect(dialled).toEqual([ENGINE, ENGINE, ENGINE])
  })

  test('busy join retries once after 1 s and is not a refusal; other errors and a second busy throw', async () => {
    vi.useFakeTimers()
    const call = vi.fn().mockRejectedValueOnce(new Error('busy')).mockResolvedValue('joined')
    const p = joinBusy(call)
    await vi.advanceTimersByTimeAsync(999)
    expect(call).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    await expect(p).resolves.toBe('joined')
    await expect(joinBusy(() => Promise.reject(new Error('link_revoked')))).rejects.toThrow('link_revoked')
    const twice = joinBusy(() => Promise.reject(new Error('busy')))
    const caught = expect(twice).rejects.toThrow('busy')
    await vi.advanceTimersByTimeAsync(1000)
    await caught
  })
})

test('serverMoved: a moved hub/server is retried; every final refusal is not', () => {
  for (const c of ['hub_moved', 'server_moved', 'NotHub:abc', 'rpc error: hub_moved']) expect(serverMoved(c)).toBe(true)
  for (const c of ['not_member', 'viewer_readonly', 'sharing_disabled', 'sharing_off', 'link_revoked', 'link_expired', 'link_unknown', 'bad_proof', 'members_full', 'member_revoked', 'busy']) expect(serverMoved(c)).toBe(false)
})

