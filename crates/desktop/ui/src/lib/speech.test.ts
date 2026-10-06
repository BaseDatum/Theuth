import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import * as speech from './speech'

// No React render in the runner: `useSpeaking()` reads the store's snapshot straight, to assert the badge.
vi.mock('react', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useSyncExternalStore: (_sub: unknown, get: () => unknown) => get(),
}))

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
// plus the tail grace (117 D1), and every source must be stopped before a suspend or a resume (117 D2).
const calls: string[] = []
class FakeSource {
  buffer: unknown = null
  onended: (() => void) | null = null
  connected = false
  stopped = false
  to: unknown = null
  connect(node: unknown) {
    this.connected = true
    this.to = node
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
class FakeGain {
  gain = { value: 0 }
  to: unknown = null
  connect(node: unknown) {
    this.to = node
  }
}
let context: FakeContext | null = null
class FakeContext {
  destination = {}
  currentTime = 0
  state = 'running'
  outputLatency = 0
  constructor() {
    context = this
  }
  createGain() {
    return new FakeGain()
  }
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
    if (!deferResume) {
      this.state = 'running'
      return Promise.resolve()
    }
    // iOS: the session activates asynchronously, `state` stays `suspended` until then.
    const { promise, resolve } = Promise.withResolvers<void>()
    pendingResumes.push(() => {
      this.state = 'running'
      resolve()
    })
    return promise
  }
  suspend() {
    calls.push('suspend')
    this.state = 'suspended'
    return Promise.resolve()
  }
}
/** Set by a test to hold every `resume()` open until `pendingResumes` are called. */
let deferResume = false
const pendingResumes: (() => void)[] = []
const audioSession = { type: 'auto' }
// `speech.ts` reads both lazily, inside `unlock`/`release`, so stubbing after the import is in time.
vi.stubGlobal('AudioContext', FakeContext)
vi.stubGlobal('navigator', { audioSession })
// The tail timer is `window.setTimeout`; no DOM in the runner.
vi.stubGlobal('window', globalThis)
vi.stubGlobal('document', { visibilityState: 'visible', addEventListener: () => undefined, removeEventListener: () => undefined })
speech.installSpeechListeners()
// Registered before any `unlock`: the node does not exist yet, so the callback must wait for the first context.
const outputReady: AudioNode[] = []
speech.onOutputReady((n) => outputReady.push(n))
expect(speech.outputNode()).toBeNull()

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
  vi.useFakeTimers()
  deferResume = false
  pendingResumes.length = 0
  for (const u of ['speak', 'dictate', 'record', 'talk'] as const) await speech.release(u)
  speech.setTalk(null)
  speech.setSpeaking(null)
  calls.length = 0
  sources.length = 0
})
afterEach(() => vi.useRealTimers())

/** How often the context was suspended. */
const suspends = () => calls.filter((c) => c === 'suspend').length

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

test('done releases speak only after the last scheduled source has ended, then the tail grace (117 D1)', async () => {
  await utter('s1', 3)
  expect(sources).toHaveLength(3)
  expect(speech.useSpeaking()).toEqual({ id: 's1', entry: 'e' })
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  sources[0]!.onended?.()
  sources[1]!.onended?.()
  await settle()
  expect(calls).not.toContain('suspend')
  expect(audioSession.type).toBe('playback')
  sources[2]!.onended?.()
  await settle()
  // Rendered is not yet heard: the badge drops now, the session stays up for the grace.
  expect(speech.useSpeaking()).toBeNull()
  expect(calls).not.toContain('suspend')
  expect(audioSession.type).toBe('playback')
  vi.advanceTimersByTime(speech.TAIL_MS - 1)
  expect(calls).not.toContain('suspend')
  vi.advanceTimersByTime(1)
  await settle()
  expect(suspends()).toBe(1)
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
  expect(calls).not.toContain('suspend')
  vi.advanceTimersByTime(speech.TAIL_MS)
  await settle()
  expect(suspends()).toBe(1)
})

test('the next block inside the grace keeps the context running: no suspend/resume churn between blocks (117 D1)', async () => {
  await utter('s1', 1)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  sources[0]!.onended?.()
  await settle()
  expect(speech.useSpeaking()).toBeNull()
  vi.advanceTimersByTime(speech.TAIL_MS / 2)
  await utter('s2', 1)
  expect(sources[1]!.connected).toBe(true)
  vi.advanceTimersByTime(speech.TAIL_MS * 2)
  await settle()
  expect(calls).not.toContain('suspend')
  emit('SpeechFinished', { id: 's2', reason: { reason: 'done' } })
  await settle()
  sources[1]!.onended?.()
  vi.advanceTimersByTime(speech.TAIL_MS)
  await settle()
  expect(suspends()).toBe(1)
  expect(audioSession.type).toBe('auto')
})

test('stop inside the grace suspends at once, and the timer does not suspend again', async () => {
  await utter('s1', 1)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  sources[0]!.onended?.()
  await settle()
  speech.stop()
  await settle()
  expect(suspends()).toBe(1)
  expect(audioSession.type).toBe('auto')
  vi.advanceTimersByTime(speech.TAIL_MS * 3)
  await settle()
  expect(suspends()).toBe(1)
})

test('a mic use unlocking a context iOS interrupted cuts the frozen tail and gives the reply up (117 D2)', async () => {
  await utter('s1', 2)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  // Interrupted mid-tail: the two sources froze in the graph, their `onended` never comes.
  context!.state = 'interrupted'
  const out = speech.outputNode()
  calls.length = 0
  speech.unlock('record')
  expect(sources.map((s) => s.stopped)).toEqual([true, true])
  expect(speech.outputNode()).toBe(out)
  expect(speech.useSpeaking()).toBeNull()
  expect(audioSession.type).toBe('play-and-record')
  expect(calls).toEqual(['stop', 'stop', 'resume'])
  await speech.release('record')
  expect(suspends()).toBe(1)
  expect(audioSession.type).toBe('auto')
})

test('a new utterance on an interrupted context starts clean: the frozen tail is stopped, the new packet plays (117 D2)', async () => {
  await utter('s1', 2)
  emit('SpeechFinished', { id: 's1', reason: { reason: 'done' } })
  await settle()
  context!.state = 'interrupted'
  speech.start('s2', 24_000)
  speech.push('s2', 'AA==')
  await settle()
  expect(sources.map((s) => s.stopped)).toEqual([true, true, false])
  expect(sources[2]!.connected).toBe(true)
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

test('a second unlock while our own resume is still activating is not a frozen context: nothing is cut (117 D2 R1)', async () => {
  deferResume = true
  await utter('s1', 2)
  expect(context!.state).toBe('suspended')
  expect(sources).toHaveLength(2)
  // mic-tap's second `unlock` after `getUserMedia`, before the activation landed.
  speech.unlock('record')
  expect(sources.map((s) => s.stopped)).toEqual([false, false])
  expect(calls).not.toContain('stop')
  expect(calls).not.toContain('suspend')
  expect(audioSession.type).toBe('play-and-record')
  for (const done of pendingResumes.splice(0)) done()
  await settle()
  expect(context!.state).toBe('running')
  expect(calls).not.toContain('stop')
  await speech.release('record')
  expect(audioSession.type).toBe('playback')
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

test('every scheduled source plays through the one output gain (83 §1), announced once the context exists', async () => {
  await utter('s1', 3)
  const out = speech.outputNode() as unknown as FakeGain
  expect(out).toBeInstanceOf(FakeGain)
  expect(out.gain.value).toBe(1)
  expect(out.to).toBe((speech.unlock('speak') as unknown as FakeContext).destination)
  expect(sources.map((s) => s.to)).toEqual([out, out, out])
  expect(outputReady).toEqual([out])
  // Late subscribers get it at once; the node is the same for the module's life.
  const late = vi.fn()
  speech.onOutputReady(late)()
  expect(late).toHaveBeenCalledWith(out)
  await speech.release('speak')
})

test("a client-sink talk claims its session's utterances and queues them gaplessly; played_seq follows what ended", async () => {
  speech.unlock('talk')
  speech.setTalk('sess')
  expect(audioSession.type).toBe('play-and-record')
  emit('SpeechStarted', { id: 'a', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'a', session: 'sess', seq: 0, opus: 'AA==' })
  emit('SpeechAudio', { id: 'a', session: 'sess', seq: 1, opus: 'AA==' })
  await settle()
  expect(sources).toHaveLength(2)
  expect(speech.playedSeq()).toBeNull()
  sources[0]!.onended?.()
  expect(speech.playedSeq()).toBe(0)
  // The next sentence starts while the first's tail is still scheduled: nothing is cut.
  emit('SpeechFinished', { id: 'a', session: 'sess', reason: { reason: 'done' } })
  emit('SpeechStarted', { id: 'b', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'b', session: 'sess', seq: 0, opus: 'AA==' })
  await settle()
  expect(sources[1]!.stopped).toBe(false)
  expect(sources).toHaveLength(3)
  // A late `onended` of the previous utterance does not count towards the current one.
  sources[1]!.onended?.()
  expect(speech.playedSeq()).toBeNull()
  // Another session's speech is not ours; nor is a device-sink utterance on our session.
  emit('SpeechStarted', { id: 'z', session: 'other', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'z', session: 'other', seq: 0, opus: 'AA==' })
  emit('SpeechStarted', { id: 'd', session: 'sess', sink: 'device', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'd', session: 'sess', seq: 0, opus: 'AA==' })
  await settle()
  expect(sources).toHaveLength(3)
  // `Flush` = stop: everything scheduled is cut, the decoder freed; the talk leg stays claimed.
  speech.stop()
  expect(sources[2]!.stopped).toBe(true)
  emit('SpeechStarted', { id: 'c', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'c', session: 'sess', seq: 0, opus: 'AA==' })
  await settle()
  expect(sources).toHaveLength(4)
  speech.setTalk(null)
  await speech.release('talk')
  expect(audioSession.type).toBe('auto')
})

test('a talk reports the utterance heard now: from the first packet, at the join to the next, null when the tail ends, `cut` on a stop', async () => {
  const heard: (string | null)[] = []
  // Ids no earlier test left claimed (`mine` is module state).
  speech.unlock('talk')
  speech.setTalk('sess', (s, cut) => heard.push(cut ? 'cut' : s))
  emit('SpeechStarted', { id: 'ha', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'ha', session: 'sess', seq: 0, opus: 'AA==' })
  emit('SpeechAudio', { id: 'ha', session: 'sess', seq: 1, opus: 'AA==' })
  await settle()
  expect(heard).toEqual(['ha'])
  // Synthesis ahead of playback: `b` is announced, queued and "finished" while `a` still plays.
  emit('SpeechFinished', { id: 'ha', session: 'sess', reason: { reason: 'done' } })
  emit('SpeechStarted', { id: 'hb', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'hb', session: 'sess', seq: 0, opus: 'AA==' })
  emit('SpeechFinished', { id: 'hb', session: 'sess', reason: { reason: 'done' } })
  await settle()
  sources[0]!.onended?.()
  expect(heard).toEqual(['ha'])
  sources[1]!.onended?.()
  expect(heard).toEqual(['ha', 'hb'])
  sources[2]!.onended?.()
  await settle()
  expect(heard).toEqual(['ha', 'hb', null])
  // An underrun (sources drained, packets still to come) is not the end of the sentence…
  emit('SpeechStarted', { id: 'hc', session: 'sess', sink: 'client', sample_rate: 24_000 })
  emit('SpeechAudio', { id: 'hc', session: 'sess', seq: 0, opus: 'AA==' })
  await settle()
  sources[3]!.onended?.()
  expect(heard).toEqual(['ha', 'hb', null, 'hc'])
  emit('SpeechAudio', { id: 'hc', session: 'sess', seq: 1, opus: 'AA==' })
  await settle()
  expect(heard).toEqual(['ha', 'hb', null, 'hc'])
  // …a `Flush` (stop) is, and says so (`cut`: the talk store holds the line for `Interrupted`).
  speech.stop()
  expect(heard).toEqual(['ha', 'hb', null, 'hc', 'cut'])
  speech.setTalk(null)
  await speech.release('talk')
})
