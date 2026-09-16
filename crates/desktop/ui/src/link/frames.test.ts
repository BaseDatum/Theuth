import { expect, test } from 'vitest'
import { RECORD_MAX, Reassembler, decodeFrame, encodeData, splitRecords } from './frames'

test('device-socket frames decode by op; unknown or truncated frames are null', () => {
  expect(decodeFrame(Uint8Array.of(0x02, 9, 8))).toEqual({ op: 'data', payload: Uint8Array.of(9, 8) })
  expect(decodeFrame(Uint8Array.of(0x03, 3))).toEqual({ op: 'close', reason: 3 })
  expect(decodeFrame(Uint8Array.of(0x04, 1))).toEqual({ op: 'presence', online: true })
  expect(decodeFrame(Uint8Array.of(0x04, 0))).toEqual({ op: 'presence', online: false })
  expect(decodeFrame(Uint8Array.of(0x04))).toBeNull()
  expect(decodeFrame(Uint8Array.of(0x01, 0, 0, 0, 0))).toBeNull()
  expect(decodeFrame(new Uint8Array())).toBeNull()
  expect(encodeData(Uint8Array.of(7))).toEqual(Uint8Array.of(0x02, 7))
})

test('records split at 65 518 plaintext bytes, FIN only on the last, and reassemble', () => {
  expect(RECORD_MAX).toBe(65_518)
  const msg = new Uint8Array(RECORD_MAX * 2 + 1).map((_, i) => i & 0xff)
  const recs = splitRecords(msg)
  expect(recs.map((r) => r.length)).toEqual([RECORD_MAX + 1, RECORD_MAX + 1, 2])
  expect(recs.map((r) => r[0])).toEqual([0, 0, 1])
  const asm = new Reassembler()
  expect(asm.push(recs[0]!)).toBeNull()
  expect(asm.push(recs[1]!)).toBeNull()
  expect(asm.push(recs[2]!)).toEqual(msg)
  // exactly one record's worth is a single FIN record; the empty message too
  expect(splitRecords(new Uint8Array(RECORD_MAX)).length).toBe(1)
  expect(splitRecords(new Uint8Array())).toEqual([Uint8Array.of(1)])
  expect(asm.push(Uint8Array.of(1))).toEqual(new Uint8Array())
})

test('reassembler drops a partial message on reset', () => {
  const asm = new Reassembler()
  asm.push(Uint8Array.of(0, 1, 2))
  asm.reset()
  expect(asm.push(Uint8Array.of(1, 3))).toEqual(Uint8Array.of(3))
})
