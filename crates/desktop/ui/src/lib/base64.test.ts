import { expect, test } from 'vitest'
import { b64ToUtf8, decode, encode, utf8ToB64 } from './base64'

test('encode is standard base64 with padding, across the chunk boundary', () => {
  const big = new Uint8Array(0x8000 * 2 + 5).map((_, i) => (i * 7) & 0xff)
  expect(encode(big)).toBe(Buffer.from(big).toString('base64'))
  expect(encode(new Uint8Array([0xfb, 0xff]))).toBe('+/8=')
  expect(encode(new Uint8Array())).toBe('')
})

test('decode gives the exact bytes in a tight ArrayBuffer and throws on bad input', () => {
  const bytes = decode(btoa('RIFF\u0000\u00ff'))
  expect([...bytes]).toEqual([82, 73, 70, 70, 0, 255])
  expect(bytes.buffer.byteLength).toBe(6)
  expect(decode('').length).toBe(0)
  expect(() => decode('***')).toThrow()
})

test('a payload over 64 KiB round-trips', () => {
  const big = new Uint8Array(70_000).map((_, i) => (i * 31) & 0xff)
  expect(decode(encode(big))).toEqual(big)
})

test('utf8ToB64 and b64ToUtf8 round-trip multi-byte text', () => {
  const s = 'héllo ✓ 日本 😀'
  expect(utf8ToB64(s)).toBe(Buffer.from(s).toString('base64'))
  expect(b64ToUtf8(utf8ToB64(s))).toBe(s)
})
