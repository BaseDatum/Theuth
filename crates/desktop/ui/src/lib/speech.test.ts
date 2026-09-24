import { beforeEach, expect, test, vi } from 'vitest'
import * as speech from './speech'

// `start`/`prime` open a decoder (libopus as wasm); the fake decodes any packet to 10 ms of silence.
vi.mock('opus-decoder', () => ({
  OpusDecoder: class {
    ready = Promise.resolve()
    decodeFrame() {
      return { channelData: [new Float32Array(240)] }
    }
    free() {}
  },
}))
// The `VoiceEvent` bus: `installSpeechListeners` subscribes through `on`; tests fire events with `emit`.
const handlers = vi.hoisted(() => new Map<string, (e: unknown) => void>())
vi.mock('@/store/sessions', () => ({
  on: (type: string, cb: (e: unknown) => void) => {
    handlers.set(type, cb)
    return () => handlers.delete(type)
  },
  useSessions: { getState: () => ({ sessions: {} }) },
}))
const emit = (type: string, e: Record<string, unknown>) => handlers.get(type)?.({ type, ...e })

// The session owner (speech.ts `unlock`/`release`) against a fake `AudioContext` and `navigator.audioSession`:
// iOS keeps its Now Playing island up while the page's context runs or the type is forced, so the idle state
// (nothing speaking, dictating or recording) must be exactly "suspended + `auto`". A suspended context keeps
// its scheduled sources and plays them on the next resume, so `speak` must be held until the last one ended
// and every source must be stopped before a suspend.
const calls: string[] = []
class FakeSource {
  buffer: unknown = null
  onended: (() => void) | null = null
  connected = false
  stopped = false
  connect() {
    this.connected = true
  }
  disconnect() {
    this.connected = false
  }
  start() {}
  stop() {
    this.stopped = true
    calls.push('stop')
  }
}
const sources: FakeSource[] = []
class FakeContext {
  destination = {}
  currentTime = 0
  createBuffer(_ch: number, length: number, rate: number) {
    return { duration: length / rate, copyToChannel: () => undefined }
  }
  createBufferSource() {
    const s = new FakeSource()
    sources.push(s)
    return s
  }
  resume() {
    calls.push('resume')
    return Promise.resolve()
  }
  suspend() {
    calls.push('suspend')
    return Promise.resolve()
  }
}
const audioSession = { type: 'auto' }
// `speech.ts` reads both lazily, inside `unlock`/`release`, so stubbing after the import is in time.
vi.stubGlobal('AudioContext', FakeContext)
vi.stubGlobal('navigator', { audioSession })
vi.stubGlobal('document', { visibilityState: 'visible', addEventListener: () => undefined, removeEventListener: () => undefined })
speech.installSpeechListeners()

/** Decode + schedule run behind the decoder's `ready`, `release` sets the type behind its `suspend`: a few microtask turns settle them. */
async function settle(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve()
}
/** An utterance this client asked for: `speak` resolved with `id`, then `SpeechStarted` and `n` packets. */
async function utter(id: string, n: number): Promise<void> {
  speech.prime()
  await speech.speak({ speak: () => Promise.resolve(id) } as never, 's', 'e', 'text')
  emit('SpeechStarted', { id, sample_rate: 24_000 })
  for (let i = 0; i < n; i++) emit('SpeechAudio', { id, opus: 'AA==' })
  await settle()
}

beforeEach(async () => {
  for (const u of ['speak', 'dictate', 'record'] as const) await speech.release(u)
  speech.setSpeaking(null)
  calls.length = 0
  sources.length = 0
})

test('a lone use holds the session and its release leaves it suspended and auto', async () => {
  speech.unlock('dictate')
  expect(audioSession.type).toBe('play-and-record')
  expect(calls).toEqual(['resume'])
  await speech.release('dictate')
  expect(calls).toEqual(['resume', 'suspend'])
  expect(audioSession.type).toBe('auto')
})

test('speaking during a dictation neither flips the mic type nor suspends on its own stop', async () => {
  speech.unlock('dictate')
  speech.start('s1', 24_000)
  expect(audioSession.type).toBe('play-and-record')
  speech.stop()
  await Promise.resolve()
  expect(calls).not.toContain('suspend')
  expect(audioSession.type).toBe('play-and-record')
  await speech.release('dictate')
  expect(calls).toContain('suspend')
  expect(audioSession.type).toBe('auto')
})

test('a dictation ending while speaking drops back to playback, still running', async () => {
  speech.start('s1', 24_000)
  speech.unlock('dictate')
  await speech.release('dictate')
  expect(audioSession.type).toBe('playback')
  expect(calls).not.toContain('suspend')
})

test('a stray release of a use never held leaves a real holder untouched', async () => {
  await speech.release('record')
  expect(audioSession.type).toBe('auto')
  calls.length = 0
  speech.unlock('record')
  await speech.release('dictate')
  expect(audioSession.type).toBe('play-and-record')
  expect(calls).toEqual(['resume'])
})

test('done releases speak only after the last scheduled source has ended', async () => {
  await utter('s1', 3)
  expect(sources).toHaveLength(3)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  sources[0]!.onended?.()
  sources[1]!.onended?.()
  await settle()
  expect(calls).not.toContain('suspend')
  expect(audioSession.type).toBe('playback')
  sources[2]!.onended?.()
  await settle()
  expect(calls).toContain('suspend')
  expect(audioSession.type).toBe('auto')
})

test('a packet arriving after the scheduled sources drained keeps the hold and still plays', async () => {
  await utter('s1', 1)
  sources[0]!.onended?.()
  await settle()
  expect(calls).not.toContain('suspend')
  emit('SpeechAudio', { id: 's1', opus: 'AA==' })
  await settle()
  expect(sources).toHaveLength(2)
  expect(sources[1]!.connected).toBe(true)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  expect(calls).not.toContain('suspend')
  sources[1]!.onended?.()
  await settle()
  expect(calls).toContain('suspend')
})

test('stop halts every scheduled source before the suspend, so none can resume later', async () => {
  await utter('s1', 2)
  speech.stop()
  await settle()
  expect(sources.map((s) => s.stopped)).toEqual([true, true])
  expect(sources.map((s) => s.connected)).toEqual([false, false])
  expect(calls.slice(-3)).toEqual(['stop', 'stop', 'suspend'])
})

test('a second unlock after a stop resumes the context with nothing of the old utterance scheduled', async () => {
  await utter('s1', 2)
  speech.stop()
  await settle()
  calls.length = 0
  speech.unlock('dictate')
  expect(calls).toEqual(['resume'])
  expect(sources.every((s) => s.stopped && !s.connected)).toBe(true)
  await utter('s2', 1)
  expect(sources).toHaveLength(3)
  expect(sources[2]!.connected).toBe(true)
})

test('the last holder out stops leftover sources before suspending', async () => {
  speech.unlock('dictate')
  await utter('s1', 1)
  // The hold given back with audio still scheduled and another holder keeping the context running.
  await speech.release('speak')
  expect(calls).not.toContain('suspend')
  expect(sources[0]!.stopped).toBe(false)
  await speech.release('dictate')
  expect(sources[0]!.stopped).toBe(true)
  expect(calls.slice(-2)).toEqual(['stop', 'suspend'])
})
