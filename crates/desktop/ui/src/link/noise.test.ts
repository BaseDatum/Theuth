// Cacophony's published vectors (`vectors/cacophony.txt`) for the two patterns
// the device runs as initiator. Node 22+ (X25519 in WebCrypto); run under 26.7.
import { createPrivateKey, createPublicKey } from 'node:crypto'
import { expect, test } from 'vitest'
import { importPrivate } from './keys'
import { handshakeIK, handshakeIKpsk2, prologue } from './noise'

const hex = (s: string) => Uint8Array.from(Buffer.from(s, 'hex'))
const toHex = (b: Uint8Array<ArrayBuffer>) => Buffer.from(b).toString('hex')

/** Raw private → `{ priv, pub }` (public derived through node:crypto; WebCrypto cannot). */
async function keypair(raw: string) {
  const priv = await importPrivate(hex(raw))
  const der = Buffer.concat([Buffer.from('302e020100300506032b656e04220420', 'hex'), Buffer.from(raw, 'hex')])
  const pub = createPublicKey(createPrivateKey({ key: der, format: 'der', type: 'pkcs8' })).export({ type: 'spki', format: 'der' })
  return { priv, pub: Uint8Array.from(pub.subarray(-32)) }
}

type Vector = { psk?: string; hash: string; messages: [string, string][] }
const COMMON = {
  prologue: '4a6f686e2047616c74',
  initStatic: 'e61ef9919cde45dd5f82166404bd08e38bceb5dfdfded0a34c8df7ed542214d1',
  initEphemeral: '893e28b9dc6ca8d611ab664754b8ceb7bac5117349a4439a6b0569da977c464a',
  respStatic: '31e0303fd6418d2f8c0e78b91f22e8caed0fbe48656dcf4767e4834f701b8f62',
}
const VECTORS: Record<string, Vector> = {
  Noise_IK_25519_AESGCM_SHA256: {
    hash: '669c8640d9e42a3cda2f232f78597ceefb01daa6e3df81181ccce6fc6b5026bf',
    messages: [
      ['4c756477696720766f6e204d69736573', 'ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c79444e417bc55c7a8166c993356c1be41ef67818a292426f301556c7f26b21d25ddb097153891a9a956cff47b83e63ad8d701c1342c209cff1ca5ecd43402762ac249e3bd3a4c0a145fe07cb5dae28ea13a3'],
      ['4d757272617920526f746862617264', '95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f144808843af2ccf9972e22afc67aeafcd25162f7f98c363b7762e3e4cb7d272e39f27a5'],
      ['462e20412e20486179656b', '66acfc92e3197de166809e6d4d5d003dcc819a84bc3522ca53c9d9'],
      ['4361726c204d656e676572', '71f89aa6533a6de70b0826864dd75f60806ee40170c16290189eb3'],
      ['4a65616e2d426170746973746520536179', '4795a3423550c8bf00386bd496a3e2c76c10669d2a75ab8f79b5094c5412a25705'],
      ['457567656e2042f6686d20766f6e2042617765726b', 'aa0bb39097555c918e40be82abc2b909eb79d9eb87adb07e268fc37323a6cf904fd01fb391'],
    ],
  },
  Noise_IKpsk2_25519_AESGCM_SHA256: {
    psk: '54686973206973206d7920417573747269616e20706572737065637469766521',
    hash: '48c3f1eb397c797af6b139032c6074cb41282ff033ee25d5aee4a4badd3a2c3f',
    messages: [
      ['4c756477696720766f6e204d69736573', 'ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c79445a1baf339186d7867062b0dd31f5de322d370165d217939b2b7ed9e2bae7840458ea65c2ce1554e0e8077fe7334c4bd514c658fd04d97b86d216f58f59714a7e24144155045c8f36bed85a2cb0fd9af7'],
      ['4d757272617920526f746862617264', '95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f1448088432fe8d675e949799d2a968211a66cc7ba2b0f4cad8eda3c2bd37eae737bbf88'],
      ['462e20412e20486179656b', '38cbf3835c78b7802df0dd0d9640ff8575debd0cb61cfb52590da1'],
      ['4361726c204d656e676572', 'ffc365ff6955fc66c5d809b31ac995f812c5770bd29f0df12f0ffd'],
      ['4a65616e2d426170746973746520536179', 'a7f2828174c4ead2491ffeb77c41c890fd1229ee36f23ccb9066a795daff1cad6b'],
      ['457567656e2042f6686d20766f6e2042617765726b', '3b86963d5c0e6e3b4153d77153836606393eefc4e5e6dae97796552d1265839ee3825cb80c'],
    ],
  },
}

for (const [name, v] of Object.entries(VECTORS)) {
  test(`${name}: initiator matches the cacophony vector`, async () => {
    const [m1, m2, ...transport] = v.messages
    const input = {
      s: await keypair(COMMON.initStatic),
      ephemeral: await keypair(COMMON.initEphemeral),
      rs: hex(COMMON.respStatic),
      prologue: hex(COMMON.prologue),
      payload: hex(m1![0]),
    }
    const hs = v.psk ? await handshakeIKpsk2(input, hex(v.psk)) : await handshakeIK(input)
    expect(toHex(hs.message)).toBe(m1![1])
    const session = await hs.finish(hex(m2![1]))
    expect(toHex(session.payload)).toBe(m2![0])
    expect(toHex(session.handshakeHash)).toBe(v.hash)
    // Transport messages alternate initiator → responder → …
    for (const [i, [payload, ciphertext]] of transport.entries()) {
      if (i % 2 === 0) expect(toHex(await session.send.encrypt(new Uint8Array(), hex(payload)))).toBe(ciphertext)
      else expect(toHex(await session.recv.decrypt(new Uint8Array(), hex(ciphertext)))).toBe(payload)
    }
  })
}

test('IK: a tampered message 2 fails to decrypt', async () => {
  const v = VECTORS.Noise_IK_25519_AESGCM_SHA256!
  const hs = await handshakeIK({
    s: await keypair(COMMON.initStatic),
    ephemeral: await keypair(COMMON.initEphemeral),
    rs: hex(COMMON.respStatic),
    prologue: hex(COMMON.prologue),
    payload: hex(v.messages[0]![0]),
  })
  const bad = hex(v.messages[1]![1])
  bad[bad.length - 1]! ^= 1
  await expect(hs.finish(bad)).rejects.toThrow()
})

test('prologue is "wth-link-1" followed by the harness id', () => {
  expect(new TextDecoder().decode(prologue('abc'))).toBe('wth-link-1abc')
})
