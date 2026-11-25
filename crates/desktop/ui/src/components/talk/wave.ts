// 82 §D: the talk's waveform input, shared by the talk view's band and the composer's running-talk glyph. The mic
// feeds it while the user may speak (the relay tap's analyser, else the engine's level packets); `thinking` pulses;
// `speaking` shows the agent's voice off speech.ts's output node (83 §1).

import { type VoiceState, useVoiceSource } from '@/components/voice/VoiceWave'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import * as speech from '@/lib/speech'
import * as talk from '@/lib/talk'
import { tapOrLevels } from '@/lib/visual-source'
import type { State } from '@/rpc/bindings/method.talk.output'

/** `TalkState` → the waveform's look. */
export const talkWave = (state: State): VoiceState =>
  state === 'listening' || state === 'interrupted' || state === 'holding' ? 'listening' : state === 'thinking' || state === 'speaking' ? state : 'idle'

/** The talk's look and its live frames while it runs `here`: the mic while listening, the agent's voice while speaking, else `null`. */
export function useTalkWave(here: boolean, state: State): { wave: VoiceState; source: FrameSource<VisualFrame> | null } {
  const wave = talkWave(state)
  const mic = useVoiceSource(here, () => tapOrLevels(talk.currentTap(), talk.onLevels))
  const voice = useVoiceSource(here && wave === 'speaking', speech.speakerSource)
  return { wave, source: wave === 'listening' ? mic : wave === 'speaking' ? voice : null }
}
