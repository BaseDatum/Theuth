// Settings › Voice › Microphone › Talk (76 TALK-UI-A A2; 188 moved it from Chat): client-only talk preferences, persisted like the theme.

import { create } from 'zustand'

const KEY = 'talk-cue'

type TalkSettings = {
  /** `talk.cue`: the soft chime when the model takes the turn; on unless the user opted out. */
  cue: boolean
  setCue(on: boolean): void
}

export const useTalkSettings = create<TalkSettings>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  cue: globalThis.localStorage?.getItem(KEY) !== 'false',
  setCue(on) {
    localStorage.setItem(KEY, String(on))
    set({ cue: on })
  },
}))
