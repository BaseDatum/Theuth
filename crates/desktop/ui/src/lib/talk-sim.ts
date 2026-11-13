// Dev-only (item 7 of the 76 brief): `?sim=1` drives the talk view with a scripted `TalkEvent` sequence through
// the same `handle` the bus feeds, so the UX can be judged without an engine. `?sim=idle|thinking|speaking`
// runs to the first event that puts the orb in that state and holds there (the screenshot hook). `?sim=client`
// plays the client-sink script: the engine's `Saying`/`SpeechFinished` run seconds ahead and the agent line follows
// this speaker's `Playing`, and a barge-in freezes the cut sentence until the next turn (TALK-UI-A A1); the tool
// steps light `Working…` above the orb (A2). Tree-shaken out of production: every import site is behind
// `import.meta.env.DEV`.

import * as speech from '@/lib/speech'
import { handle, type OrbState, type SubEvent } from '@/lib/talk'
import { dispatch } from '@/store/sessions'

const ID = 'sim-talk'

/** `[delay before, event]`; the script loops. A function step drives something other than the talk store. */
type Step = [number, SubEvent | (() => void)]

/** The session's tool activity (`Working…` above the orb): a pending call the transcript shows as a sim card. */
const tool = (session: string, on: boolean) =>
  dispatch([
    on
      ? { type: 'ToolCallPending', session, turn: 'sim-turn', call_id: 'sim-tool', name: 'sim_lookup' }
      : { type: 'ToolCallFinished', session, turn: 'sim-turn', call_id: 'sim-tool', output: 'ok', is_error: false, files: [] },
  ])

/** The engine's speaker (`sink: device`): `Saying` is within the ring's 200 ms of playback. */
const script = (session: string): Step[] => [
  [0, { type: 'TalkStarted', id: ID, session, sink: 'device', aec: 'none', since_ms: 0 }],
  [0, { type: 'Degraded', id: ID, degradation: 'mic_no_aec' }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [900, { type: 'Heard', id: ID, utterance: 0, text: 'So what', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way to learn a language as an adult', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way to learn a language as an adult who already speaks two?', final: true }],
  [400, { type: 'Submitted', id: ID, text: 'So what would you say is the fastest way to learn a language as an adult who already speaks two?', interrupted: false }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [800, () => tool(session, true)],
  [1200, () => tool(session, false)],
  [600, { type: 'TalkState', id: ID, state: 'speaking' }],
  [0, { type: 'Saying', id: ID, speech: 's1', text: 'Speak from day one, badly and often, with someone who answers back.' }],
  [3200, { type: 'SpeechFinished', id: 's1', session, reason: { reason: 'done' }, sink: 'device' }],
  [50, { type: 'Saying', id: ID, speech: 's2', text: 'Your two languages already gave you the habit of tolerating ambiguity, so lean on comprehensible input and skip the grammar drills until the patterns start to itch.' }],
  [1800, { type: 'TalkState', id: ID, state: 'holding' }],
  [0, { type: 'Holding', id: ID, since_speech_ms: 400 }],
  [300, { type: 'Heard', id: ID, utterance: 1, text: 'Wait, what', final: false }],
  // The barge-in commits: the speaker stops (`stopped`), the line freezes on the cut sentence until `thinking`.
  [400, { type: 'SpeechFinished', id: 's2', session, reason: { reason: 'stopped' }, sink: 'device' }],
  [0, { type: 'Interrupted', id: ID, heard_sentences: 1, dropped_sentences: 1 }],
  [0, { type: 'TalkState', id: ID, state: 'interrupted' }],
  [200, { type: 'TalkState', id: ID, state: 'listening' }],
  [500, { type: 'Heard', id: ID, utterance: 1, text: 'Wait, what do you mean by comprehensible input?', final: true }],
  [600, { type: 'Submitted', id: ID, text: 'Wait, what do you mean by comprehensible input?', interrupted: true }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [1200, { type: 'TalkState', id: ID, state: 'speaking' }],
  [0, { type: 'Saying', id: ID, speech: 's3', text: 'Material you understand about ninety percent of, so the rest fills in from context.' }],
  [3000, { type: 'SpeechFinished', id: 's3', session, reason: { reason: 'done' }, sink: 'device' }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [2500, { type: 'TalkFinished', id: ID, reason: { reason: 'stopped' } }],
  [800, { type: 'TalkStarted', id: ID, session, sink: 'device', aec: 'none', since_ms: 0 }],
]

/**
 * This client's speaker (`sink: client`, a phone or a linked desktop): synthesis outruns speech, so every `Saying`
 * and `SpeechFinished` of the reply lands within ~2 s while playback takes ~9 s. The line must move with `Playing`.
 */
const clientScript = (session: string): Step[] => [
  [0, { type: 'TalkStarted', id: ID, session, sink: 'client', aec: 'none', since_ms: 0 }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [900, { type: 'Heard', id: ID, utterance: 0, text: 'What time is it', final: false }],
  [600, { type: 'Heard', id: ID, utterance: 0, text: 'What time is it in Lisbon right now?', final: true }],
  [400, { type: 'Submitted', id: ID, text: 'What time is it in Lisbon right now?', interrupted: false }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [1500, { type: 'TalkState', id: ID, state: 'speaking' }],
  [0, { type: 'Saying', id: ID, speech: 'c1', text: 'It is twenty past ten in the evening in Lisbon.' }],
  [300, { type: 'Playing', speech: 'c1' }],
  [700, { type: 'SpeechFinished', id: 'c1', session, reason: { reason: 'done' }, sink: 'client' }],
  [0, { type: 'Saying', id: ID, speech: 'c2', text: 'That is one hour behind you, so your call at eleven is their ten.' }],
  [800, { type: 'SpeechFinished', id: 'c2', session, reason: { reason: 'done' }, sink: 'client' }],
  [0, { type: 'Saying', id: ID, speech: 'c3', text: 'Shall I move it?' }],
  [200, { type: 'SpeechFinished', id: 'c3', session, reason: { reason: 'done' }, sink: 'client' }],
  // The engine is done with the turn long before the phone is; the store must not clear the line on its say-so.
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [1000, { type: 'Playing', speech: 'c2' }],
  [3500, { type: 'Playing', speech: 'c3' }],
  [1200, { type: 'Playing', speech: null }],
  // A second turn, cut by a barge-in: `Flush` stops this speaker (`Playing null, cut`), then `Interrupted`; the
  // line holds the cut sentence while the user talks and clears when the next turn starts.
  [1500, { type: 'Heard', id: ID, utterance: 1, text: 'Yes please', final: true }],
  [400, { type: 'Submitted', id: ID, text: 'Yes please', interrupted: false }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [900, { type: 'TalkState', id: ID, state: 'speaking' }],
  [0, { type: 'Saying', id: ID, speech: 'c4', text: 'Done. I moved the call to ten, their time, and sent everyone the new invite with a short note.' }],
  [200, { type: 'Playing', speech: 'c4' }],
  [0, { type: 'SpeechFinished', id: 'c4', session, reason: { reason: 'done' }, sink: 'client' }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [1800, { type: 'Heard', id: ID, utterance: 2, text: 'Wait', final: false }],
  [300, { type: 'Flush', id: ID }],
  [0, { type: 'Playing', speech: null, cut: true }],
  [0, { type: 'Interrupted', id: ID, heard_sentences: 0, dropped_sentences: 1 }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [700, { type: 'Heard', id: ID, utterance: 2, text: 'Wait, which invite?', final: true }],
  [500, { type: 'Submitted', id: ID, text: 'Wait, which invite?', interrupted: true }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [1500, { type: 'TalkState', id: ID, state: 'listening' }],
  [2500, { type: 'TalkFinished', id: ID, reason: { reason: 'stopped' } }],
  [800, { type: 'TalkStarted', id: ID, session, sink: 'client', aec: 'none', since_ms: 0 }],
]

export type SimFlag = { hold: OrbState | null; client: boolean }

/**
 * 83 §1: the sim has no Opus leg, so while it says `speaking` a wobbling tone plays through speech.ts's output node
 * — the same node the band taps — and the band moves as it would with the agent's voice. `unlock('speak')` here is
 * the sim's stand-in for the `speak` hold an utterance takes.
 */
function tone(): () => void {
  const ctx = speech.unlock('speak')
  const out = speech.outputNode()
  if (!out) return () => undefined
  const osc = ctx.createOscillator()
  const lfo = ctx.createOscillator()
  const depth = ctx.createGain()
  const vol = ctx.createGain()
  osc.frequency.value = 180
  lfo.frequency.value = 4
  depth.gain.value = 0.25
  vol.gain.value = 0.25
  lfo.connect(depth).connect(vol.gain)
  osc.connect(vol).connect(out)
  osc.start()
  lfo.start()
  return () => {
    osc.stop()
    lfo.stop()
    vol.disconnect()
    void speech.release('speak')
  }
}

/** `?sim=` value → run; returns the stop function. `hold` freezes at the first event that shows that state with its subtitle. */
export function runSim(session: string, { hold, client }: SimFlag): () => void {
  const steps = (client ? clientScript : script)(session)
  let i = 0
  let timer = 0
  let stopTone: (() => void) | null = null
  const held = (ev: SubEvent) =>
    hold === 'idle'
      ? ev.type === 'Heard' && !ev.final && ev.text.split(' ').length >= 8
      : hold === 'thinking'
        ? ev.type === 'TalkState' && ev.state === 'thinking'
        : hold === 'speaking' && ev.type === 'Saying' && ev.speech === 's2'
  const tick = () => {
    const [delay, ev] = steps[i % steps.length]!
    timer = window.setTimeout(() => {
      if (typeof ev === 'function') ev()
      else {
        handle(ev)
        if (ev.type === 'TalkState') {
          stopTone?.()
          stopTone = ev.state === 'speaking' ? tone() : null
        }
      }
      i++
      if (typeof ev === 'function' || !held(ev)) tick()
    }, delay)
  }
  tick()
  return () => {
    clearTimeout(timer)
    stopTone?.()
  }
}

/** The `?sim` flag of the current URL: `hold` names the state to freeze in, `null` = loop; `client` = the client-sink script. */
export function simFlag(): SimFlag | null {
  const v = new URLSearchParams(location.search).get('sim')
  if (!v) return null
  return { hold: v === 'idle' || v === 'thinking' || v === 'speaking' ? v : null, client: v === 'client' }
}
