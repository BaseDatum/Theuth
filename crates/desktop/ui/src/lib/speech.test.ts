import { beforeEach, expect, test, vi } from 'vitest'
import * as speech from './speech'

// `start`/`prime` open a decoder (libopus as wasm); the session logic under test never touches it.
vi.mock('opus-decoder', () => ({
  OpusDecoder: class {
    ready = Promise.resolve()
    free() {}
  },
}))

// The session owner (speech.ts `unlock`/`release`) against a fake `AudioContext` and `navigator.audioSession`:
// iOS keeps its Now Playing island up while the page's context runs or the type is forced, so the idle state
// (nothing speaking, dictating or recording) must be exactly "suspended + `auto`".
const calls: string[] = []
class FakeContext {
  destination = {}
  currentTime = 0
  createGain() {
    return { connect: () => undefined, disconnect: () => undefined }
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

beforeEach(async () => {
  for (const u of ['speak', 'dictate', 'record'] as const) await speech.release(u)
  calls.length = 0
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
