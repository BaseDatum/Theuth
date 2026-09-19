// Settings › General › Show harness nudges: a client-only display preference, persisted like the
// theme. Nothing here changes what the harness sends; hidden nudges stay in the session and its context.

import { create } from 'zustand'

const KEY = 'show-harness-nudges'

type NudgeStore = {
  /** Whether `harness` transcript entries render; off unless the user opted in. */
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
