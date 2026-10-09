import { type Mock, expect, test, vi } from 'vitest'
import { type PlayerContext, SegmentPlayer, base64ToBytes, padRange, playError } from './segment-audio'

test('padRange adds 150 ms each side and clamps at zero', () => {
  expect(padRange(1000, 2000)).toEqual([850, 2150])
  expect(padRange(100, 400)).toEqual([0, 550])
  expect(padRange(0, 0, 20)).toEqual([0, 20])
})

test('base64ToBytes gives the exact bytes in a tight ArrayBuffer', () => {
  const bytes = base64ToBytes(btoa('RIFF\u0000\u00ff'))
  expect([...bytes]).toEqual([82, 73, 70, 70, 0, 255])
  expect(bytes.buffer.byteLength).toBe(6)
  expect(base64ToBytes('').length).toBe(0)
})

interface StubSource {
  buffer: AudioBuffer | null
  connect: Mock
  start: Mock
  stop: Mock
  onended: (() => void) | null
}

/** A context whose decodes resolve when the test releases them, and whose sources record start/stop. */
function stubContext() {
  const sources: StubSource[] = []
  let release: (() => void)[] = []
  const ctx = {
    resume: vi.fn(async () => {}),
    decodeAudioData: vi.fn((buf: ArrayBuffer) => {
      const { promise, resolve } = Promise.withResolvers<AudioBuffer>()
      release.push(() => resolve({ length: buf.byteLength } as AudioBuffer))
      return promise
    }),
    createBufferSource: vi.fn(() => {
      const src: StubSource = { buffer: null, connect: vi.fn(), start: vi.fn(), stop: vi.fn(), onended: null }
      sources.push(src)
      return src as unknown as AudioBufferSourceNode
    }),
    destination: {} as AudioDestinationNode,
  }
  /** Flush microtasks until a decode is pending, then resolve every pending decode. */
  const decodeAll = async () => {
    for (let i = 0; i < 20 && release.length === 0; i++) await Promise.resolve()
    const r = release
    release = []
    for (const f of r) f()
  }
  return { ctx: ctx as unknown as PlayerContext, resume: ctx.resume, sources, decodeAll }
}

const wav = async () => btoa('wav')

test('toggle resumes the context in the click, marks the row, then plays; onended clears the marker', async () => {
  const { ctx, resume, sources, decodeAll } = stubContext()
  const marks: (number | null)[] = []
  const p = new SegmentPlayer(() => ctx, (m) => marks.push(m))
  const fetch = vi.fn(wav)
  const done = p.toggle(7, 1000, 2000, fetch)
  expect(resume).toHaveBeenCalledTimes(1)
  expect(fetch).toHaveBeenCalledWith(850, 2150)
  expect(marks).toEqual([7])
  expect(p.playing).toBe(7)
  await decodeAll()
  await done
  expect(sources).toHaveLength(1)
  expect(sources[0]!.start).toHaveBeenCalledTimes(1)
  sources[0]!.onended?.()
  expect(marks).toEqual([7, null])
  expect(p.playing).toBeNull()
})

test('clicking the playing row stops it; clicking another row stops the first and plays the second', async () => {
  const { ctx, sources, decodeAll } = stubContext()
  const marks: (number | null)[] = []
  const p = new SegmentPlayer(() => ctx, (m) => marks.push(m))
  const a = p.toggle(1, 0, 500, wav)
  await decodeAll()
  await a
  const b = p.toggle(2, 500, 900, wav)
  expect(sources[0]!.stop).toHaveBeenCalledTimes(1)
  expect(marks).toEqual([1, null, 2])
  await decodeAll()
  await b
  expect(sources).toHaveLength(2)
  expect(sources[1]!.start).toHaveBeenCalled()
  // A late `onended` from the stopped source must not clear the new marker.
  sources[0]!.onended?.()
  expect(p.playing).toBe(2)
  await p.toggle(2, 500, 900, wav)
  expect(sources[1]!.stop).toHaveBeenCalledTimes(1)
  expect(marks.at(-1)).toBeNull()
  expect(p.playing).toBeNull()
})

test('a click that overtakes a pending fetch drops the stale result instead of starting it', async () => {
  const { ctx, sources, decodeAll } = stubContext()
  const p = new SegmentPlayer(() => ctx, () => {})
  const late = Promise.withResolvers<string>()
  const a = p.toggle(1, 0, 500, () => late.promise)
  const b = p.toggle(2, 500, 900, wav)
  late.resolve(btoa('late'))
  await decodeAll()
  await Promise.all([a, b])
  expect(sources).toHaveLength(1)
  expect(p.playing).toBe(2)
})

test('a failing fetch clears the marker and rejects with the engine message, in words', async () => {
  const { ctx } = stubContext()
  const marks: (number | null)[] = []
  const p = new SegmentPlayer(() => ctx, (m) => marks.push(m))
  await expect(p.toggle(3, 0, 100, async () => Promise.reject(new Error('no such track')))).rejects.toThrow('no such track')
  expect(marks).toEqual([3, null])
  expect(playError(new Error('no such track'))).toBe('Could not play this segment: no such track')
  expect(playError('boom')).toBe('Could not play this segment: boom')
  const noCtx = new SegmentPlayer(() => null, () => {})
  await expect(noCtx.toggle(1, 0, 1, wav)).rejects.toThrow('audio is not available')
})
