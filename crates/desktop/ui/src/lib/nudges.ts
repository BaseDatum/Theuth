// Settings › General › Appearance › Show theuth nudges: a client-only display preference, persisted like the
// theme. Nothing here changes what the engine sends; hidden nudges stay in the session and its context.

import { create } from 'zustand'

// localStorage key keeps its pre-rename name so the saved preference survives.
const KEY = 'show-harness-nudges'

type NudgeStore = {
  /** Whether `engine` transcript entries render; off unless the user opted in. */
  show: boolean
  setShow(on: boolean): void
}

export const useNudges = create<NudgeStore>()((set) => ({
  show: localStorage.getItem(KEY) === 'true',
  setShow(on) {
    localStorage.setItem(KEY, String(on))
    set({ show: on })
  },
}))
