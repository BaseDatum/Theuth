// Dev-only (item 7 of the 76 brief): `?sim=1` drives the talk view with a scripted `TalkEvent` sequence through
// the same `handle` the bus feeds, so the UX can be judged without an engine. `?sim=idle|thinking|speaking`
// runs to the first event that puts the orb in that state and holds there (the screenshot hook). Tree-shaken out
// of production: every import site is behind `import.meta.env.DEV`.

import type { TalkEvent } from '@/rpc/bindings/TalkEvent'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import { handle, type OrbState } from '@/lib/talk'

type Ev = TalkEvent | Extract<VoiceEvent, { type: 'SpeechFinished' }>

const ID = 'sim-talk'

/** `[delay before, event]`; the script loops. */
const script = (session: string): [number, Ev][] => [
  [0, { type: 'TalkStarted', id: ID, session, sink: 'device', aec: 'none', since_ms: 0n }],
  [0, { type: 'Degraded', id: ID, degradation: 'mic_no_aec' }],
  [0, { type: 'TalkState', id: ID, state: 'listening' }],
  [900, { type: 'Heard', id: ID, utterance: 0, text: 'So what', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way to learn a language as an adult', final: false }],
  [500, { type: 'Heard', id: ID, utterance: 0, text: 'So what would you say is the fastest way to learn a language as an adult who already speaks two?', final: true }],
  [400, { type: 'Submitted', id: ID, text: 'So what would you say is the fastest way to learn a language as an adult who already speaks two?', interrupted: false }],
  [0, { type: 'TalkState', id: ID, state: 'thinking' }],
  [1600, { type: 'TalkState', id: ID, state: 'speaking' }],
  [0, { type: 'Saying', id: ID, speech: 's1', text: 'Speak from day one, badly and often, with someone who answers back.' }],
  [3200, { type: 'SpeechFinished', id: 's1', session, reason: { reason: 'done' }, sink: 'device' }],
  [50, { type: 'Saying', id: ID, speech: 's2', text: 'Your two languages already gave you the habit of tolerating ambiguity, so lean on comprehensible input and skip the grammar drills until the patterns start to itch.' }],
  [1800, { type: 'TalkState', id: ID, state: 'holding' }],
  [0, { type: 'Holding', id: ID, since_speech_ms: 400n }],
  [300, { type: 'Heard', id: ID, utterance: 1, text: 'Wait, what', final: false }],
  [400, { type: 'Interrupted', id: ID, heard_sentences: 1, dropped_sentences: 1 }],
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
  [800, { type: 'TalkStarted', id: ID, session, sink: 'device', aec: 'none', since_ms: 0n }],
]

/** `?sim=` value → run; returns the stop function. `hold` freezes at the first event that shows that state with its subtitle. */
export function runSim(session: string, hold: OrbState | null): () => void {
  const steps = script(session)
  let i = 0
  let timer = 0
  const held = (ev: Ev) =>
    hold === 'idle'
      ? ev.type === 'Heard' && !ev.final && ev.text.split(' ').length >= 8
      : hold === 'thinking'
        ? ev.type === 'TalkState' && ev.state === 'thinking'
        : hold === 'speaking' && ev.type === 'Saying' && ev.speech === 's2'
  const tick = () => {
    const [delay, ev] = steps[i % steps.length]!
    timer = window.setTimeout(() => {
      handle(ev)
      i++
      if (!held(ev)) tick()
    }, delay)
  }
  tick()
  return () => clearTimeout(timer)
}

/** The `?sim` flag of the current URL: `hold` names the state to freeze in, `null` = loop. */
export function simFlag(): { hold: OrbState | null } | null {
  const v = new URLSearchParams(location.search).get('sim')
  if (!v) return null
  return { hold: v === 'idle' || v === 'thinking' || v === 'speaking' ? v : null }
}
