// Relay device-socket frames (`op:u8 ‖ payload`, 31 §4.2) and channel-layer
// records (`flags:u8 ‖ bytes`, FIN = bit 0, §4.3).

import { NOISE_MAX_PLAINTEXT, concat } from './noise'

export const OP_DATA = 0x02
export const OP_CLOSE = 0x03
export const OP_PRESENCE = 0x04

/** `CLOSE` reasons as the relay sends them. */
export const CLOSE_REASON: Record<number, string> = {
  0: 'closed',
  1: 'queue full',
  2: 'idle',
  3: 'refused',
  4: 'harness gone',
}
/** DATA before OPEN, conn limit, or the harness rejected the device. */
export const CLOSE_REFUSED = 3

export type Frame =
  | { op: 'data'; payload: Uint8Array<ArrayBuffer> }
  | { op: 'close'; reason: number }
  | { op: 'presence'; online: boolean }

/** `null` for an unknown op or a truncated frame. */
export function decodeFrame(bytes: Uint8Array<ArrayBuffer>): Frame | null {
  const op = bytes[0]
  if (op === OP_DATA) return { op: 'data', payload: bytes.subarray(1) }
  if (op === OP_CLOSE && bytes.length >= 2) return { op: 'close', reason: bytes[1] as number }
  if (op === OP_PRESENCE && bytes.length >= 2) return { op: 'presence', online: bytes[1] === 1 }
  return null
}

/** A `DATA` frame around one Noise message. */
export const encodeData = (payload: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer> => concat(Uint8Array.of(OP_DATA), payload)

const FIN = 0x01
/** Plaintext bytes per record: the Noise plaintext ceiling less the flags byte. */
export const RECORD_MAX = NOISE_MAX_PLAINTEXT - 1

/** One logical message → records `flags ‖ chunk`, the last flagged FIN. The empty message is one FIN record. */
export function splitRecords(bytes: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer>[] {
  const out: Uint8Array<ArrayBuffer>[] = []
  let at = 0
  do {
    const end = Math.min(bytes.length, at + RECORD_MAX)
    out.push(concat(Uint8Array.of(end === bytes.length ? FIN : 0), bytes.subarray(at, end)))
    at = end
  } while (at < bytes.length)
  return out
}

/** Collects records until FIN; `push` returns the whole message then, else `null`. */
export class Reassembler {
  #parts: Uint8Array<ArrayBuffer>[] = []

  push(record: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer> | null {
    if (record.length === 0) throw new Error('link: empty record')
    this.#parts.push(record.subarray(1))
    if (((record[0] as number) & FIN) === 0) return null
    const whole = this.#parts.length === 1 ? (this.#parts[0] as Uint8Array<ArrayBuffer>) : concat(...this.#parts)
    this.#parts = []
    return whole
  }

  /** Drop a half-received message (connection lost). */
  reset(): void {
    this.#parts = []
  }
}
